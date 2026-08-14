use async_trait::async_trait;
use noxrelay::{PropertiesRequest, Quaternion, RelayInstance, Transform, TransformRequest, TransformType, Vector3};
use std::collections::HashMap;

#[async_trait]
pub trait Movement: Send + Sync {
    fn name(&self) -> &str;
    #[allow(dead_code)]
    fn description(&self) -> &str;
    /// Current speed value (useful for logging, after overrides)
    fn speed(&self) -> f32;
    fn initialize(&self, index: usize) -> MovementState;
    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance);
}

#[derive(Debug, Clone)]
pub struct MovementState {
    pub position: Vector3,
    pub rotation: Quaternion,
    #[allow(dead_code)]
    pub velocity: Vector3,
    pub time: f32,
    pub player_id: u16,
    #[allow(dead_code)]
    pub custom_data: std::collections::HashMap<String, f64>,
}

impl MovementState {
    pub fn new(x: f32, y: f32, z: f32, player_id: u16) -> Self {
        Self {
            position: Vector3 { x, y, z },
            rotation: Quaternion {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            velocity: Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            time: 0.0,
            player_id,
            custom_data: std::collections::HashMap::new(),
        }
    }
}

/// Clamp velocity to max magnitude and send as properties.
async fn send_velocity_properties(
    vx: f32,
    vz: f32,
    max_speed: f32,
    entity_id: u16,
    instance: &RelayInstance,
) {
    let mut vx = vx;
    let mut vz = vz;

    // Clamp to max magnitude
    let magnitude = (vx * vx + vz * vz).sqrt();
    if magnitude > max_speed {
        let scale = max_speed / magnitude;
        vx *= scale;
        vz *= scale;
    }

    let mut params: HashMap<i32, Vec<u8>> = HashMap::new();
    params.insert(crc32fast::hash(b"VelocityX") as i32, vx.to_be_bytes().to_vec());
    params.insert(crc32fast::hash(b"VelocityZ") as i32, vz.to_be_bytes().to_vec());

    let _ = instance
        .set_properties(PropertiesRequest {
            entity_id,
            parameters: params,
        })
        .await;
}

pub struct CircularMovement {
    pub radius: f32,
    pub speed: f32,
    pub max_speed: f32,
}

#[async_trait]
impl Movement for CircularMovement {
    fn name(&self) -> &str {
        "circular"
    }

    fn description(&self) -> &str {
        "Mouvement circulaire"
    }

    fn speed(&self) -> f32 {
        self.speed
    }

    fn initialize(&self, index: usize) -> MovementState {
        let offset = index as f32 * 2.0;
        MovementState::new(offset, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;
        let angle = state.time * self.speed;
        state.position.x = angle.cos() * self.radius;
        state.position.z = angle.sin() * self.radius;

        // Analytical velocity: tangent to circle
        let vx = -angle.sin() * self.radius * self.speed;
        let vz = angle.cos() * self.radius * self.speed;

        // Send transform to relay
        let _ = instance
            .transform(TransformRequest {
                player_id: state.player_id,
                rig_id: 0,
                transform: Transform {
                    position: Some(state.position.clone()),
                    rotation: Some(state.rotation.clone()),
                    scale: None,
                },
                transform_type: TransformType::EntityPart,
            })
            .await;

        send_velocity_properties(vx, vz, self.max_speed, state.player_id, instance).await;
    }
}

pub struct RandomTeleportMovement {
    pub range: f32,
    pub max_speed: f32,
}

#[async_trait]
impl Movement for RandomTeleportMovement {
    fn name(&self) -> &str {
        "rtp"
    }

    fn description(&self) -> &str {
        "Téléportation aléatoire"
    }

