use async_trait::async_trait;
use nox_relay::{Quaternion, RelayInstance, Transform, TransformRequest, TransformType, Vector3};

#[async_trait]
pub trait Movement: Send + Sync {
    fn name(&self) -> &str;
    #[allow(dead_code)]
    fn description(&self) -> &str;
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

pub struct CircularMovement {
    pub radius: f32,
    pub speed: f32,
}

#[async_trait]
impl Movement for CircularMovement {
    fn name(&self) -> &str {
        "circular"
    }

    fn description(&self) -> &str {
        "Mouvement circulaire"
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
    }
}

pub struct RandomTeleportMovement {
    pub range: f32,
}

#[async_trait]
impl Movement for RandomTeleportMovement {
    fn name(&self) -> &str {
        "rtp"
    }

    fn description(&self) -> &str {
        "Téléportation aléatoire"
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;

        if state.time > 2.0 {
            state.time = 0.0;
            state.position.x = (rand::random::<f32>() - 0.5) * self.range;
            state.position.z = (rand::random::<f32>() - 0.5) * self.range;

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
        }
    }
}

pub struct SquareMovement {
    pub size: f32,
    pub speed: f32,
}

#[async_trait]
impl Movement for SquareMovement {
    fn name(&self) -> &str {
        "square"
    }

    fn description(&self) -> &str {
        "Mouvement en carré"
    }

    fn initialize(&self, index: usize) -> MovementState {
        MovementState::new(index as f32, 0.0, 0.0, 0)
    }

    async fn update(&self, state: &mut MovementState, dt: f32, instance: &RelayInstance) {
        state.time += dt / 1000.0;
        let progress = (state.time * self.speed) % 4.0;

        let half_size = self.size / 2.0;
        match progress as i32 {
            0 => {
                state.position.x = -half_size + (progress.fract() * self.size);
                state.position.z = -half_size;
            }
            1 => {
                state.position.x = half_size;
                state.position.z = -half_size + ((progress - 1.0).fract() * self.size);
            }
            2 => {
                state.position.x = half_size - ((progress - 2.0).fract() * self.size);
                state.position.z = half_size;
            }
            _ => {
                state.position.x = -half_size;
                state.position.z = half_size - ((progress - 3.0).fract() * self.size);
            }
        }

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
    }
}

pub fn get_movements() -> Vec<Box<dyn Movement>> {
    vec![
        Box::new(CircularMovement {
            radius: 5.0,
            speed: 1.0,
        }),
        Box::new(RandomTeleportMovement { range: 20.0 }),
        Box::new(SquareMovement {
            size: 10.0,
            speed: 0.5,
        }),
    ]
}

pub fn get_random_movement() -> Box<dyn Movement> {
    let movements = get_movements();
    let index = rand::random::<usize>() % movements.len();
    movements.into_iter().nth(index).unwrap()
}
