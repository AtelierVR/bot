use anyhow::{Context, Result};
use clap::Parser;
use nox_api::Nox;
use nox_relay::{
    AvatarChangeRequest, EnterFlags, EnterRequest, HandshakeRequest, NoxRelay, QuicConnector,
    RelayInstance, SessionRequest, TcpConnector, TravelingAction, TravelingRequest, UdpConnector,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use url::Url;

mod movements;

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
}

fn get_env_or_default(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("noxbot=debug".parse().unwrap())
                .add_directive("nox_relay=debug".parse().unwrap())
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
            nox_relay::NoxCredentials::nox_folder(None)
        );
    }

    // Create API client
    let api_url = format!("https://nox.{}/", server);
    info!("Connecting to API: {}", api_url);
    let nox = Nox::new(&api_url);

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

    // Determine desired protocol via BOT_PROTOCOL env var (default: tcp)
    let bot_protocol = std::env::var("BOT_PROTOCOL")
        .unwrap_or_else(|_| "tcp".to_string())
        .to_lowercase();
    let scheme = match bot_protocol.as_str() {
        "tcp" | "udp" | "quic" => bot_protocol.clone(),
        other => {
            error!(
                "Unsupported BOT_PROTOCOL '{}'. Use tcp, udp, or quic.",
                other
            );
            return Ok(());
        }
    };

    let relay_addresses: Vec<String> = addresses
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| s.starts_with(&format!("{}://", scheme)))
        .map(|s| s.to_string())
        .collect();

    if relay_addresses.is_empty() {
        error!("No {} relay addresses found", scheme.to_uppercase());
        return Ok(());
    }

    info!(
        "Using {} relay addresses ({})",
        scheme.to_uppercase(),
        relay_addresses.len()
    );

    // Bot creation parameters
    let bot_count = get_env_or_default("BOT_COUNT", 64);
    let concurrent_workers = get_env_or_default("CONCURRENT_BOTS", 10);
    let bot_delay_ms = get_env_or_default("BOT_DELAY_MS", 100);

    info!(
        "Creating {} bots with {} concurrent workers ({}ms delay between spawns)...",
        bot_count, concurrent_workers, bot_delay_ms
    );

    let shutdown = Arc::new(AtomicBool::new(false));

    // Track active bot tasks for graceful shutdown
    let active_bots = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Create a queue for bot creation commands
    struct BotCommand {
        index: usize,
        relay_addr: String,
        instance_id: u64,
        config_dir: Option<PathBuf>,
    }

    let (tx, rx) = mpsc::channel::<BotCommand>(bot_count);
    let rx = Arc::new(tokio::sync::Mutex::new(rx));

    // Spawn workers that consume from the queue
    let mut worker_handles = Vec::new();
    for worker_id in 0..concurrent_workers {
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
    let producer = tokio::spawn(async move {
        for i in 0..bot_count {
            let relay_addr = relay_addresses[i % relay_addresses.len()].clone();
            let cmd = BotCommand {
                index: i,
                relay_addr,
                instance_id: instance.id as u64,
                config_dir: config_dir_clone.clone(),
            };

            if tx_clone.send(cmd).await.is_err() {
                error!("Failed to enqueue bot {}", i);
                break;
            }

            if i % 10 == 0 && i > 0 {
                info!("Enqueued {} / {} bots", i, bot_count);
            }

            // Delay between each bot enqueue
            tokio::time::sleep(tokio::time::Duration::from_millis(bot_delay_ms as u64)).await;
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
    shutdown: Arc<AtomicBool>,
    active_bots: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) -> Result<()> {
    let url = Url::parse(relay_addr).context("Invalid relay address")?;
    let host = url.host_str().context("No host in URL")?.to_string();
    let port = url.port().context("No port in URL")?;
    let scheme = url.scheme();

    info!(
        "[Bot {}] Connecting via {} to {}:{}...",
        index,
        scheme.to_uppercase(),
        host,
        port
    );

    let connector: Box<dyn nox_relay::Connector> = match scheme {
        "tcp" => Box::new(TcpConnector::new(host, port)),
        "udp" => Box::new(UdpConnector::new(host, port)),
        "quic" => Box::new(QuicConnector::new(host, port)),
        other => anyhow::bail!("Unsupported relay scheme: {}", other),
    };
    let relay = Arc::new(NoxRelay::new(connector));

    // Connect
    relay.connect().await.context("Failed to connect")?;
    info!("[Bot {}] Connected", index);

    // Handshake
    let mut handshake_attempt = 0;
    let handshake_response = loop {
        handshake_attempt += 1;
        match relay
            .handshake(HandshakeRequest {
                protocol: 0x0001,
                engine: "rust".to_string(),
                platform: "windows".to_string(),
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
    info!("[Bot {}] Keep-alive started", index);

    // Authentification
    match nox_relay::NoxCredentials::load(config_dir) {
        Ok(credentials) => {
            info!(
                "[Bot {}] Credentials loaded: user_id={} server={}",
                index, credentials.user_id, credentials.server
            );
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

    info!(
        "[Bot {}] Found {} instances",
        index,
        sessions.instances.len()
    );

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

    info!("[Bot {}] Entered instance: {:?}", index, enter_response);

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

    info!("[Bot {}] Traveling complete", index);

    // Change avatar and extract player_id
    let bot_player_id = if let nox_relay::EnterResponse::Success { player_id, .. } = enter_response
    {
        instance
            .change_avatar(AvatarChangeRequest {
                player_id,
                avatar_id: 1,
                avatar_server: "hactazia.fr".to_string(),
            })
            .await
            .context("Failed to change avatar")?;
        player_id
    } else {
        0
    };

    // Select random movement
    let movement = movements::get_random_movement();
    let mut movement_state = movement.initialize(index);
    movement_state.player_id = bot_player_id;
    info!(
        "[Bot {}] Using movement: {} (player_id={})",
        index,
        movement.name(),
        bot_player_id
    );

    // Get tps for movement
    let tps = if let nox_relay::EnterResponse::Success { tps, .. } = enter_response {
        tps as u64
    } else {
        20
    };

    // Spawn movement loop as independent task so worker can handle next bot
    info!("[Bot {}] Spawning movement loop task", index);
    let bot_task = tokio::spawn(async move {
        let dt = 1000.0 / tps as f32;
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1000 / tps));
        let mut tick_count = 0u64;

        loop {
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

            interval.tick().await;
            tick_count += 1;

            // Afficher le dernier ping toutes les 5 secondes environ (dépend du TPS)
            if tick_count.is_multiple_of(tps * 5) {
                if let Some(ping) = relay.get_last_ping().await {
                    debug!(
                        "[Bot {}] Latency: up={}ms, down={}ms, total={}ms",
                        index,
                        ping.up(),
                        ping.down(),
                        ping.total()
                    );
                }
            }

            movement.update(&mut movement_state, dt, &instance).await;
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