    fn speed(&self) -> f32 {
        0.0 // RTP has no speed field, jumps are instantaneous
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;

        if state.time > 2.0 {
            state.time = 0.0;
            let prev = state.position.clone();
            state.position.x = (rand::random::<f32>() - 0.5) * self.range;
            state.position.z = (rand::random::<f32>() - 0.5) * self.range;

            // Velocity from jump delta over one tick
            let dt_s = dt / 1000.0;
            let vx = (state.position.x - prev.x) / dt_s;
            let vz = (state.position.z - prev.z) / dt_s;

            // Send transform to relay
            let _ = instance
                .transform(TransformRequest {
                    player_id: state.player_id,
                    rig_id: 0,
                    transform: Transform {
                        position: Some(state.position.clone()),
                        rotation: Some(state.rotation.clone()),
                        scale: None,
                    },
                    transform_type: TransformType::EntityPart,
                })
                .await;

            send_velocity_properties(vx, vz, self.max_speed, state.player_id, instance).await;
        }
    }
}

pub struct SquareMovement {
    pub size: f32,
    pub speed: f32,
    pub max_speed: f32,
}

#[async_trait]
impl Movement for SquareMovement {
    fn name(&self) -> &str {
        "square"
    }

    fn description(&self) -> &str {
        "Mouvement en carré"
    }

    fn speed(&self) -> f32 {
        self.speed
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;
        let progress = (state.time * self.speed) % 4.0;

        let half_size = self.size / 2.0;
        // Analytical velocity: speed along the current segment's axis
        let seg_speed = self.size * self.speed;
        let (vx, vz) = match progress as i32 {
            0 => {
                state.position.x = -half_size + (progress.fract() * self.size);
                state.position.z = -half_size;
                (seg_speed, 0.0_f32)
            }
            1 => {
                state.position.x = half_size;
                state.position.z = -half_size + ((progress - 1.0).fract() * self.size);
                (0.0_f32, seg_speed)
            }
            2 => {
                state.position.x = half_size - ((progress - 2.0).fract() * self.size);
                state.position.z = half_size;
                (-seg_speed, 0.0_f32)
            }
            _ => {
                state.position.x = -half_size;
                state.position.z = half_size - ((progress - 3.0).fract() * self.size);
                (0.0_f32, -seg_speed)
            }
        };

        // Send transform to relay
        let _ = instance
            .transform(TransformRequest {
                player_id: state.player_id,
                rig_id: 0,
                transform: Transform {
                    position: Some(state.position.clone()),
                    rotation: Some(state.rotation.clone()),
                    scale: None,
                },
                transform_type: TransformType::EntityPart,
            })
            .await;

        send_velocity_properties(vx, vz, self.max_speed, state.player_id, instance).await;
    }
}

/// Bot stands still — no movement at all.
pub struct NoneMovement;

#[async_trait]
impl Movement for NoneMovement {
    fn name(&self) -> &str {
        "none"
    }

    fn description(&self) -> &str {
        "Aucun mouvement (statique)"
    }

    fn speed(&self) -> f32 {
        0.0
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32 * 0.1, 0.0, 0.0, 0)
    }

    async fn update(&self, _state: &mut MovementState, _dt: f32, _instance: &RelayInstance) {
        // No movement
    }
}

/// Walks forward along Z: 0 → arm, teleports back to 0, repeats.
pub struct ForwardMovement {
    /// Distance to walk before teleporting back
    pub arm: f32,
    /// Walk speed in units/second
    pub speed: f32,
    pub max_speed: f32,
}

#[async_trait]
impl Movement for ForwardMovement {
    fn name(&self) -> &str {
        "forward"
    }

    fn description(&self) -> &str {
        "Marche en avant sur Z (0 → arm, tp à 0)"
    }

