use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::auth::Credentials;

/// Configuration de l'utilisateur Nox
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoxConfig {
    pub server: String,
    pub servers: std::collections::HashMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub user_id: Option<u32>,
    pub gateway: Option<String>,
    #[serde(rename = "_token")]
    pub token: Option<String>,
}

/// Gestionnaire des credentials Nox
pub struct NoxCredentials;

impl NoxCredentials {
    /// Obtient le chemin du dossier .nox
    pub fn nox_folder(custom_path: Option<PathBuf>) -> PathBuf {
        if let Some(path) = custom_path {
            return path;
        }

        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());

        #[cfg(target_os = "macos")]
        let base_dir = format!("{}/Library/Application Support", home);

        #[cfg(target_os = "linux")]
        let base_dir = format!("{}/.local/share", home);

        #[cfg(target_os = "windows")]
        let base_dir =
            std::env::var("APPDATA").unwrap_or_else(|_| format!("{}\\AppData\\Roaming", home));

        PathBuf::from(base_dir).join(".nox")
    }

    /// Obtient le chemin du fichier de configuration
    pub fn config_path(custom_path: Option<PathBuf>) -> PathBuf {
        Self::nox_folder(custom_path).join("config.json")
    }

    /// Obtient le chemin de la clé publique
    pub fn public_key_path(custom_path: Option<PathBuf>) -> PathBuf {
        Self::nox_folder(custom_path).join("public_key.pem")
    }

    /// Obtient le chemin de la clé privée
    pub fn private_key_path(custom_path: Option<PathBuf>) -> PathBuf {
        Self::nox_folder(custom_path).join("private_key.pem")
    }

    /// Charge la configuration depuis config.json
    pub fn load_config(custom_path: Option<PathBuf>) -> Result<NoxConfig> {
        let path = Self::config_path(custom_path.clone());
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file at {:?}", path))?;
        serde_json::from_str(&content).with_context(|| "Failed to parse config.json")
    }

    /// Charge les credentials depuis le dossier .nox
    pub fn load(custom_path: Option<PathBuf>) -> Result<Credentials> {
        let config = Self::load_config(custom_path.clone())?;
        let server = config.server;

        let server_config = config
            .servers
            .get(&server)
            .with_context(|| format!("Server '{}' not found in config", server))?;

        let user_id = server_config
            .user_id
            .with_context(|| format!("Server '{}' has no user_id in config", server))?;

        let public_pem = std::fs::read_to_string(Self::public_key_path(custom_path.clone()))
            .context("Failed to read public key")?;
        let private_pem = std::fs::read_to_string(Self::private_key_path(custom_path))
            .context("Failed to read private key")?;

        Credentials::from_pem(user_id, server, &public_pem, &private_pem)
            .map_err(|e| anyhow::anyhow!("Failed to load credentials: {}", e))
    }

    /// Vérifie si les credentials existent
    pub fn exists(custom_path: Option<PathBuf>) -> bool {
        Self::config_path(custom_path.clone()).exists()
            && Self::public_key_path(custom_path.clone()).exists()
            && Self::private_key_path(custom_path).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nox_folder_path() {
        let path = NoxCredentials::nox_folder(None);
        assert!(path.to_str().unwrap().contains(".nox"));
    }
}
