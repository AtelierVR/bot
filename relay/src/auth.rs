use bytes::Bytes;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::buffer::Buffer;

/// Actions d'authentification
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAction {
    RequestChallenge = 0,
    ResolveChallenge = 1,
}

/// Résultats d'authentification
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthResult {
    Success = 0,
    Challenge = 1,
    MasterError = 2,
    Blacklisted = 3,
    Invalid = 4,
    Signature = 5,
    Unknown = 255,
}

impl TryFrom<u8> for AuthResult {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AuthResult::Success),
            1 => Ok(AuthResult::Challenge),
            2 => Ok(AuthResult::MasterError),
            3 => Ok(AuthResult::Blacklisted),
            4 => Ok(AuthResult::Invalid),
            5 => Ok(AuthResult::Signature),
            255 => Ok(AuthResult::Unknown),
            _ => Err(()),
        }
    }
}

/// Réponse d'authentification générique
#[derive(Debug, Clone)]
pub enum AuthResponse {
    Success {
        user_id: u32,
        address: String,
        display_name: String,
    },
    Challenge {
        challenge: Vec<u8>,
    },
    Error {
        result: AuthResult,
        reason: String,
        expire_at: Option<i64>,
    },
}

/// Credentials d'authentification avec clés RSA
#[derive(Clone)]
pub struct Credentials {
    pub user_id: u32,
    pub server: String,
    pub public_key: RsaPublicKey,
    pub private_key: RsaPrivateKey,
}

impl Credentials {
    /// Crée des credentials depuis des clés PEM
    pub fn from_pem(
        user_id: u32,
        server: String,
        public_pem: &str,
        private_pem: &str,
    ) -> Result<Self, String> {
        let public_key = RsaPublicKey::from_public_key_pem(public_pem)
            .map_err(|e| format!("Failed to parse public key: {}", e))?;

        // Essayer d'abord PKCS#1 (BEGIN RSA PRIVATE KEY), puis PKCS#8 (BEGIN PRIVATE KEY)
        let private_key = RsaPrivateKey::from_pkcs1_pem(private_pem)
            .or_else(|_| RsaPrivateKey::from_pkcs8_pem(private_pem))
            .map_err(|e| format!("Failed to parse private key: {}", e))?;

        Ok(Self {
            user_id,
            server,
            public_key,
            private_key,
        })
    }

    /// Charge les credentials depuis des fichiers
    pub fn from_files(
        user_id: u32,
        server: String,
        public_path: impl AsRef<Path>,
        private_path: impl AsRef<Path>,
    ) -> Result<Self, String> {
        let public_pem = std::fs::read_to_string(public_path)
            .map_err(|e| format!("Failed to read public key: {}", e))?;
        let private_pem = std::fs::read_to_string(private_path)
            .map_err(|e| format!("Failed to read private key: {}", e))?;
        Self::from_pem(user_id, server, &public_pem, &private_pem)
    }

    /// Génère une nouvelle paire de clés RSA
    pub fn generate(user_id: u32, server: String) -> Result<Self, String> {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048)
            .map_err(|e| format!("Failed to generate keys: {}", e))?;
        let public_key = RsaPublicKey::from(&private_key);

        Ok(Self {
            user_id,
            server,
            public_key,
            private_key,
        })
    }

    /// Export la clé publique au format DER (SubjectPublicKeyInfo)
    pub fn public_key_der(&self) -> Result<Vec<u8>, String> {
        use rsa::pkcs8::EncodePublicKey;
        let der = self
            .public_key
            .to_public_key_der()
            .map(|doc| doc.as_bytes().to_vec())
            .map_err(|e| format!("Failed to export public key: {}", e))?;

        tracing::debug!("[Auth] Exported public key: {} bytes", der.len());
        tracing::debug!(
            "[Auth] Public key (first 64 bytes, hex): {}",
            hex::encode(&der[..der.len().min(64)])
        );

        Ok(der)
    }

    /// Signe des données avec la clé privée (PKCS#1v15 SHA-256)
    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        use rsa::pkcs1v15::SigningKey;
        use rsa::signature::{SignatureEncoding, Signer};
        use sha2::Sha256;

        tracing::debug!("[Auth] Signing data: {} bytes", data.len());
        tracing::debug!("[Auth] Data to sign (hex): {}", hex::encode(data));

        // Créer un signing key avec SHA-256 (le hash sera fait automatiquement)
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        let signature = signing_key.sign(data);
        let signature_bytes = signature.to_vec();

        tracing::debug!(
            "[Auth] Generated signature: {} bytes",
            signature_bytes.len()
        );
        tracing::debug!(
            "[Auth] Signature (first 64 bytes, hex): {}",
            hex::encode(&signature_bytes[..signature_bytes.len().min(64)])
        );

        Ok(signature_bytes)
    }
}