    fn speed(&self) -> f32 {
        self.speed
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32 * 0.1, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        let dt_s = dt / 1000.0;

        // Walk forward along Z until arm, then teleport back to 0
        state.position.z += self.speed * dt_s;
        if state.position.z >= self.arm {
            state.position.z = 0.0;
        }

        let _ = instance
            .transform(TransformRequest {
                player_id: state.player_id,
                rig_id: 0,
                transform: Transform {
                    position: Some(state.position.clone()),
                    rotation: Some(state.rotation.clone()),
                    scale: None,
                },
                transform_type: TransformType::EntityPart,
            })
            .await;

        send_velocity_properties(0.0, self.speed, self.max_speed, state.player_id, instance).await;
    }
}

pub struct CrossMovement {
    /// Length of each arm from the center
    pub arm: f32,
    /// Speed in units/second
    pub speed: f32,
    pub max_speed: f32,
}

#[async_trait]
impl Movement for CrossMovement {
    fn name(&self) -> &str {
        "cross"
    }

    fn description(&self) -> &str {
        "Mouvement en croix (centre ↔ chaque côté)"
    }

    fn speed(&self) -> f32 {
        self.speed
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32 * 0.1, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;

        // 8 segments: C→N, N→C, C→E, E→C, C→S, S→C, C→W, W→C
        let phase = (state.time * self.speed / self.arm) % 8.0;
        let seg = phase as i32;
        let t = phase.fract();

        let (vx, vz) = match seg {
            0 => { state.position.x = 0.0; state.position.z = t * self.arm;           (0.0,          self.speed) }
            1 => { state.position.x = 0.0; state.position.z = (1.0 - t) * self.arm;  (0.0,         -self.speed) }
            2 => { state.position.x = t * self.arm;           state.position.z = 0.0; ( self.speed,  0.0) }
            3 => { state.position.x = (1.0 - t) * self.arm;  state.position.z = 0.0; (-self.speed,  0.0) }
            4 => { state.position.x = 0.0; state.position.z = -t * self.arm;          (0.0,         -self.speed) }
            5 => { state.position.x = 0.0; state.position.z = -(1.0 - t) * self.arm; (0.0,          self.speed) }
            6 => { state.position.x = -t * self.arm;          state.position.z = 0.0; (-self.speed,  0.0) }
            _ => { state.position.x = -(1.0 - t) * self.arm; state.position.z = 0.0; ( self.speed,  0.0) }
        };

        let _ = instance
            .transform(TransformRequest {
                player_id: state.player_id,
                rig_id: 0,
                transform: Transform {
                    position: Some(state.position.clone()),
                    rotation: Some(state.rotation.clone()),
                    scale: None,
                },
                transform_type: TransformType::EntityPart,
            })
            .await;

        send_velocity_properties(vx, vz, self.max_speed, state.player_id, instance).await;
    }
}

pub struct TargetMovement {
    /// Half-size of the random target zone
    pub range: f32,
    /// Walk speed in units/second
    pub speed: f32,
    pub max_speed: f32,
    /// Distance at which the target is considered reached
    pub threshold: f32,
}

#[async_trait]
impl Movement for TargetMovement {
    fn name(&self) -> &str {
        "target"
    }

    fn description(&self) -> &str {
        "Marche vers un point aléatoire, puis en choisit un nouveau"
    }

    fn speed(&self) -> f32 {
        self.speed
    }

