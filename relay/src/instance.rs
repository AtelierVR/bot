use crate::buffer::Buffer;
use crate::relay::NoxRelay;
use crate::types::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone)]
pub struct RelayInstance {
    pub id: u64,
    pub master: u64,
    pub name: String,
    relay: Arc<NoxRelay>,
    /// Current TPS (updated from ServerConfig)
    current_tps: Arc<RwLock<u8>>,
    /// Current threshold (updated from ServerConfig)
    current_threshold: Arc<RwLock<f32>>,
    /// Current property resend interval (updated from Enter)
    current_property_resend_interval: Arc<RwLock<u8>>,
    /// Channel to notify TPS changes
    tps_change_tx: Arc<RwLock<Option<mpsc::UnboundedSender<u8>>>>,
}

impl RelayInstance {
    pub fn new(info: RelayInstanceInfo, relay: Arc<NoxRelay>) -> Self {
        Self {
            id: info.id,
            master: info.master,
            name: info.name,
            relay,
            current_tps: Arc::new(RwLock::new(20)),
            current_threshold: Arc::new(RwLock::new(0.01)),
            current_property_resend_interval: Arc::new(RwLock::new(0)),
            tps_change_tx: Arc::new(RwLock::new(None)),
        }
    }

    /// Set a callback channel that will be notified when TPS changes
    pub async fn set_tps_change_listener(&self, tx: mpsc::UnboundedSender<u8>) {
        *self.tps_change_tx.write().await = Some(tx);
    }

    /// Update TPS from a ServerConfig broadcast
    pub async fn update_tps_from_broadcast(&self, new_tps: u8) -> bool {
        let mut tps = self.current_tps.write().await;
        if *tps != new_tps {
            *tps = new_tps;
            
            // Notify listener if set
            if let Some(tx) = self.tps_change_tx.read().await.as_ref() {
                let _ = tx.send(new_tps);
            }
            
            true
        } else {
            false
        }
    }

    /// Update threshold from a ServerConfig broadcast
    pub async fn update_threshold_from_broadcast(&self, new_threshold: f32) -> bool {
        let mut threshold = self.current_threshold.write().await;
        if (*threshold - new_threshold).abs() > 0.001 {
            *threshold = new_threshold;
            true
        } else {
            false
        }
    }

    /// Get the current TPS
    pub async fn get_current_tps(&self) -> u8 {
        *self.current_tps.read().await
    }

    /// Get the current threshold
    pub async fn get_current_threshold(&self) -> f32 {
        *self.current_threshold.read().await
    }

    /// Get the current property resend interval
    pub async fn get_current_property_resend_interval(&self) -> u8 {
        *self.current_property_resend_interval.read().await
    }

    pub async fn enter(&self, request: EnterRequest) -> Result<EnterResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID

        // Calculate enter flags - auto-add flags based on provided data
        let mut flags = request.flags;
        if !request.display.is_empty() {
            flags |= EnterFlags::USE_PSEUDONYM;
        }
        if request.password.is_some() && !request.password.as_ref().unwrap().is_empty() {
            flags |= EnterFlags::USE_PASSWORD;
        }

        buffer.write_u8(flags.bits());

        if flags.contains(EnterFlags::USE_PSEUDONYM) {
            buffer.write_string(&request.display);
        }
        if flags.contains(EnterFlags::USE_PASSWORD) {
            buffer.write_string(request.password.as_deref().unwrap_or(""));
        }

        let response = self
            .relay
            .request_internal(
                crate::protocol::RequestType::Enter,
                crate::protocol::ResponseType::Enter,
                buffer.as_slice(),
                10000,
            )
            .await?;

        if response.is_empty() {
            return Err(anyhow::anyhow!(
                "Empty response from server - not authenticated?"
            ));
        }

        let mut buf = Buffer::from_vec(response);
        let _iid = buf.read_u8()?;
        let result = buf.read_u8()?;

