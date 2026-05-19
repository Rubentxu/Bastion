//! Secret source types for provider secrets.
//!
//! Describes how secrets are provided to provider instances.

use serde::{Deserialize, Serialize};

use super::image_reference::SecretValue;

/// Source of a secret for provider instances.
///
/// Describes where sensitive data (passwords, tokens, keys) comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretSource {
    /// Secret stored as an environment variable.
    Env {
        /// Name of the environment variable.
        name: String,
        /// The secret value.
        value: SecretValue,
    },
    /// Secret stored as a file.
    File {
        /// Path where the secret file will be mounted.
        path: String,
        /// The secret content.
        value: SecretValue,
        /// Permissions for the file (defaults to 0o600).
        #[serde(default = "default_file_mode")]
        mode: u32,
    },
    /// Secret from a Kubernetes secret.
    Kubernetes {
        /// Name of the Kubernetes secret.
        secret_name: String,
        /// Key within the secret.
        key: String,
    },
    /// Secret from AWS Secrets Manager.
    AwsSecretsManager {
        /// ARN or name of the secret.
        arn: String,
        /// Optional version ID or staging label.
        version_id: Option<String>,
        /// Optional specific key within the secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// Secret from HashiCorp Vault.
    Vault {
        /// Vault address URL.
        address: String,
        /// Path to the secret in Vault.
        path: String,
        /// Key within the secret.
        key: String,
        /// Optional Vault authentication method.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_method: Option<String>,
    },
}

fn default_file_mode() -> u32 {
    0o600
}

impl SecretSource {
    /// Create an environment variable secret source.
    pub fn env(name: impl Into<String>, value: SecretValue) -> Self {
        Self::Env {
            name: name.into(),
            value,
        }
    }

    /// Create a file-based secret source.
    pub fn file(path: impl Into<String>, value: SecretValue) -> Self {
        Self::File {
            path: path.into(),
            value,
            mode: default_file_mode(),
        }
    }

    /// Create a Kubernetes secret source.
    pub fn kubernetes(secret_name: impl Into<String>, key: impl Into<String>) -> Self {
        Self::Kubernetes {
            secret_name: secret_name.into(),
            key: key.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_source_env() {
        let source = SecretSource::env("API_KEY", SecretValue::plain_text("secret123"));
        assert!(matches!(source, SecretSource::Env { .. }));
    }

    #[test]
    fn test_secret_source_file() {
        let source = SecretSource::file("/secrets/api-key", SecretValue::plain_text("secret123"));
        assert!(matches!(source, SecretSource::File { .. }));
    }

    #[test]
    fn test_secret_source_kubernetes() {
        let source = SecretSource::kubernetes("my-secret", "api-key");
        assert!(matches!(source, SecretSource::Kubernetes { .. }));
    }

    #[test]
    fn test_secret_source_serde_env() {
        let source = SecretSource::env("API_KEY", SecretValue::plain_text("secret"));
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"env\""));
        let parsed: SecretSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SecretSource::Env { .. }));
    }

    #[test]
    fn test_secret_source_serde_file() {
        let source = SecretSource::file("/secrets/key", SecretValue::plain_text("secret"));
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"file\""));
        let parsed: SecretSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SecretSource::File { .. }));
    }

    #[test]
    fn test_secret_source_serde_kubernetes() {
        let source = SecretSource::kubernetes("my-secret", "api-key");
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"kubernetes\""));
        let parsed: SecretSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SecretSource::Kubernetes { .. }));
    }

    #[test]
    fn test_secret_source_serde_aws_secrets_manager() {
        let source = SecretSource::AwsSecretsManager {
            arn: "arn:aws:secretsmanager:us-east-1:123456789:secret:my-secret".to_string(),
            version_id: None,
            key: None,
        };
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"aws_secrets_manager\""));
        let parsed: SecretSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SecretSource::AwsSecretsManager { .. }));
    }
}