    fn initialize(&self, index: usize) -> MovementState {
        let mut state = MovementState::new(index as f32 * 0.1, 0.0, 0.0, 0);
        // Pick an initial random target
        state.custom_data.insert("target_x".to_string(), ((rand::random::<f32>() - 0.5) * self.range * 2.0) as f64);
        state.custom_data.insert("target_z".to_string(), ((rand::random::<f32>() - 0.5) * self.range * 2.0) as f64);
        state
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        let dt_s = dt / 1000.0;

        let tx = *state.custom_data.get("target_x").unwrap_or(&0.0) as f32;
        let tz = *state.custom_data.get("target_z").unwrap_or(&0.0) as f32;

        let dx = tx - state.position.x;
        let dz = tz - state.position.z;
        let dist = (dx * dx + dz * dz).sqrt();

        let (vx, vz) = if dist <= self.threshold {
            // Reached — pick new target
            let nx = (rand::random::<f32>() - 0.5) * self.range * 2.0;
            let nz = (rand::random::<f32>() - 0.5) * self.range * 2.0;
            state.custom_data.insert("target_x".to_string(), nx as f64);
            state.custom_data.insert("target_z".to_string(), nz as f64);
            (0.0_f32, 0.0_f32)
        } else {
            let nx = dx / dist;
            let nz = dz / dist;
            let step = self.speed * dt_s;
            state.position.x += nx * step;
            state.position.z += nz * step;
            (nx * self.speed, nz * self.speed)
        };

        let _ = instance
            .transform(TransformRequest {
                player_id: state.player_id,
                rig_id: 0,
                transform: Transform {
                    position: Some(state.position.clone()),
                    rotation: Some(state.rotation.clone()),
                    scale: None,
                },
                transform_type: TransformType::EntityPart,
            })
            .await;

        send_velocity_properties(vx, vz, self.max_speed, state.player_id, instance).await;
    }
}

pub fn get_movements(speed_override: Option<f32>) -> Vec<Box<dyn Movement>> {
    vec![
        Box::new(NoneMovement),
        Box::new(CircularMovement {
            radius: 5.0,
            speed: speed_override.unwrap_or(1.0),
            max_speed: 10.0,
        }),
        Box::new(RandomTeleportMovement {
            range: 20.0,
            max_speed: 10.0,
        }),
        Box::new(SquareMovement {
            size: 10.0,
            speed: speed_override.unwrap_or(0.5),
            max_speed: 10.0,
        }),
        Box::new(CrossMovement {
            arm: 8.0,
            speed: speed_override.unwrap_or(3.0),
            max_speed: 10.0,
        }),
        Box::new(ForwardMovement {
            arm: 8.0,
            speed: speed_override.unwrap_or(2.0),
            max_speed: 10.0,
        }),
        Box::new(TargetMovement {
            range: 15.0,
            speed: speed_override.unwrap_or(3.0),
            max_speed: 10.0,
            threshold: 0.15,
        }),
    ]
}

pub fn get_movement_by_name(name: &str, speed_override: Option<f32>) -> Option<Box<dyn Movement>> {
    let m: Box<dyn Movement> = match name {
        "none" => Box::new(NoneMovement),
        "circular" => Box::new(CircularMovement {
            radius: 5.0,
            speed: speed_override.unwrap_or(1.0),
            max_speed: 10.0,
        }),
        "rtp" => Box::new(RandomTeleportMovement {
            range: 20.0,
            max_speed: 10.0,
        }),
        "square" => Box::new(SquareMovement {
            size: 10.0,
            speed: speed_override.unwrap_or(0.5),
            max_speed: 10.0,
        }),
        "cross" => Box::new(CrossMovement {
            arm: 8.0,
            speed: speed_override.unwrap_or(3.0),
            max_speed: 10.0,
        }),
        "forward" => Box::new(ForwardMovement {
            arm: 8.0,
            speed: speed_override.unwrap_or(2.0),
            max_speed: 10.0,
        }),
        "target" => Box::new(TargetMovement {
            range: 15.0,
            speed: speed_override.unwrap_or(3.0),
            max_speed: 10.0,
            threshold: 0.15,
        }),
        _ => return None,
    };
    Some(m)
}

pub fn get_random_movement(speed_override: Option<f32>) -> Box<dyn Movement> {
    let names = ["circular", "rtp", "square", "cross", "forward", "target"];
    let index = rand::random::<usize>() % names.len();
    get_movement_by_name(names[index], speed_override).unwrap()
}

pub fn get_movement(name: &str, speed_override: Option<f32>) -> Box<dyn Movement> {
    if name == "random" {
        return get_random_movement(speed_override);
    }
    match get_movement_by_name(name, speed_override) {
        Some(m) => m,
        None => {
            let all = get_movements(None);
            let names: Vec<&str> = all.iter().map(|m| m.name()).collect();
            panic!("Unknown movement '{}'. Available: random, {}", name, names.join(", "));
        }
    }
}
