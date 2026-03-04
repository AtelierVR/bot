use bitflags::bitflags;
use std::collections::HashMap;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EnterFlags: u8 {
        const NONE           = 0;
        const AS_BOT         = 1 << 0;
        const USE_PSEUDONYM  = 1 << 1;
        const USE_PASSWORD   = 1 << 2;
        const HIDE_IN_LIST   = 1 << 3;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TransformFlags: u8 {
        const POSITION         = 0x01;
        const ROTATION         = 0x02;
        const SCALE            = 0x04;
        const VELOCITY         = 0x08;
        const ANGULAR_VELOCITY = 0x10;
        const RESET            = 0x20;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ServerConfigFlags: u8 {
        const NONE                  = 0x00;
        const TPS                   = 0x01;
        const THRESHOLD             = 0x02;
        const CAPACITY              = 0x04;
        const PASSWORD              = 0x08;
        const FLAGS                 = 0x10;
        const MIN_TPS               = 0x20;
        const MAX_TPS               = 0x40;
        const LOAD_BALANCING        = 0x80;
        const ALL                   = 0xFF;
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TravelingAction {
    Travel = 0,
    Ready = 1,
    Failed = 2,
}

impl TravelingAction {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformType {
    ByPath = 0,
    EntityPart = 1,
}

impl TransformType {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Debug, Clone)]
pub struct Transform {
    pub position: Option<Vector3>,
    pub rotation: Option<Quaternion>,
    pub scale: Option<Vector3>,
}

#[derive(Debug, Clone)]
pub struct HandshakeRequest {
    pub protocol: u16,
    pub engine: String,
    pub platform: String,
}

#[derive(Debug, Clone)]
pub struct HandshakeResponse {
    pub protocol: u16,
    pub client_id: u16,
    pub ip: String,
    pub port: u16,
    pub flags: Vec<String>,
    pub master: Option<String>,
    pub max_packet_size: u16,
    pub timeout: u16,
    pub keep_alive: u16,
}

#[derive(Debug, Clone)]
pub struct DisconnectRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ServerConfigRequest {
    pub instance_id: u8,
    pub flags: ServerConfigFlags,
    pub tps: Option<u8>,
    pub threshold: Option<f32>,
    pub capacity: Option<u16>,
    pub password: Option<String>,
    pub instance_flags: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ServerConfigResponse {
    pub instance_id: u8,
    pub result: ServerConfigResult,
    pub tps: Option<u8>,
    pub threshold: Option<f32>,
    pub capacity: Option<u16>,
    pub has_password: Option<bool>,
    pub instance_flags: Option<u32>,
    pub min_tps: Option<u8>,
    pub max_tps: Option<u8>,
    pub load_balancing_enabled: Option<bool>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerConfigResult {
    Success = 0,
    Failure = 1,
    Change = 2,
}

#[derive(Debug, Clone)]
pub struct DisconnectResponse {
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LatencyRequest;

#[derive(Debug, Clone)]
pub struct LatencyResponse {
    pub initial: i64,
    pub server_time: i64,
    pub final_time: i64,
    pub rtt: i64,
}

impl LatencyResponse {
    /// Half of RTT (approximation of one-way latency)
    pub fn up(&self) -> i64 {
        self.rtt / 2
    }

    /// Half of RTT (approximation of one-way latency)
    pub fn down(&self) -> i64 {
        self.rtt / 2
    }

    /// Total round-trip time in milliseconds
    pub fn total(&self) -> i64 {
        self.rtt
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticateRequest {
    pub user_id: u64,
    pub user_server: String,
    pub public_key: Vec<u8>,
    pub private_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum AuthenticateResponse {
    Success { session_id: u64 },
    Error { code: u8, message: String },
}

#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub page: u32,
}

#[derive(Debug, Clone)]
pub struct SessionResponse {
    pub instances: Vec<RelayInstanceInfo>,
    pub current_page: u8,
    pub total_pages: u8,
}

#[derive(Debug, Clone)]
pub struct RelayInstanceInfo {
    pub id: u64,
    pub master: u64,
    pub name: String,
    pub capacity: u32,
    pub client_count: u32,
}

#[derive(Debug, Clone)]
pub struct EnterRequest {
    pub instance_id: u64,
    pub display: String,
    pub flags: EnterFlags,
    pub password: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EnterResponse {
    Success {
        player_id: u16,
        entity_id: u16,
        tps: u8,
    },
    Error {
        code: u8,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct TravelingRequest {
    pub action: TravelingAction,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TravelingResponse {
    Success { status: String },
    Error { reason: String },
}

#[derive(Debug, Clone)]
pub struct TransformRequest {
    pub player_id: u16,
    pub rig_id: u16,
    pub transform: Transform,
    pub transform_type: TransformType,
}

#[derive(Debug, Clone)]
pub struct AvatarChangeRequest {
    pub player_id: u16,
    pub avatar_id: u64,
    pub avatar_server: String,
}

#[derive(Debug, Clone)]
pub enum AvatarChangeResponse {
    Success,
    Failed,
    Error { code: u8, message: String },
}

#[derive(Debug, Clone)]
pub struct PropertiesRequest {
    pub entity_id: u16,
    pub parameters: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct EventRequest {
    pub name: String,
    pub data: Vec<u8>,
    pub targets: Vec<u16>,
}

#[derive(Debug, Clone)]
pub enum RelayEvent {
    Traveling(TravelingEvent),
    PlayerUpdate(PlayerUpdateEvent),
    Properties(PropertiesEvent),
    AvatarChanged(AvatarChangedEvent),
    Enter(EnterEvent),
    Leave(LeaveEvent),
    Join(JoinEvent),
    Quit(QuitEvent),
    Event(CustomEvent),
    Transform(TransformEvent),
}

#[derive(Debug, Clone)]
pub struct TravelingEvent {
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct PlayerUpdateEvent {
    pub player_id: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PropertiesEvent {
    pub entity_id: u16,
    pub parameters: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct AvatarChangedEvent {
    pub player_id: u16,
    pub avatar_id: u64,
    pub avatar_server: String,
}

#[derive(Debug, Clone)]
pub struct EnterEvent {
    pub player_id: u16,
    pub entity_id: u16,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct LeaveEvent {
    pub player_id: u16,
}

#[derive(Debug, Clone)]
pub struct JoinEvent {
    pub player_id: u16,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct QuitEvent {
    pub player_id: u16,
}

#[derive(Debug, Clone)]
pub struct CustomEvent {
    pub name: u64,
    pub player_id: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TransformEvent {
    pub entity_id: u16,
    pub transform: Transform,
}

// Utility function to compute CRC64
pub fn crc64(s: &str) -> u64 {
    let digest = crc::Crc::<u64>::new(&crc::CRC_64_ECMA_182);
    let mut hasher = digest.digest();
    hasher.update(s.as_bytes());
    hasher.finalize()
}
