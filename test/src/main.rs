use anyhow::{Context, Result};
use clap::Parser;
use noxapi::Nox;
use noxrelay::{
    AvatarChangeRequest, EnterFlags, EnterRequest, HandshakeRequest, NoxRelay, QuicConnector,
    RelayInstance, SessionRequest, TravelingAction, TravelingRequest,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use url::Url;

mod movements;

/// Get the platform name based on the OS
fn get_platform() -> &'static str {
    #[cfg(target_os = "windows")]
    return "windows";

    #[cfg(target_os = "linux")]
    return "linux";

    #[cfg(target_os = "macos")]
    return "macos";

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return "unknown";
}

/// Nox bot load testing tool
#[derive(Parser, Debug)]
#[command(name = "noxbot")]
#[command(about = "Load testing bot for Nox relay servers")]
struct Args {
    /// Instance to connect to in format: id@server (e.g., 1@hactazia.fr)
    #[arg(short, long)]
    instance: String,

    /// Custom config directory (default: ~/.local/share/.nox or %APPDATA%/.nox)
    #[arg(short, long)]
    config_dir: Option<PathBuf>,

    /// Number of bots to create
    #[arg(long, default_value = "64")]
    count: usize,

    /// Number of concurrent bot workers
    #[arg(long, default_value = "1")]
    concurrent: usize,

    /// Delay in milliseconds between bot spawns
    #[arg(long, default_value = "1000")]
    delay: u64,

    /// Print all received relay packets as JSON (uses serde_json)
    #[arg(long, default_value = "false")]
    listen: bool,

