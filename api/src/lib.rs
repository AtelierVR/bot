use serde::{Deserialize, Serialize};
use tracing::debug;

pub mod node_discovery;

#[derive(Debug, Clone)]
pub struct Nox {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoxResponse<T> {
    pub data: Option<T>,
    pub error: Option<NoxError>,
    pub time: i64,
    pub request: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoxError {
    pub code: i32,
    pub message: String,
    pub status: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u32,
    pub username: String,
    #[serde(rename = "display")]
    pub display_name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: u32,
    pub name: String,
    pub capacity: u16,
    pub client_count: u16,
    pub connection: ConnectionInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub method: String,
    pub data: String,
}

impl Nox {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Create a new Nox client with automatic gateway discovery
    ///
    /// This will attempt to discover the node gateway from the provided address
    /// using DNS TXT records and .well-known endpoints.
    ///
    /// # Example
    /// ```no_run
    /// use noxapi::Nox;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = Nox::with_discovery("example.com").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn with_discovery(address: impl AsRef<str>) -> anyhow::Result<Self> {
        let discovered = node_discovery::find_node_gateway(address.as_ref()).await?;
        Ok(Self::new(discovered))
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    pub async fn get_user_by_username(&self, username: &str) -> NoxResponse<User> {
        let url = format!(
            "{}/api/users/{}",
            self.base_url.trim_end_matches('/'),
            username
        );
        debug!("Fetching user from: {}", url);

        match self.client.get(&url).send().await {
            Ok(response) => response.json().await.unwrap_or_else(|e| {
                debug!("Failed to parse response: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Parse error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url.clone(),
                }
            }),
            Err(e) => {
                debug!("Request failed: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Network error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url,
                }
            }
        }
    }

    pub async fn get_me(&self, token: &str) -> NoxResponse<User> {
        let url = format!(
            "{}/api/users/@me",
            self.base_url.trim_end_matches('/')
        );
        debug!("Fetching current user from: {}", url);

        match self.client.get(&url).bearer_auth(token).send().await {
            Ok(response) => response.json().await.unwrap_or_else(|e| {
                debug!("Failed to parse response: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Parse error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url.clone(),
                }
            }),
            Err(e) => {
                debug!("Request failed: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Network error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url,
                }
            }
        }
    }

    pub async fn get_user_by_id(&self, id: u32, token: Option<&str>) -> NoxResponse<User> {
        let url = format!(
            "{}/api/users/{}",
            self.base_url.trim_end_matches('/'),
            id
        );
        debug!("Fetching user by id from: {}", url);

        let mut req = self.client.get(&url);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => response.json().await.unwrap_or_else(|e| {
                debug!("Failed to parse response: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Parse error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url.clone(),
                }
            }),
            Err(e) => {
                debug!("Request failed: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Network error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url,
                }
            }
        }
    }

    pub async fn get_instance_by_id(&self, id: u32) -> NoxResponse<Instance> {
        let url = format!(
            "{}/api/instances/{}",
            self.base_url.trim_end_matches('/'),
            id
        );
        debug!("Fetching instance from: {}", url);

        match self.client.get(&url).send().await {
            Ok(response) => response.json().await.unwrap_or_else(|e| {
                debug!("Failed to parse response: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Parse error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url.clone(),
                }
            }),
            Err(e) => {
                debug!("Request failed: {}", e);
                NoxResponse {
                    data: None,
                    error: Some(NoxError {
                        code: -1,
                        message: format!("Network error: {}", e),
                        status: 500,
                    }),
                    time: Self::now(),
                    request: url,
                }
            }
        }
    }
}
