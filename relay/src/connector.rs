use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn send(&self, data: &[u8]) -> Result<()>;
    async fn receive(&self, buf: &mut [u8]) -> Result<usize>;
    async fn close(&mut self) -> Result<()>;
    fn is_connected(&self) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

use std::sync::Arc;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::Mutex;

pub struct TcpConnector {
    host: String,
    port: u16,
    reader: Option<Arc<Mutex<ReadHalf<TcpStream>>>>,
    writer: Option<Arc<Mutex<WriteHalf<TcpStream>>>>,
    connected: bool,
}

impl TcpConnector {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            reader: None,
            writer: None,
            connected: false,
        }
    }
}

#[async_trait]
impl Connector for TcpConnector {
    async fn connect(&mut self) -> Result<()> {
        debug!("Connecting to {}:{}...", self.host, self.port);
        let stream = TcpStream::connect((&self.host[..], self.port)).await?;
        let (reader, writer) = tokio::io::split(stream);
        self.reader = Some(Arc::new(Mutex::new(reader)));
        self.writer = Some(Arc::new(Mutex::new(writer)));
        self.connected = true;
        debug!("Connected to {}:{}", self.host, self.port);
        Ok(())
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        if let Some(ref writer) = self.writer {
            let mut w = writer.lock().await;
            w.write_all(data).await?;
            w.flush().await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Not connected"))
        }
    }

    async fn receive(&self, buf: &mut [u8]) -> Result<usize> {
        if let Some(ref reader) = self.reader {
            let mut r = reader.lock().await;
            let n = r.read(buf).await?;
            Ok(n)
        } else {
            Err(anyhow::anyhow!("Not connected"))
        }
    }

    async fn close(&mut self) -> Result<()> {
        self.reader = None;
        self.writer = None;
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct UdpConnector {
    host: String,
    port: u16,
    socket: Option<Arc<tokio::net::UdpSocket>>,
    connected: bool,
}

impl UdpConnector {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            socket: None,
            connected: false,
        }
    }
}

#[async_trait]
impl Connector for UdpConnector {
    async fn connect(&mut self) -> Result<()> {
        debug!("Connecting UDP to {}:{}...", self.host, self.port);
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect((&self.host[..], self.port)).await?;
        self.socket = Some(Arc::new(socket));
        self.connected = true;
        debug!("Connected UDP to {}:{}", self.host, self.port);
        Ok(())
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        if let Some(ref socket) = self.socket {
            socket.send(data).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Not connected"))
        }
    }

    async fn receive(&self, buf: &mut [u8]) -> Result<usize> {
        if let Some(ref socket) = self.socket {
            let n = socket.recv(buf).await?;
            Ok(n)
        } else {
            Err(anyhow::anyhow!("Not connected"))
        }
    }

    async fn close(&mut self) -> Result<()> {
        self.socket = None;
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ─── QUIC Connector ──────────────────────────────────────────────────────────

use quinn::{ClientConfig, Connection, Endpoint};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::net::SocketAddr;

/// A TLS verifier that accepts any server certificate.
/// Used for bot testing where self-signed certificates are common.
#[derive(Debug)]
pub struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub struct QuicConnector {
    host: String,
    port: u16,
    /// Skip TLS certificate verification (useful for self-signed certs in dev/test)
    skip_cert_verification: bool,
    connection: Option<Connection>,
    connected: bool,
}

impl QuicConnector {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            skip_cert_verification: true,
            connection: None,
            connected: false,
        }
    }

    /// Create a connector with explicit TLS certificate verification control.
    pub fn with_tls_verification(host: String, port: u16, skip_cert_verification: bool) -> Self {
        Self {
            host,
            port,
            skip_cert_verification,
            connection: None,
            connected: false,
        }
    }