/// Crée une requête pour demander un challenge
pub fn create_challenge_request() -> Vec<u8> {
    vec![AuthAction::RequestChallenge as u8]
}

/// Crée une requête pour résoudre un challenge
pub fn create_resolve_request(
    credentials: &Credentials,
    signature: &[u8],
) -> Result<Vec<u8>, String> {
    let mut buffer = Buffer::new();

    // Action
    buffer.write_u8(AuthAction::ResolveChallenge as u8);

    // Public key (DER format)
    let public_key_der = credentials.public_key_der()?;
    buffer.write_u16(public_key_der.len() as u16);
    buffer.write_bytes(&public_key_der);

    // Signature
    buffer.write_u16(signature.len() as u16);
    buffer.write_bytes(signature);

    // User info
    buffer.write_u32(credentials.user_id);
    buffer.write_string(&credentials.server);

    Ok(buffer.to_vec())
}

/// Parse une réponse d'authentification
pub fn parse_auth_response(data: &[u8]) -> Result<AuthResponse, String> {
    let mut buffer = Buffer::from_vec(data.to_vec());

    let result_code = buffer
        .read_u8()
        .map_err(|e| format!("Failed to read result code: {}", e))?;
    let result = AuthResult::try_from(result_code)
        .map_err(|_| format!("Invalid auth result code: {}", result_code))?;

    match result {
        AuthResult::Success => {
            let user_id = buffer
                .read_u32()
                .map_err(|e| format!("Failed to read user_id: {}", e))?;
            let address = buffer
                .read_string()
                .map_err(|e| format!("Failed to read address: {}", e))?;
            let display_name = buffer
                .read_string()
                .map_err(|e| format!("Failed to read display_name: {}", e))?;

            Ok(AuthResponse::Success {
                user_id,
                address,
                display_name,
            })
        }
        AuthResult::Challenge => {
            let challenge_len = buffer
                .read_u8()
                .map_err(|e| format!("Failed to read challenge length: {}", e))?
                as usize;
            let challenge = buffer
                .read_bytes(challenge_len)
                .map_err(|e| format!("Failed to read challenge: {}", e))?;

            Ok(AuthResponse::Challenge { challenge })
        }
        AuthResult::Blacklisted => {
            let expire_at = buffer
                .read_i64()
                .map_err(|e| format!("Failed to read expire_at: {}", e))?;
            let reason = if buffer.remaining() > 0 {
                buffer
                    .read_string()
                    .unwrap_or_else(|_| "Blacklisted".to_string())
            } else {
                "Blacklisted".to_string()
            };

            Ok(AuthResponse::Error {
                result,
                reason,
                expire_at: Some(expire_at),
            })
        }
        _ => {
            let reason = if buffer.remaining() > 0 {
                buffer.read_string().unwrap_or_default()
            } else {
                format!("{:?}", result)
            };

            Ok(AuthResponse::Error {
                result,
                reason,
                expire_at: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_generation() {
        let creds = Credentials::generate(123, "test.server".to_string()).unwrap();
        assert_eq!(creds.user_id, 123);
        assert_eq!(creds.server, "test.server");

        // Test signing
        let data = b"test data";
        let signature = creds.sign(data).unwrap();
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_challenge_request() {
        let request = create_challenge_request();
        assert_eq!(request, vec![AuthAction::RequestChallenge as u8]);
    }

    #[test]
    fn test_parse_challenge_response() {
        let mut buffer = Buffer::new();
        buffer.write_u8(AuthResult::Challenge as u8);
        buffer.write_u8(16); // challenge length
        buffer.write_bytes(&[0u8; 16]); // challenge data

        let response = parse_auth_response(&buffer.to_vec()).unwrap();
        match response {
            AuthResponse::Challenge { challenge } => {
                assert_eq!(challenge.len(), 16);
            }
            _ => panic!("Expected Challenge response"),
        }
    }
}