    /// Movement mode: random, circular, rtp, square, cross, target
    #[arg(long, default_value = "random")]
    movement: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("noxbot=debug".parse().unwrap())
                .add_directive("noxrelay=info".parse().unwrap())
                .add_directive("nox_api=debug".parse().unwrap()),
        )
        .init();

    info!("Starting Nox bot test...");

    // Parse instance argument (format: id@server)
    let instance_parts: Vec<&str> = args.instance.split('@').collect();
    if instance_parts.len() != 2 {
        error!("Invalid instance format. Expected: id@server (e.g., 1@hactazia.fr)");
        return Ok(());
    }

    let instance_id: u32 = instance_parts[0]
        .parse()
        .context("Failed to parse instance ID")?;
    let server = instance_parts[1];

    info!("Target instance: {} (server: {})", instance_id, server);

    // Display config directory
    if let Some(ref config_dir) = args.config_dir {
        info!("Using custom config directory: {:?}", config_dir);
    } else {
        info!(
            "Using default config directory: {:?}",
            noxrelay::NoxCredentials::nox_folder(None)
        );
    }

    // Create API client with automatic gateway discovery
    let nox = Nox::with_discovery(server)
        .await
        .context("Failed to discover API gateway")?;

    // Get instance info
    info!("Fetching instance info...");
    let instance_response = nox.get_instance_by_id(instance_id).await;
    let instance = match instance_response.data {
        Some(inst) => inst,
        None => {
            error!("Failed to fetch instance: {:?}", instance_response.error);
            return Ok(());
        }
    };

    info!(
        "Instance: {} (ID: {}, Players: {}/{})",
        instance.name, instance.id, instance.client_count, instance.capacity
    );

    // Parse connection info
    if instance.connection.method != "relay" {
        error!(
            "Unsupported connection method: {}",
            instance.connection.method
        );
        return Ok(());
    }

    use base64::{engine::general_purpose, Engine as _};
    let connection_data = general_purpose::STANDARD
        .decode(&instance.connection.data)
        .context("Failed to decode connection data")?;
    let connection_json: serde_json::Value =
        serde_json::from_slice(&connection_data).context("Failed to parse connection JSON")?;

    let addresses = connection_json["a"]
        .as_array()
        .context("No addresses found")?;

    // Filter QUIC addresses only
    let relay_addresses: Vec<String> = addresses
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| s.starts_with("quic://"))
        .map(|s| s.to_string())
        .collect();

    if relay_addresses.is_empty() {
        error!("No QUIC relay addresses found");
        return Ok(());
    }

    info!("Using {} QUIC relay addresses", relay_addresses.len());

    // Bot creation parameters
    info!(
        "Creating {} bots with {} concurrent workers ({}ms delay between spawns)...",
        args.count, args.concurrent, args.delay
    );

    let shutdown = Arc::new(AtomicBool::new(false));

    // Track active bot tasks for graceful shutdown
    let active_bots = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Load the API token for the current server (best-effort)
    let api_token: Option<String> = noxrelay::NoxCredentials::load_config(args.config_dir.clone())
        .ok()
        .and_then(|cfg| {
            let token = cfg.servers.get(&cfg.server)?.token.clone();
            token
        });

    // Create a queue for bot creation commands
    struct BotCommand {
        index: usize,
        relay_addr: String,
        instance_id: u64,
        config_dir: Option<PathBuf>,
        nox: Nox,
        token: Option<String>,
        listen: bool,
        movement: String,
    }

    let (tx, rx) = mpsc::channel::<BotCommand>(args.count);
    let rx = Arc::new(tokio::sync::Mutex::new(rx));

    // Spawn workers that consume from the queue
    let mut worker_handles = Vec::new();
    for worker_id in 0..args.concurrent {
        let rx = rx.clone();
        let shutdown_flag = shutdown.clone();
        let active_bots_ref = active_bots.clone();

        let worker = tokio::spawn(async move {
            loop {
                // Check if shutdown was requested
                if shutdown_flag.load(Ordering::Relaxed) {
                    break;
                }

                // Try to get a bot command from the queue
                let command = {
                    let mut rx_guard = rx.lock().await;
                    rx_guard.recv().await
                };

                match command {
                    Some(cmd) => {
                        if let Err(e) = create_bot(
                            cmd.index,
                            &cmd.relay_addr,
                            cmd.instance_id,
                            cmd.config_dir,
                            cmd.nox,
                            cmd.token,
                            cmd.listen,
                            &cmd.movement,
                            shutdown_flag.clone(),
                            active_bots_ref.clone(),
                        )
                        .await
                        {
                            error!("Bot {} error: {}", cmd.index, e);
                        }
                    }
                    None => break, // Channel closed
                }
            }

            info!("Worker {} shutting down", worker_id);
        });

        worker_handles.push(worker);
    }

    // Enqueue all bot creation commands with delay
    let tx_clone = tx.clone();
    let config_dir_clone = args.config_dir.clone();
    let bot_count = args.count;
    let bot_delay = args.delay;
    let bot_listen = args.listen;
    let bot_movement = args.movement.clone();
    let producer = tokio::spawn(async move {
        for i in 0..bot_count {
            let relay_addr = relay_addresses[i % relay_addresses.len()].clone();
            let cmd = BotCommand {
                index: i,
                relay_addr,
                instance_id: instance.id as u64,
                config_dir: config_dir_clone.clone(),
                nox: nox.clone(),
                token: api_token.clone(),
                listen: bot_listen,
                movement: bot_movement.clone(),
            };

            if tx_clone.send(cmd).await.is_err() {
                error!("Failed to enqueue bot {}", i);
                break;
            }

            if i % 10 == 0 && i > 0 {
                info!("Enqueued {} / {} bots", i, bot_count);
            }

            // Delay between each bot enqueue
            tokio::time::sleep(tokio::time::Duration::from_millis(bot_delay)).await;
        }

        info!("All {} bots enqueued", bot_count);
        // Don't drop tx_clone - keep channel open
    });

    info!("Bot creation queue started. Press Ctrl+C to quit.");

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    // Signal all workers to stop
    shutdown.store(true, Ordering::Relaxed);

    // Close the channel to wake up waiting workers
    drop(tx);

    // Wait for producer to finish
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(2), producer).await;

    // Wait for all workers to complete with timeout
    for worker in worker_handles {
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), worker).await;
    }

    // Wait for all bot tasks to finish disconnecting
    info!("Waiting for all bots to disconnect...");
    let bot_tasks = {
        let mut bots = active_bots.lock().await;
        std::mem::take(&mut *bots)
    };

    let total_bots = bot_tasks.len();
    for (i, task) in bot_tasks.into_iter().enumerate() {
        if i % 10 == 0 && i > 0 {
            info!("Waiting for bots to disconnect: {}/{}", i, total_bots);
        }
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(10), task).await;
    }

    info!(
        "All {} bots have been disconnected and shut down",
        total_bots
    );
    Ok(())
}