        if result == 0 {
            // Success
            let _player_flags = buf.read_u32()?;
            let player_id = buf.read_u16()?;
            let _user_id = buf.read_u32()?;
            let _user_address = buf.read_string()?;
            let _display = buf.read_string()?;
            let _created_at = buf.read_i64()?;
            let tps = buf.read_u8()?;
            let threshold = buf.read_f32()?;
            let entity_id = buf.read_f32()? as u16;
            let property_resend_interval = buf.read_u8()?;

            // Update current TPS, threshold and property_resend_interval
            *self.current_tps.write().await = tps;
            *self.current_threshold.write().await = threshold;
            *self.current_property_resend_interval.write().await = property_resend_interval;

            Ok(EnterResponse::Success {
                player_id,
                entity_id,
                tps,
                property_resend_interval,
            })
        } else {
            let reason = buf
                .read_string()
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(EnterResponse::Error {
                code: result,
                reason,
            })
        }
    }

    pub async fn traveling(&self, request: TravelingRequest) -> Result<TravelingResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID
        buffer.write_u8(request.action.as_u8());

        if request.action == TravelingAction::Failed {
            if let Some(reason) = &request.reason {
                buffer.write_string(reason);
            }
        }

        let response = self
            .relay
            .request_internal(
                crate::protocol::RequestType::Traveling,
                crate::protocol::ResponseType::Traveling,
                buffer.as_slice(),
                15000,
            )
            .await?;

        let mut buf = Buffer::from_vec(response);
        let _iid = buf.read_u8()?;
        let results = buf.read_u8()?;

        // Check for ready flag (0x20)
        if results & 0x20 != 0 {
            Ok(TravelingResponse::Success {
                status: "ready".to_string(),
            })
        } else if results & 0x10 != 0 {
            let reason = buf
                .read_string()
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(TravelingResponse::Error { reason })
        } else {
            Ok(TravelingResponse::Success {
                status: "traveling".to_string(),
            })
        }
    }

    pub async fn transform(&self, request: TransformRequest) -> Result<()> {
        use crate::types::TransformFlags;

        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID

        // Transform type
        buffer.write_u8(request.transform_type.as_u8());

        // Player ID and Rig ID
        buffer.write_u16(request.player_id);
        buffer.write_u16(request.rig_id);

        // Write transform flags
        let mut flags = TransformFlags::empty();
        if request.transform.position.is_some() {
            flags |= TransformFlags::POSITION;
        }
        if request.transform.rotation.is_some() {
            flags |= TransformFlags::ROTATION;
        }
        if request.transform.scale.is_some() {
            flags |= TransformFlags::SCALE;
        }
        buffer.write_u8(flags.bits());

        if let Some(pos) = &request.transform.position {
            buffer.write_f32(pos.x);
            buffer.write_f32(pos.y);
            buffer.write_f32(pos.z);
        }

        if let Some(rot) = &request.transform.rotation {
            buffer.write_f32(rot.x);
            buffer.write_f32(rot.y);
            buffer.write_f32(rot.z);
            buffer.write_f32(rot.w);
        }

        if let Some(scale) = &request.transform.scale {
            buffer.write_f32(scale.x);
            buffer.write_f32(scale.y);
            buffer.write_f32(scale.z);
        }

        self.relay
            .send_internal(crate::protocol::RequestType::Transform, buffer.as_slice())
            .await?;

        Ok(())
    }

    pub async fn set_properties(&self, request: PropertiesRequest) -> Result<()> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID
        buffer.write_u16(request.entity_id);
        buffer.write_u8(request.parameters.len() as u8);

        for (key, value) in &request.parameters {
            buffer.write_i32(*key);
            buffer.write_u8(value.len() as u8);
            buffer.write_bytes(value);
        }

        self.relay
            .send_internal(crate::protocol::RequestType::Properties, buffer.as_slice())
            .await?;

        Ok(())
    }

    pub async fn change_avatar(
        &self,
        request: AvatarChangeRequest,
    ) -> Result<AvatarChangeResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8);         // iid: u8
        buffer.write_u16(request.player_id);    // pid: u16
        buffer.write_u32(request.avatar_id as u32); // avatar_id: u32
        buffer.write_string(&request.avatar_server); // server: string
        buffer.write_u16(request.version);          // version: u16

        let response = self
            .relay
            .request_internal(
                crate::protocol::RequestType::AvatarChanged,
                crate::protocol::ResponseType::AvatarChanged,
                buffer.as_slice(),
                5000,
            )
            .await?;

        let mut buf = Buffer::from_vec(response);
        let _iid = buf.read_u8()?;
        let result = buf.read_u8()?;

        match result {
            3 => Ok(AvatarChangeResponse::Success), // AvatarChangedResult::Success = 3
            2 => {
                // AvatarChangedResult::Failed = 2
                let message = buf
                    .read_string()
                    .unwrap_or_else(|_| "Unknown error".to_string());
                Ok(AvatarChangeResponse::Error {
                    code: result,
                    message,
                })
            }
            _ => Ok(AvatarChangeResponse::Failed),
        }
    }

    pub async fn send_event(&self, request: EventRequest) -> Result<()> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID
        let name_hash = crc64(&request.name);
        buffer.write_u64(name_hash);
        buffer.write_u16(request.data.len() as u16);
        buffer.write_bytes(&request.data);
        buffer.write_u16(request.targets.len() as u16);
        for target in &request.targets {
            buffer.write_u16(*target);
        }

        self.relay
            .send_internal(crate::protocol::RequestType::Event, buffer.as_slice())
            .await?;

        Ok(())
    }

    /// Send a voice sample datagram.
    /// Wire format (after send_internal wrapper):
    ///   [iid:u8][sub_type:u8(0x00)][channel_id:u32][level_flags:u8][frame_index:i32][timestamp:f64][sample:bytes]
    /// Matches C# StreamRequest.Sample wire format with MetaVoiceChat frame metadata.
    pub async fn send_voice_sample(
        &self,
        channel_id: u32,
        level_flags: u8,
        frame_index: i32,
        timestamp: f64,
        sample: &[u8],
    ) -> Result<()> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // iid
        buffer.write_u8(0x00);          // sub_type: Sample
        buffer.write_u32(channel_id);
        buffer.write_u8(level_flags);
        buffer.write_i32(frame_index);
        buffer.write_f64(timestamp);
        buffer.write_bytes(sample);

        self.relay
            .send_internal(crate::protocol::RequestType::Stream, buffer.as_slice())
            .await?;

        Ok(())
    }

    /// Query current server configuration (TPS, threshold, capacity, etc.)
    /// When flags is NONE (0x00), the server returns all current values.
    pub async fn get_server_config(&self) -> Result<ServerConfigResponse> {
        let mut buffer = Buffer::new();
        buffer.write_u8(self.id as u8); // Instance internal ID
        buffer.write_u8(ServerConfigFlags::NONE.bits()); // Query all settings

        let response = self
            .relay
            .request_internal(
                crate::protocol::RequestType::ServerConfig,
                crate::protocol::ResponseType::ServerConfig,
                buffer.as_slice(),
                5000,
            )
            .await?;

        let mut buf = Buffer::from_vec(response);
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

        // Update current values if received
        if let Some(t) = tps {
            *self.current_tps.write().await = t;
        }
        if let Some(th) = threshold {
            *self.current_threshold.write().await = th;
        }

        Ok(ServerConfigResponse {
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
        })
    }
}
