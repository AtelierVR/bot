use crate::auth::{
    create_challenge_request, create_resolve_request, parse_auth_response, AuthResponse,
    Credentials,
};
use crate::buffer::Buffer;
use crate::connector::Connector;
use crate::protocol::{RequestType, ResponseType};
use crate::types::*;
use anyhow::{anyhow, Result};
use serde_json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

/// Callback invoked on a ServerConfig broadcast.
type ServerConfigCallback = Arc<dyn Fn(ServerConfigResponse) + Send + Sync>;
/// Callback invoked for every relay event.
type EventCallback = Arc<dyn Fn(RelayEvent) + Send + Sync>;

pub struct NoxRelay {
    connector: Arc<RwLock<Box<dyn Connector>>>,
    #[allow(dead_code)]
    event_tx: mpsc::UnboundedSender<RelayEvent>,
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<RelayEvent>>>,
    #[allow(dead_code)]
    pending_responses: Arc<RwLock<HashMap<u16, tokio::sync::oneshot::Sender<Vec<u8>>>>>,
    counter: Arc<Mutex<u16>>,
    running: Arc<AtomicBool>,
    keep_alive_interval: Arc<Mutex<u64>>,
    last_ping: Arc<Mutex<Option<LatencyResponse>>>,
    /// Callback for ServerConfig broadcasts
    server_config_callback: Arc<RwLock<Option<ServerConfigCallback>>>,
    /// Generic callback for all relay events (Join, Leave, PlayerUpdate, etc.)
    event_callback: Arc<RwLock<Option<EventCallback>>>,
}

