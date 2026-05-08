//! Environment-variable-backed secret resolver.
//!
//! Reads secrets from process environment variables.

use async_trait::async_trait;

use bastion_domain::secret::{SecretResolver, SecretValue};
use bastion_domain::shared::DomainError;

/// Secret resolver that reads from environment variables.
#[derive(Debug, Clone, Default)]
pub struct EnvSecretResolver;

impl EnvSecretResolver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecretResolver for EnvSecretResolver {
    async fn resolve(&self, key: &str) -> Result<SecretValue, DomainError> {
        std::env::var(key)
            .map(|value| SecretValue::new(value, format!("env:{}", key)))
            .map_err(|_| DomainError::Config(format!("Secret '{}' not found in environment", key)))
    }
}
