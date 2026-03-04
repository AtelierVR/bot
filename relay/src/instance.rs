use crate::buffer::Buffer;
use crate::relay::NoxRelay;
use crate::types::*;
use anyhow::Result;
use std::sync::Arc;

pub struct RelayInstance {
    pub id: u64,
    pub master: u64,
    pub name: String,
    relay: Arc<NoxRelay>,
}

impl RelayInstance {
    pub fn new(info: RelayInstanceInfo, relay: Arc<NoxRelay>) -> Self {
        Self {
            id: info.id,
            master: info.master,
            name: info.name,
            relay,
        }
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
            let _threshold = buf.read_f32()?;
            let entity_id = buf.read_f32()? as u16;

            Ok(EnterResponse::Success {
                player_id,
                entity_id,
                tps,
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

        if request.action == TravelingAction::Failed && request.reason.is_some() {
            buffer.write_string(request.reason.as_ref().unwrap());
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
        buffer.write_u16(request.parameters.len() as u16);

        for (key, value) in &request.parameters {
            buffer.write_string(key);
            buffer.write_u16(value.len() as u16);
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
        buffer.write_u8(self.id as u8); // Instance internal ID
        buffer.write_u16(request.player_id);
        buffer.write_u64(request.avatar_id);
        buffer.write_string(&request.avatar_server);

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

        if result == 0x01 {
            // Success
            Ok(AvatarChangeResponse::Success)
        } else if result == 0x02 {
            // Failed
            let message = buf
                .read_string()
                .unwrap_or_else(|_| "Unknown error".to_string());
            Ok(AvatarChangeResponse::Error {
                code: result,
                message,
            })
        } else {
            Ok(AvatarChangeResponse::Failed)
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
}