    fn build_client_config(&self) -> Result<ClientConfig> {
        // Always supply the provider explicitly so rustls never tries to
        // auto-detect the process-level default (which panics when both
        // ring and aws-lc-rs are compiled in).
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        let crypto = if self.skip_cert_verification {
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()?
                .dangerous()
                .with_custom_certificate_verifier(SkipServerVerification::new())
                .with_no_client_auth()
        } else {
            // Load native system certificates
            let mut roots = rustls::RootCertStore::empty();
            let certs = rustls_native_certs::load_native_certs();
            for cert in certs.certs {
                roots.add(cert)?;
            }
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()?
                .with_root_certificates(roots)
                .with_no_client_auth()
        };
        
        // Enable QUIC datagrams for server broadcasts
        let mut transport = quinn::TransportConfig::default();
        transport.datagram_receive_buffer_size(Some(65536));  // 64KB buffer for datagrams
        transport.datagram_send_buffer_size(65536);
        
        let mut client_config = ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
        ));
        client_config.transport_config(Arc::new(transport));
        
        Ok(client_config)
    }
}

#[async_trait]
impl Connector for QuicConnector {
    async fn connect(&mut self) -> Result<()> {
        debug!("Connecting QUIC to {}:{}...", self.host, self.port);

        let client_config = self.build_client_config()?;

        // Bind to an OS-assigned local address
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse::<SocketAddr>()?)?;
        endpoint.set_default_client_config(client_config);

        let server_addr: SocketAddr =
            tokio::net::lookup_host(format!("{}:{}", self.host, self.port))
                .await?
                .next()
                .ok_or_else(|| anyhow::anyhow!("DNS resolution failed for {}", self.host))?;

        let conn = endpoint.connect(server_addr, &self.host)?.await?;

        debug!("QUIC connection established to {}:{}", self.host, self.port);

        self.connection = Some(conn);
        self.connected = true;

        Ok(())
    }

    async fn send(&self, data: &[u8]) -> Result<()> {
        // For QUIC, we need to open a new stream for each request
        // This method is kept for compatibility but sends without expecting a response
        if let Some(ref conn) = self.connection {
            let (mut send, _recv) = conn.open_bi().await?;
            send.write_all(data).await?;
            send.finish()?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("QUIC: not connected"))
        }
    }

    async fn receive(&self, _buf: &mut [u8]) -> Result<usize> {
        // This method is no longer used in QUIC mode since each request/response
        // happens on its own stream
        Err(anyhow::anyhow!(
            "QUIC: receive should not be called directly, use send_and_receive"
        ))
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(conn) = self.connection.take() {
            conn.close(0u32.into(), b"client closed");
        }
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl QuicConnector {
    /// Send a request and receive a response on a new bidirectional stream.
    /// This matches the server's architecture where each stream handles one request/response.
    pub async fn send_and_receive(&self, data: &[u8]) -> Result<Vec<u8>> {
        if let Some(ref conn) = self.connection {
            // Open a new bidirectional stream for this request
            let (mut send, mut recv) = conn.open_bi().await?;

            // Send the request
            send.write_all(data).await?;
            send.finish()?;

            // Read the response
            let mut response = Vec::new();
            let mut buf = vec![0u8; 65536];
            while let Some(n) = recv.read(&mut buf).await? {
                response.extend_from_slice(&buf[..n]);
            }

            Ok(response)
        } else {
            Err(anyhow::anyhow!("QUIC: not connected"))
        }
    }

    /// Send data as a datagram without expecting a response.
    /// Used for Transform, Properties, AvatarChanged, etc.
    pub async fn send_datagram(&self, data: &[u8]) -> Result<()> {
        if let Some(ref conn) = self.connection {
            conn.send_datagram(data.to_vec().into())?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("QUIC: not connected"))
        }
    }

    /// Receive a datagram (blocking until one arrives).
    /// Returns Ok(data) when a datagram is received, or Err on connection error.
    pub async fn recv_datagram(&self) -> Result<Vec<u8>> {
        if let Some(ref conn) = self.connection {
            match conn.read_datagram().await {
                Ok(data) => Ok(data.to_vec()),
                Err(e) => Err(anyhow::anyhow!("Datagram read error: {}", e)),
            }
        } else {
            Err(anyhow::anyhow!("QUIC: not connected"))
        }
    }

    /// Get the connection for accessing datagrams
    pub fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()
    }
}