impl NoxRelay {
    pub fn new(connector: Box<dyn Connector>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            connector: Arc::new(RwLock::new(connector)),
            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
            pending_responses: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            keep_alive_interval: Arc::new(Mutex::new(5000)),
            last_ping: Arc::new(Mutex::new(None)),
            server_config_callback: Arc::new(RwLock::new(None)),
            event_callback: Arc::new(RwLock::new(None)),
        }
    }

    /// Set a callback that will be called when ServerConfig broadcast is received
    pub async fn set_server_config_callback<F>(&self, callback: F)
    where
        F: Fn(ServerConfigResponse) + Send + Sync + 'static,
    {
        *self.server_config_callback.write().await = Some(Arc::new(callback));
    }

    /// Set a callback that will be called for every relay event received from the server
    /// (Join, Leave, PlayerUpdate, Transform, Properties, AvatarChanged, Custom Event, etc.)
    pub async fn set_event_callback<F>(&self, callback: F)
    where
        F: Fn(RelayEvent) + Send + Sync + 'static,
    {
        *self.event_callback.write().await = Some(Arc::new(callback));
    }

    /// Démarre l'écoute des packets push (uni-stream, fiable) envoyés par le serveur
    pub fn start_push_listener(self: &Arc<Self>) {
        let relay = Arc::clone(self);
        tokio::spawn(async move {
            relay.push_listener_loop().await;
        });
    }

    pub async fn connect(&self) -> Result<()> {
        let mut connector = self.connector.write().await;
        connector.connect().await?;
        drop(connector);

        self.running.store(true, Ordering::SeqCst);

        Ok(())
    }

    /// Démarre le keep-alive automatique après le handshake
    pub fn start_keep_alive(self: &Arc<Self>) {
        let relay = Arc::clone(self);
        tokio::spawn(async move {
            relay.keep_alive_loop().await;
        });
    }

    /// Démarre l'écoute des broadcasts (datagrams) du serveur
    pub fn start_datagram_listener(self: &Arc<Self>) {
        let relay = Arc::clone(self);
        tokio::spawn(async move {
            relay.datagram_listener_loop().await;
        });
    }

    async fn datagram_listener_loop(&self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            let connector = self.connector.read().await;
            let quic = match connector
                .as_any()
                .downcast_ref::<crate::connector::QuicConnector>()
            {
                Some(q) => q,
                None => break,
            };

            match tokio::time::timeout(Duration::from_millis(100), quic.recv_datagram()).await {
                Ok(Ok(data)) => {
                    drop(connector);
                    if let Err(e) = self.process_datagram(&data).await {
                        warn!("Failed to process datagram: {}", e);
                    }
                }
                Ok(Err(_e)) => {
                    drop(connector);
                    break;
                }
                Err(_timeout) => {
                    drop(connector);
                }
            }
        }
    }

    async fn process_datagram(&self, data: &[u8]) -> Result<()> {
        if data.len() < 5 {
            return Err(anyhow!("Datagram too short"));
        }

        let mut buf = Buffer::from_vec(data.to_vec());
        let _length = buf.read_u16()?; // inclusive total length (same as stream)
        let _uid = buf.read_u16()?;
        let type_byte = buf.read_u8()?;

        // Check if it's a ServerConfig packet
        if type_byte == ResponseType::ServerConfig as u8 {
            // Parse ServerConfig response
            let iid = buf.read_u8()?;
            let result = buf.read_u8()?;
            let flags_byte = buf.read_u8()?;
            let flags = ServerConfigFlags::from_bits_truncate(flags_byte);

            let result_enum = match result {
                0 => ServerConfigResult::Success,
                1 => ServerConfigResult::Failure,
                2 => ServerConfigResult::Change,
                _ => ServerConfigResult::Failure,
            };

            let mut tps = None;
            let mut threshold = None;
            let mut capacity = None;
            let mut has_password = None;
            let mut instance_flags = None;
            let mut min_tps = None;
            let mut max_tps = None;
            let mut load_balancing_enabled = None;

            if flags.contains(ServerConfigFlags::TPS) {
                tps = Some(buf.read_u8()?);
            }
            if flags.contains(ServerConfigFlags::THRESHOLD) {
                threshold = Some(buf.read_f32()?);
            }
            if flags.contains(ServerConfigFlags::CAPACITY) {
                capacity = Some(buf.read_u16()?);
            }
            if flags.contains(ServerConfigFlags::FLAGS) {
                instance_flags = Some(buf.read_u32()?);
            }
            if flags.contains(ServerConfigFlags::PASSWORD) {
                has_password = Some(buf.read_u8()? != 0);
            }
            if flags.contains(ServerConfigFlags::MIN_TPS) {
                min_tps = Some(buf.read_u8()?);
            }
            if flags.contains(ServerConfigFlags::MAX_TPS) {
                max_tps = Some(buf.read_u8()?);
            }
            if flags.contains(ServerConfigFlags::LOAD_BALANCING) {
                load_balancing_enabled = Some(buf.read_u8()? != 0);
            }

            let response = ServerConfigResponse {
                instance_id: iid,
                result: result_enum,
                tps,
                threshold,
                capacity,
                has_password,
                instance_flags,
                min_tps,
                max_tps,
                load_balancing_enabled,
            };

            // Call the callback if set
            if let Some(callback) = self.server_config_callback.read().await.as_ref() {
                callback(response);
            }
        }

        Ok(())
    }

    async fn push_listener_loop(&self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            let connector = self.connector.read().await;
            let quic = match connector
                .as_any()
                .downcast_ref::<crate::connector::QuicConnector>()
            {
                Some(q) => q,
                None => break,
            };

            match tokio::time::timeout(Duration::from_millis(200), quic.accept_uni_packet()).await {
                Ok(Ok((type_byte, payload))) => {
                    drop(connector);
                    if let Err(e) = self.process_push_packet(type_byte, &payload).await {
                        warn!(
                            "Failed to process push packet (type=0x{:02X}): {}",
                            type_byte, e
                        );
                    }
                }
                Ok(Err(_e)) => {
                    drop(connector);
                    // Connection closed or stream error — stop loop
                    break;
                }
                Err(_timeout) => {
                    drop(connector);
                    // No packet within timeout window — continue
                }
            }
        }
    }

    async fn process_push_packet(&self, type_byte: u8, payload: &[u8]) -> Result<()> {
        use crate::protocol::ResponseType;

        let event = match type_byte {
            // Join (0x10): [iid:u8][player_flags:u32][player_id:u16][user_id:u32]
            //              [user_address:string][display:string][created_at:i64]
            //              [engine:string][platform:string]
            t if t == ResponseType::Join as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let _player_flags = buf.read_u32()?;
                let player_id = buf.read_u16()?;
                let _user_id = buf.read_u32()?;
                let _user_address = buf.read_string().unwrap_or_default();
                let display = buf.read_string().unwrap_or_default();
                RelayEvent::Join(JoinEvent { player_id, display })
            }

            // Leave (0x11): [iid:u8][quit_type:u8][player_id:u16]
            t if t == ResponseType::Leave as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let _quit_type = buf.read_u8()?;
                let player_id = buf.read_u16()?;
                RelayEvent::Leave(LeaveEvent { player_id })
            }

            // PlayerUpdate (0x12): [iid:u8][result:u8][player_id:u16][flags:u8]
            //                       [?display:string][?player_flags:u32]
            t if t == ResponseType::PlayerUpdate as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let _result = buf.read_u8()?;
                let player_id = buf.read_u16()?;
                let remaining = buf.remaining();
                let data = buf.read_bytes(remaining)?;
                RelayEvent::PlayerUpdate(PlayerUpdateEvent { player_id, data })
            }

            // ServerConfig (0x0E): already handled via callback, but also emit event
            t if t == ResponseType::ServerConfig as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let iid = buf.read_u8()?;
                let result = buf.read_u8()?;
                let flags_byte = buf.read_u8()?;
                let flags = ServerConfigFlags::from_bits_truncate(flags_byte);
                let result_enum = match result {
                    0 => ServerConfigResult::Success,
                    1 => ServerConfigResult::Failure,
                    _ => ServerConfigResult::Change,
                };
                let mut tps = None;
                let mut threshold = None;
                let mut capacity = None;
                let mut has_password = None;
                let mut instance_flags = None;
                let mut min_tps = None;
                let mut max_tps = None;
                let mut load_balancing_enabled = None;
                if flags.contains(ServerConfigFlags::TPS) {
                    tps = Some(buf.read_u8()?);
                }
                if flags.contains(ServerConfigFlags::THRESHOLD) {
                    threshold = Some(buf.read_f32()?);
                }
                if flags.contains(ServerConfigFlags::CAPACITY) {
                    capacity = Some(buf.read_u16()?);
                }
                if flags.contains(ServerConfigFlags::FLAGS) {
                    instance_flags = Some(buf.read_u32()?);
                }
                if flags.contains(ServerConfigFlags::PASSWORD) {
                    has_password = Some(buf.read_u8()? != 0);
                }
                if flags.contains(ServerConfigFlags::MIN_TPS) {
                    min_tps = Some(buf.read_u8()?);
                }
                if flags.contains(ServerConfigFlags::MAX_TPS) {
                    max_tps = Some(buf.read_u8()?);
                }
                if flags.contains(ServerConfigFlags::LOAD_BALANCING) {
                    load_balancing_enabled = Some(buf.read_u8()? != 0);
                }
                let response = ServerConfigResponse {
                    instance_id: iid,
                    result: result_enum,
                    tps,
                    threshold,
                    capacity,
                    has_password,
                    instance_flags,
                    min_tps,
                    max_tps,
                    load_balancing_enabled,
                };
                // Also trigger the existing server_config_callback
                if let Some(cb) = self.server_config_callback.read().await.as_ref() {
                    cb(response.clone());
                }
                // Emit as generic event so the --listen listener can see it
                if let Some(cb) = self.event_callback.read().await.as_ref() {
                    // Wrap in a custom event string for display
                    let json = serde_json::to_string(&response).unwrap_or_default();
                    cb(RelayEvent::Event(CustomEvent {
                        name: crc64("ServerConfig"),
                        player_id: 0,
                        data: json.into_bytes(),
                    }));
                }
                return Ok(());
            }

            // Transform (0x0B): handled separately — emit a simplified event
            t if t == ResponseType::Transform as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let sub_type = buf.read_u8()?;
                if sub_type == 1 {
                    // EntityPart: [pid:u16][rig:u16][flags:u8][...data...][broadcaster_pid:u16]
                    let entity_id = buf.read_u16()?;
                    let _rig_id = buf.read_u16()?;
                    let flags_byte = buf.read_u8()?;
                    let flags = TransformFlags::from_bits_truncate(flags_byte);
                    let position = if flags.contains(TransformFlags::POSITION) {
                        Some(Vector3 {
                            x: buf.read_f32()?,
                            y: buf.read_f32()?,
                            z: buf.read_f32()?,
                        })
                    } else {
                        None
                    };
                    let rotation = if flags.contains(TransformFlags::ROTATION) {
                        Some(Quaternion {
                            x: buf.read_f32()?,
                            y: buf.read_f32()?,
                            z: buf.read_f32()?,
                            w: buf.read_f32()?,
                        })
                    } else {
                        None
                    };
                    let scale = if flags.contains(TransformFlags::SCALE) {
                        Some(Vector3 {
                            x: buf.read_f32()?,
                            y: buf.read_f32()?,
                            z: buf.read_f32()?,
                        })
                    } else {
                        None
                    };
                    RelayEvent::Transform(TransformEvent {
                        entity_id,
                        transform: Transform {
                            position,
                            rotation,
                            scale,
                        },
                    })
                } else {
                    // ByPath or unknown sub-type — skip
                    return Ok(());
                }
            }

            // AvatarChanged broadcast (0x0D)
            t if t == ResponseType::AvatarChanged as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let player_id = buf.read_u16()?;
                let avatar_id = buf.read_u32()? as u64;
                let avatar_server = buf.read_string().unwrap_or_default();
                RelayEvent::AvatarChanged(AvatarChangedEvent {
                    player_id,
                    avatar_id,
                    avatar_server,
                })
            }

            // Custom Event (0x15): [iid:u8][name_hash:u64][data_len:u16][data][targets...]
            t if t == ResponseType::Event as u8 => {
                // Minimum: iid(1) + name_hash(8) + data_len(2) = 11 bytes
                if payload.len() < 11 {
                    return Ok(());
                }
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let name = buf.read_u64()?;
                let data_len = buf.read_u16()? as usize;
                // Guard against bogus data_len exceeding remaining bytes
                if buf.remaining() < data_len {
                    return Ok(());
                }
                let data = buf.read_bytes(data_len)?;
                RelayEvent::Event(CustomEvent {
                    name,
                    player_id: 0,
                    data,
                })
            }

            // Stream / voice (0x14): broadcast Opus samples and hearing control.
            // Sample: [iid:u8][0x00][player_id:u16][channel_id:u32][level_flags:u8]
            //         [?group_id:u16][frame_index:i32][timestamp:f64][sample:bytes]
            // Control:[iid:u8][0x01][listener_id:u16][speaker_id:u16][control_flags:u8]
            t if t == ResponseType::Stream as u8 => {
                let mut buf = Buffer::from_vec(payload.to_vec());
                let _iid = buf.read_u8()?;
                let sub_type = buf.read_u8()?;

                match sub_type {
                    // Sample (Opus audio)
                    0x00 => {
                        let player_id = buf.read_u16()?;
                        let channel_id = buf.read_u32()?;
                        let level_flags = buf.read_u8()?;
                        // Bit 2 (0x04) = HasGroup
                        let group_id = if level_flags & 0x04 != 0 {
                            Some(buf.read_u16()?)
                        } else {
                            None
                        };
                        let frame_index = buf.read_i32()?;
                        let timestamp = buf.read_f64()?;
                        let sample = buf.read_bytes(buf.remaining())?;
                        RelayEvent::Stream(StreamEvent {
                            sub_type,
                            player_id,
                            channel_id,
                            level_flags,
                            group_id,
                            frame_index,
                            timestamp,
                            sample,
                            listener_id: 0,
                            speaker_id: 0,
                            control_flags: 0,
                        })
                    }
                    // Control (hearing permission)
                    0x01 => {
                        let listener_id = buf.read_u16()?;
                        let speaker_id = buf.read_u16()?;
                        let control_flags = buf.read_u8()?;
                        RelayEvent::Stream(StreamEvent {
                            sub_type,
                            player_id: 0,
                            channel_id: 0,
                            level_flags: 0,
                            group_id: None,
                            frame_index: 0,
                            timestamp: 0.0,
                            sample: Vec::new(),
                            listener_id,
                            speaker_id,
                            control_flags,
                        })
                    }
                    _ => return Ok(()),
                }
            }

            // Unknown or unhandled packet type — ignore
            _ => return Ok(()),
        };

        // Dispatch to the event callback if registered
        if let Some(cb) = self.event_callback.read().await.as_ref() {
            cb(event);
        }

        Ok(())
    }

    async fn keep_alive_loop(&self) {
        loop {
            if !self.running.load(Ordering::SeqCst) {
                debug!("Keep-alive loop stopping: relay not running");
                break;
            }

            let interval = *self.keep_alive_interval.lock().await;
            tokio::time::sleep(Duration::from_millis(interval)).await;

            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            match self.latency().await {
                Ok(response) => {
                    *self.last_ping.lock().await = Some(response.clone());
                    debug!(
                        "Ping: up={}ms down={}ms total={}ms",
                        response.up(),
                        response.down(),
                        response.total()
                    );
                }
                Err(e) => {
                    warn!("Latency check failed: {} - possible disconnection", e);
                    self.running.store(false, Ordering::SeqCst);

                    break;
                }
            }
        }
        debug!("Keep-alive loop ended");
    }

    pub async fn get_last_ping(&self) -> Option<LatencyResponse> {
        self.last_ping.lock().await.clone()
    }

    async fn next_state(&self) -> u16 {
        let mut counter = self.counter.lock().await;
        let state = *counter;
        *counter = counter.wrapping_add(1);
        if *counter == 0 {
            *counter = 1;
        }
        state
    }

    async fn request_with_response(
        &self,
        request_type: RequestType,
        data: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>> {
        let state = self.next_state().await;

        // Build the full packet: [length: u16][state: u16][type: u8][data...]
        let length = 5 + data.len();
        let mut buffer = Buffer::with_capacity(length);
        buffer.write_u16(length as u16);
        buffer.write_u16(state);
        buffer.write_u8(request_type as u8);
        buffer.write_bytes(data);

        let packet = buffer.as_slice();
        // debug!("Sending request: {:?}, state: {}, packet_len: {}",
        //        request_type, state, packet.len());

        // Use the QUIC connector's send_and_receive which opens a new stream
        let connector = self.connector.read().await;

        // Downcast to QuicConnector to use send_and_receive
        let response_data = if let Some(quic) = connector
            .as_any()
            .downcast_ref::<crate::connector::QuicConnector>()
        {
            match timeout(
                Duration::from_millis(timeout_ms),
                quic.send_and_receive(packet),
            )
            .await
            {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(e)) => Err(anyhow!("Send/receive error: {}", e)),
                Err(_) => Err(anyhow!("Request timeout after {}ms", timeout_ms)),
            }
        } else {
            Err(anyhow!("Connector is not a QuicConnector"))
        }?;

        // Parse response packet: [length: u16][state: u16][type: u8][payload...]
        if response_data.len() < 5 {
            return Err(anyhow!("Response too short: {} bytes", response_data.len()));
        }

        let mut buf = Buffer::from_vec(response_data);
        let _length = buf.read_u16()?;
        let resp_state = buf.read_u16()?;
        let _type_byte = buf.read_u8()?;

        if resp_state != state {
            warn!("State mismatch: sent {}, received {}", state, resp_state);
        }

        // Remaining data is the actual payload
        let payload_len = buf.remaining();
        let payload = buf.read_bytes(payload_len)?;

        // debug!("Received response for state {}: {} bytes payload", state, payload.len());
        Ok(payload)
    }

    pub async fn handshake(&self, request: HandshakeRequest) -> Result<HandshakeResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u16(request.protocol);
        buffer.write_string(&request.engine);
        buffer.write_string(&request.platform);

        let response = self
            .request_with_response(RequestType::Handshake, buffer.as_slice(), 10000)
            .await?;
        let mut buf = Buffer::from_vec(response);

        let protocol = buf.read_u16()?;
        let client_id = buf.read_u16()?;

        // Read IP as 4 bytes
        let ip_bytes = buf.read_bytes(4)?;
        let ip = format!(
            "{}.{}.{}.{}",
            ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]
        );

        let port = buf.read_u16()?;
        let flags_byte = buf.read_u8()?;

        // Parse flags (TODO: implement proper flag parsing)
        let is_offline = (flags_byte & 0x01) != 0;
        let flags = if is_offline {
            vec!["is_offline".to_string()]
        } else {
            vec![]
        };

        // Master server string (only if not offline)
        let master = if !is_offline {
            Some(buf.read_string()?)
        } else {
            None
        };

        let max_packet_size = buf.read_u16()?;
        let timeout = buf.read_u16()?; // in seconds
        let keep_alive = buf.read_u16()?; // in seconds

        // Update keep-alive interval (convert from seconds to milliseconds)
        *self.keep_alive_interval.lock().await = (keep_alive as u64) * 1000;

        Ok(HandshakeResponse {
            protocol,
            client_id,
            ip,
            port,
            flags,
            master,
            max_packet_size,
            timeout,
            keep_alive,
        })
    }

    /// Effectue l'authentification complète avec challenge/response
    pub async fn authenticate(&self, credentials: &Credentials) -> Result<AuthResponse> {
        // Étape 1: Demander un challenge
        let challenge_req = create_challenge_request();
        let challenge_resp_data = self
            .request_with_response(RequestType::Authentication, &challenge_req, 5000)
            .await?;

        let challenge_resp = parse_auth_response(&challenge_resp_data).map_err(|e| anyhow!(e))?;

        let challenge = match challenge_resp {
            AuthResponse::Challenge { challenge } => challenge,
            AuthResponse::Error { result, reason, .. } => {
                return Err(anyhow!(
                    "Challenge request failed: {:?} - {}",
                    result,
                    reason
                ));
            }
            _ => {
                return Err(anyhow!("Unexpected response to challenge request"));
            }
        };

        // Étape 2: Signer le challenge
        let signature = credentials.sign(&challenge).map_err(|e| anyhow!(e))?;

        // Étape 3: Envoyer la résolution du challenge
        let resolve_req =
            create_resolve_request(credentials, &signature).map_err(|e| anyhow!(e))?;
        let resolve_resp_data = self
            .request_with_response(RequestType::Authentication, &resolve_req, 10000)
            .await?;

        let resolve_resp = parse_auth_response(&resolve_resp_data).map_err(|e| anyhow!(e))?;

        match &resolve_resp {
            AuthResponse::Success {
                user_id: _,
                address: _,
                display_name: _,
            } => {}
            AuthResponse::Error { result, reason, .. } => {
                warn!("Authentication failed: {:?} - {}", result, reason);
            }
            _ => {
                return Err(anyhow!("Unexpected response to resolve request"));
            }
        }

        Ok(resolve_resp)
    }

    pub async fn latency(&self) -> Result<LatencyResponse> {
        let initial = chrono::Utc::now().timestamp_millis();

        let mut buffer = Buffer::new();
        buffer.write_u64(initial as u64);
        buffer.write_u64(0); // client_long (unused)

        let response = self
            .request_with_response(RequestType::Latency, buffer.as_slice(), 5000)
            .await?;
        let mut buf = Buffer::from_vec(response);

        let _initial_resp = buf.read_i64()?; // Echo of client timestamp (validation)
        let server_time = buf.read_i64()?; // Server timestamp (for reference only)
        let _client_long = buf.read_i64()?; // Echo of client_long
        let final_time = chrono::Utc::now().timestamp_millis();

        // Calculate RTT (Round Trip Time) using only client timestamps
        let rtt = final_time - initial;

        Ok(LatencyResponse {
            initial,
            server_time,
            final_time,
            rtt,
        })
    }

    pub async fn sessions(&self, request: SessionRequest) -> Result<SessionResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u8(request.page as u8); // page is u8

        let response = self
            .request_with_response(RequestType::Sessions, buffer.as_slice(), 10000)
            .await?;
        let mut buf = Buffer::from_vec(response);

        let count = buf.read_u8()? as usize; // count is 1 byte
        let mut instances = Vec::with_capacity(count);

        for _ in 0..count {
            let _flags = buf.read_u32()?; // flags: 4 bytes
            let id = buf.read_u8()? as u64; // iid: 1 byte
            let master = buf.read_u32()? as u64; // master: 4 bytes
            let player_count = buf.read_u16()?; // playerCount: 2 bytes
            let capacity = buf.read_u16()?; // capacity: 2 bytes

            instances.push(RelayInstanceInfo {
                id,
                master,
                name: format!("Instance {}", id), // No name in protocol
                capacity: capacity as u32,
                client_count: player_count as u32,
            });
        }

        let current_page = buf.read_u8()?; // current page: 1 byte
        let total_pages = buf.read_u8()?; // total pages: 1 byte

        Ok(SessionResponse {
            instances,
            current_page,
            total_pages,
        })
    }

    /// Envoie une requête de déconnexion propre au serveur relay
    pub async fn disconnect(&self, reason: Option<String>) -> Result<DisconnectResponse> {
        let mut buffer = Buffer::new();
        if let Some(ref r) = reason {
            if !r.is_empty() {
                buffer.write_string(r);
            }
        }

        let response = self
            .request_with_response(RequestType::Disconnect, buffer.as_slice(), 5000)
            .await?;

        let mut buf = Buffer::from_vec(response);
        let response_reason = if buf.remaining() > 2 {
            Some(buf.read_string()?)
        } else {
            None
        };

        Ok(DisconnectResponse {
            reason: response_reason,
        })
    }

    pub async fn close(&self) -> Result<()> {
        info!("Closing relay connection...");
        self.running.store(false, Ordering::SeqCst);
        let mut connector = self.connector.write().await;
        connector.close().await?;

        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn get_event_receiver(&self) -> Arc<Mutex<mpsc::UnboundedReceiver<RelayEvent>>> {
        self.event_rx.clone()
    }

    /// Internal request method with explicit response type for use by RelayInstance
    pub async fn request_internal(
        &self,
        request_type: RequestType,
        _response_type: ResponseType,
        data: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>> {
        self.request_with_response(request_type, data, timeout_ms)
            .await
    }

    /// Internal send method (no response expected) for use by RelayInstance
    pub async fn send_internal(&self, request_type: RequestType, data: &[u8]) -> Result<()> {
        // Datagram wire format is identical to stream format:
        // [Length: u16][UID: u16][Type: u8][payload...]
        // where Length = total inclusive byte count (5 + data.len())
        let uid = self.next_state().await;

        let length = 5 + data.len();
        let mut buffer = Buffer::with_capacity(length);
        buffer.write_u16(length as u16); // total inclusive length
        buffer.write_u16(uid);
        buffer.write_u8(request_type as u8);
        buffer.write_bytes(data);

        let packet = buffer.as_slice();

        let connector = self.connector.read().await;

        if let Some(quic) = connector
            .as_any()
            .downcast_ref::<crate::connector::QuicConnector>()
        {
            quic.send_datagram(packet).await?;
        } else {
            return Err(anyhow!("Connector is not a QuicConnector"));
        }

        Ok(())
    }
}
