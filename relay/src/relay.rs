use crate::auth::{
    create_challenge_request, create_resolve_request, parse_auth_response, AuthResponse,
    Credentials,
};
use crate::buffer::Buffer;
use crate::connector::Connector;
use crate::protocol::{RequestType, ResponseType};
use crate::types::*;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

pub struct NoxRelay {
    connector: Arc<RwLock<Box<dyn Connector>>>,
    event_tx: mpsc::UnboundedSender<RelayEvent>,
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<RelayEvent>>>,
    pending_responses: Arc<RwLock<HashMap<u16, tokio::sync::oneshot::Sender<Vec<u8>>>>>,
    counter: Arc<Mutex<u16>>,
    running: Arc<AtomicBool>,
    keep_alive_interval: Arc<Mutex<u64>>,
    last_ping: Arc<Mutex<Option<LatencyResponse>>>,
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
        }
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
        info!("Starting handshake with protocol {}", request.protocol);
        let mut buffer = Buffer::new();
        buffer.write_u16(request.protocol);
        buffer.write_string(&request.engine);
        buffer.write_string(&request.platform);

        info!(
            "📤 Handshake Buffer: hex={}, bytes={:?}, length={}",
            buffer
                .as_slice()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" "),
            buffer.as_slice(),
            buffer.as_slice().len()
        );

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
        info!(
            "Starting authentication for user {}@{}",
            credentials.user_id, credentials.server
        );

        // Étape 1: Demander un challenge
        let challenge_req = create_challenge_request();
        let challenge_resp_data = self
            .request_with_response(RequestType::Authentication, &challenge_req, 5000)
            .await?;

        let challenge_resp = parse_auth_response(&challenge_resp_data).map_err(|e| anyhow!(e))?;

        let challenge = match challenge_resp {
            AuthResponse::Challenge { challenge } => {
                debug!("[Auth] Received challenge: {} bytes", challenge.len());
                debug!("[Auth] Challenge (hex): {}", hex::encode(&challenge));
                challenge
            }
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
        debug!(
            "[Auth] Signed challenge: {} bytes signature",
            signature.len()
        );
        debug!(
            "[Auth] Signature (first 64 bytes, hex): {}",
            hex::encode(&signature[..signature.len().min(64)])
        );

        // Étape 3: Envoyer la résolution du challenge
        let resolve_req =
            create_resolve_request(credentials, &signature).map_err(|e| anyhow!(e))?;
        let resolve_resp_data = self
            .request_with_response(RequestType::Authentication, &resolve_req, 10000)
            .await?;

        let resolve_resp = parse_auth_response(&resolve_resp_data).map_err(|e| anyhow!(e))?;

        match &resolve_resp {
            AuthResponse::Success {
                user_id,
                address,
                display_name,
            } => {
                info!(
                    "✓ Authentication successful: user={}@{} display=\"{}\"",
                    user_id, address, display_name
                );
            }
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

        let response = self
            .request_with_response(RequestType::Latency, buffer.as_slice(), 5000)
            .await?;
        let mut buf = Buffer::from_vec(response);

        let initial_resp = buf.read_i64()?;
        let intermediate = buf.read_i64()?;
        let final_time = chrono::Utc::now().timestamp_millis();

        Ok(LatencyResponse {
            initial: initial_resp,
            intermediate,
            final_time,
        })
    }

    pub async fn sessions(&self, request: SessionRequest) -> Result<SessionResponse> {
        info!("Requesting sessions page {}", request.page);
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

        debug!(
            "Sessions: {} instances, page {}/{}",
            count, current_page, total_pages
        );

        Ok(SessionResponse { instances })
    }

    /// Envoie une requête de déconnexion propre au serveur relay
    pub async fn disconnect(&self, reason: Option<String>) -> Result<DisconnectResponse> {
        info!("Disconnecting from relay (reason: {:?})", reason);

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
        // For datagrams, format is: [UID: u16][Type: u8][data...]
        // (no length or state fields)
        let uid = self.next_state().await;

        let length = 3 + data.len();
        let mut buffer = Buffer::with_capacity(length);
        buffer.write_u16(uid); // UID instead of length
        buffer.write_u8(request_type as u8);
        buffer.write_bytes(data);

        let packet = buffer.as_slice();
        // debug!("Sending one-way request: {:?}, uid: {}, packet_len: {}",
        //        request_type, uid, packet.len());

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