async fn create_bot(
    index: usize,
    relay_addr: &str,
    instance_id: u64,
    config_dir: Option<PathBuf>,
    nox: Nox,
    token: Option<String>,
    listen: bool,
    movement_name: &str,
    shutdown: Arc<AtomicBool>,
    active_bots: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) -> Result<()> {
    let url = Url::parse(relay_addr).context("Invalid relay address")?;
    let host = url.host_str().context("No host in URL")?.to_string();
    let port = url.port().context("No port in URL")?;

    info!(
        "[Bot {}] Connecting via QUIC to {}:{}...",
        index, host, port
    );

    let connector = Box::new(QuicConnector::new(host, port));
    let relay = Arc::new(NoxRelay::new(connector));

    // Connect
    relay.connect().await.context("Failed to connect")?;

    // Handshake
    let mut handshake_attempt = 0;
    let handshake_response = loop {
        handshake_attempt += 1;
        match relay
            .handshake(HandshakeRequest {
                protocol: 0x0001,
                engine: "noxbot".to_string(),
                platform: get_platform().to_string(),
            })
            .await
        {
            Ok(response) => break response,
            Err(e) if handshake_attempt < 5 => {
                warn!(
                    "[Bot {}] Handshake attempt {} failed: {}",
                    index, handshake_attempt, e
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            Err(e) => return Err(e).context("Handshake failed after 5 attempts"),
        }
    };

    info!(
        "[Bot {}] Handshake successful: client_id={}, ip={}, port={}, master={:?}, keep_alive={}s",
        index,
        handshake_response.client_id,
        handshake_response.ip,
        handshake_response.port,
        handshake_response.master,
        handshake_response.keep_alive
    );

    // Démarrer le keep-alive automatique
    relay.start_keep_alive();

    // Authentification
    match noxrelay::NoxCredentials::load(config_dir.clone()) {
        Ok(credentials) => {
            match relay.authenticate(&credentials).await {
                Ok(auth_result) => {
                    info!(
                        "[Bot {}] Authentication successful: {:?}",
                        index, auth_result
                    );
                }
                Err(e) => {
                    warn!("[Bot {}] Authentication failed: {}", index, e);
                    // Continue sans authentification
                }
            }
        }
        Err(e) => {
            warn!(
                "[Bot {}] Failed to load credentials: {} - continuing without authentication",
                index, e
            );
        }
    }

    // Get sessions
    let sessions = relay
        .sessions(SessionRequest { page: 0 })
        .await
        .context("Failed to get sessions")?;

    // Find the target instance
    let relay_instance_info = sessions
        .instances
        .iter()
        .find(|inst| inst.master == instance_id)
        .context("Instance not found in sessions")?;

    let instance = RelayInstance::new(relay_instance_info.clone(), relay.clone());

    // Enter instance
    let enter_response = instance
        .enter(EnterRequest {
            instance_id: relay_instance_info.id,
            display: format!("RustBot-{}", index),
            flags: EnterFlags::AS_BOT,
            password: None,
        })
        .await
        .context("Failed to enter instance")?;

    // Extract enter response fields
    let (bot_player_id, initial_tps) = match enter_response {
        noxrelay::EnterResponse::Success { player_id, entity_id, tps, property_resend_interval } => {
            info!(
                "[Bot {}] Entered instance: player_id={}, entity_id={}, tps={}, property_resend_interval={}",
                index, player_id, entity_id, tps, property_resend_interval
            );
            (player_id, tps as u64)
        }
        noxrelay::EnterResponse::Error { code, reason } => {
            warn!("[Bot {}] Enter failed: code={}, reason={}", index, code, reason);
            return Err(anyhow::anyhow!("Enter failed: {}", reason));
        }
    };

    // Fetch user avatar from API in parallel while we do traveling
    let avatar_future = {
        let nox = nox.clone();
        let token = token.clone();
        let index = index;
        async move {
            if let Some(tok) = token {
                match nox.get_me(&tok).await.data {
                    Some(user) => {
                        info!("[Bot {}] User profile: {} ({})", index, user.display_name, user.username);
                        user.avatar
                    }
                    None => {
                        warn!("[Bot {}] Failed to fetch user profile via @me", index);
                        None
                    }
                }
            } else {
                None
            }
        }
    };

    // Traveling
    instance
        .traveling(TravelingRequest {
            action: TravelingAction::Travel,
            reason: None,
        })
        .await
        .context("Failed to travel")?;

    instance
        .traveling(TravelingRequest {
            action: TravelingAction::Ready,
            reason: None,
        })
        .await
        .context("Failed to ready")?;

    // Now send avatar change (player is valid after Ready)
    let user_avatar = avatar_future.await;
    if let Some(ref avatar_str) = user_avatar {
        let (avatar_id_str, avatar_srv) = match avatar_str.split_once('@') {
            Some((id, srv)) => (id, srv.to_string()),
            None => (avatar_str.as_str(), "hactazia.fr".to_string()),
        };
        if let Ok(avatar_id) = avatar_id_str.parse::<u32>() {
            match instance
                .change_avatar(AvatarChangeRequest::new(
                    bot_player_id,
                    avatar_id,
                    avatar_srv.clone(),
                ))
                .await
            {
                Ok(_) => info!("[Bot {}] Avatar set to {}@{}", index, avatar_id, avatar_srv),
                Err(e) => warn!("[Bot {}] Failed to set avatar: {}", index, e),
            }
        } else {
            warn!("[Bot {}] Could not parse avatar id from '{}'", index, avatar_str);
        }
    } else {
        info!("[Bot {}] User has no default avatar set", index);
    }

    // Select movement
    let movement = movements::get_movement(movement_name);
    info!("[Bot {}] Movement: {}", index, movement.name());
    let mut movement_state = movement.initialize(index);
    movement_state.player_id = bot_player_id;

    // Create channel for TPS updates
    let (tps_tx, mut tps_rx) = tokio::sync::mpsc::unbounded_channel();
    instance.set_tps_change_listener(tps_tx).await;

    // Set up ServerConfig broadcast listener
    let instance_clone = instance.clone();
    relay
        .set_server_config_callback(move |config| {
            let instance = instance_clone.clone();
            tokio::spawn(async move {
                if let Some(new_tps) = config.tps {
                    let changed = instance.update_tps_from_broadcast(new_tps).await;
                    if changed {
                        info!(
                            "[Bot] TPS updated via broadcast: {} | Load balancing: {}",
                            new_tps,
                            if config.load_balancing_enabled.unwrap_or(false) {
                                format!(
                                    "enabled (min={}, max={})",
                                    config.min_tps.unwrap_or(5),
                                    config.max_tps.unwrap_or(20)
                                )
                            } else {
                                "disabled".to_string()
                            }
                        );
                    }
                }
                if let Some(new_threshold) = config.threshold {
                    instance.update_threshold_from_broadcast(new_threshold).await;
                }
            });
        })
        .await;

    // Start listening for server broadcasts
    relay.start_datagram_listener();

    // Always start the push listener to accept incoming uni-stream packets from the server.
    // This is required so the connection doesn't stall.
    relay.start_push_listener();

    // If --listen is active, register a callback that prints every received event as JSON
    if listen {
        let bot_index = index;
        relay.set_event_callback(move |event| {
            match serde_json::to_string(&event) {
                Ok(json) => info!("[Bot {}] RX {}", bot_index, json),
                Err(e) => warn!("[Bot {}] Failed to serialize event: {}", bot_index, e),
            }
        }).await;
    }

    let bot_task = tokio::spawn(async move {
        let mut tps = initial_tps;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1000 / tps));

        loop {
            tokio::select! {
                // Check for TPS updates from broadcasts
                Some(new_tps) = tps_rx.recv() => {
                    if new_tps as u64 != tps {
                        tps = new_tps as u64;
                        // Recreate interval with new TPS
                        interval = tokio::time::interval(tokio::time::Duration::from_millis(1000 / tps));
                    }
                }

                // Normal tick
                _ = interval.tick() => {
                    // Check shutdown flag
                    if shutdown.load(Ordering::Relaxed) {
                        info!("[Bot {}] Shutting down gracefully...", index);
                        // Déconnexion propre avant fermeture
                        if let Err(e) = relay
                            .disconnect(Some("Shutdown requested".to_string()))
                            .await
                        {
                            warn!("[Bot {}] Disconnect error: {}", index, e);
                        }
                        let _ = relay.close().await;
                        break;
                    }

                    // Vérifier si le relay est toujours connecté
                    if !relay.is_connected() {
                        warn!("[Bot {}] Relay disconnected, stopping movement loop", index);
                        break;
                    }

                    let dt = 1000.0 / tps as f32;
                    movement.update(&mut movement_state, dt, &instance).await;
                }
            }
        }
    });

    // Register the bot task for graceful shutdown
    active_bots.lock().await.push(bot_task);

    info!(
        "[Bot {}] Bot creation complete, worker can process next bot",
        index
    );
    Ok(())
}
