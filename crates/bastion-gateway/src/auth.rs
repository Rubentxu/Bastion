//! JWT Session Management
//!
//! Handles issuance and verification of JWT tokens for worker sessions.

use anyhow::{Context, Result};
use std::path::PathBuf;

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// JWT claims for worker sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct SessionClaims {
    /// Subject (sandbox_id)
    pub sub: String,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Expiration (Unix timestamp, 1 hour from issue)
    pub exp: i64,
    /// Capability hash (optional, for future use)
    pub cap_hash: Option<String>,
    /// JWT ID (unique identifier for this token)
    pub jti: String,
}

/// JWT Manager for issuing and verifying session tokens
#[derive(Clone)]
pub struct JwtManager {
    #[allow(dead_code)]
    encoding_key: EncodingKey,
    #[allow(dead_code)]
    decoding_key: DecodingKey,
}

impl JwtManager {
    /// Initialize or load the JWT manager from the given base directory.
    /// Generates a new HMAC key if one doesn't exist.
    pub fn init_or_load(base_dir: &PathBuf) -> Result<Self> {
        let key_path = base_dir.join("jwt-secret.key");

        let secret = if key_path.exists() {
            std::fs::read_to_string(&key_path).context("Failed to read JWT secret")?
        } else {
            // Generate a new 256-bit HMAC key
            let secret = generate_hmac_key();
            std::fs::create_dir_all(base_dir).context("Failed to create JWT directory")?;
            std::fs::write(&key_path, &secret).context("Failed to write JWT secret")?;
            // Set restrictive permissions on the key file
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&key_path, perms)?;
            }
            secret
        };

        Ok(Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
        })
    }

    /// Issue a new JWT token for a sandbox
    #[allow(dead_code)]
    pub fn issue(&self, sandbox_id: &str, cap_hash: Option<String>) -> Result<String> {
        let now = OffsetDateTime::now_utc();
        let claims = SessionClaims {
            sub: sandbox_id.to_string(),
            iat: now.unix_timestamp(),
            exp: (now + Duration::hours(1)).unix_timestamp(),
            cap_hash,
            jti: Uuid::new_v4().to_string(),
        };

        let token = encode(&Header::default(), &claims, &self.encoding_key)
            .context("Failed to encode JWT")?;

        Ok(token)
    }

    /// Verify a JWT token and return the claims if valid
    #[allow(dead_code)]
    pub fn verify(&self, token: &str) -> Result<SessionClaims> {
        let validation = Validation::default();
        let token_data = decode::<SessionClaims>(token, &self.decoding_key, &validation)
            .context("Failed to verify JWT")?;
        Ok(token_data.claims)
    }
}

/// Generate a 256-bit HMAC key
fn generate_hmac_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Use sha2 to hash the timestamp with some random bytes
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(ts.to_le_bytes());
    hasher.update(rand_bytes());
    let result = hasher.finalize();

    hex::encode(result)
}

fn rand_bytes() -> [u8; 32] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((ts.wrapping_mul((i as u64) + 1)) >> (i % 8)) as u8;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_issue_and_verify() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = JwtManager::init_or_load(&temp_dir.path().to_path_buf()).unwrap();

        let token = manager.issue("test-sandbox", None).unwrap();
        assert!(!token.is_empty());

        let claims = manager.verify(&token).unwrap();
        assert_eq!(claims.sub, "test-sandbox");
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn test_jwt_with_cap_hash() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = JwtManager::init_or_load(&temp_dir.path().to_path_buf()).unwrap();

        let token = manager
            .issue("test-sandbox", Some("abc123".to_string()))
            .unwrap();
        let claims = manager.verify(&token).unwrap();

        assert_eq!(claims.sub, "test-sandbox");
        assert_eq!(claims.cap_hash, Some("abc123".to_string()));
    }

    #[test]
    fn test_jwt_invalid_token() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = JwtManager::init_or_load(&temp_dir.path().to_path_buf()).unwrap();

        let result = manager.verify("invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_jwt_different_keys() {
        let temp_dir1 = tempfile::tempdir().unwrap();
        let temp_dir2 = tempfile::tempdir().unwrap();

        let manager1 = JwtManager::init_or_load(&temp_dir1.path().to_path_buf()).unwrap();
        let manager2 = JwtManager::init_or_load(&temp_dir2.path().to_path_buf()).unwrap();

        let token = manager1.issue("test-sandbox", None).unwrap();
        let result = manager2.verify(&token);
        assert!(result.is_err()); // Different key should fail verification
    }
}
