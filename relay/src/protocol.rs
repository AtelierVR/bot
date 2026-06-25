#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    None = 0xFF,
    Disconnect = 0x00,
    Handshake = 0x01,
    Segmentation = 0x02,
    Reliable = 0x03,
    Latency = 0x04,
    Authentication = 0x05,
    Enter = 0x06,
    Quit = 0x07,
    Custom = 0x08,
    PasswordRequirement = 0x09,
    Traveling = 0x0A,
    Transform = 0x0B,
    Teleport = 0x0C,
    AvatarChanged = 0x0D,
    ServerConfig = 0x0E,
    Properties = 0x0F,
    PlayerUpdate = 0x12,
    Sessions = 0x13,
    Stream = 0x14,
    Event = 0x15,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    None = 0xFF,
    Disconnect = 0x00,
    Handshake = 0x01,
    Segmentation = 0x02,
    Reliable = 0x03,
    Latency = 0x04,
    Authentication = 0x05,
    Enter = 0x06,
    Quit = 0x07,
    Custom = 0x08,
    PasswordRequirement = 0x09,
    Traveling = 0x0A,
    Transform = 0x0B,
    Teleport = 0x0C,
    AvatarChanged = 0x0D,
    ServerConfig = 0x0E,
    Properties = 0x0F,
    Join = 0x10,
    Leave = 0x11,
    PlayerUpdate = 0x12,
    Sessions = 0x13,
    Stream = 0x14,
    Event = 0x15,
}

impl RequestType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0xFF => Some(Self::None),
            0x00 => Some(Self::Disconnect),
            0x01 => Some(Self::Handshake),
            0x02 => Some(Self::Segmentation),
            0x03 => Some(Self::Reliable),
            0x04 => Some(Self::Latency),
            0x05 => Some(Self::Authentication),
            0x06 => Some(Self::Enter),
            0x07 => Some(Self::Quit),
            0x08 => Some(Self::Custom),
            0x09 => Some(Self::PasswordRequirement),
            0x0A => Some(Self::Traveling),
            0x0B => Some(Self::Transform),
            0x0C => Some(Self::Teleport),
            0x0D => Some(Self::AvatarChanged),
            0x0E => Some(Self::ServerConfig),
            0x0F => Some(Self::Properties),
            0x12 => Some(Self::PlayerUpdate),
            0x13 => Some(Self::Sessions),
            0x14 => Some(Self::Stream),
            0x15 => Some(Self::Event),
            _ => None,
        }
    }
}

impl ResponseType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0xFF => Some(Self::None),
            0x00 => Some(Self::Disconnect),
            0x01 => Some(Self::Handshake),
            0x02 => Some(Self::Segmentation),
            0x03 => Some(Self::Reliable),
            0x04 => Some(Self::Latency),
            0x05 => Some(Self::Authentication),
            0x06 => Some(Self::Enter),
            0x07 => Some(Self::Quit),
            0x08 => Some(Self::Custom),
            0x09 => Some(Self::PasswordRequirement),
            0x0A => Some(Self::Traveling),
            0x0B => Some(Self::Transform),
            0x0C => Some(Self::Teleport),
            0x0D => Some(Self::AvatarChanged),
            0x0E => Some(Self::ServerConfig),
            0x0F => Some(Self::Properties),
            0x10 => Some(Self::Join),
            0x11 => Some(Self::Leave),
            0x12 => Some(Self::PlayerUpdate),
            0x13 => Some(Self::Sessions),
            0x14 => Some(Self::Stream),
            0x15 => Some(Self::Event),
            _ => None,
        }
    }
}
