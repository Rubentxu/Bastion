//! No-op secret resolver for testing.
//!
//! Returns the key itself as the value with source "noop".

use async_trait::async_trait;

use bastion_domain::secret::{SecretResolver, SecretValue};
use bastion_domain::shared::DomainError;

/// Secret resolver that returns the key as value — for testing only.
#[derive(Debug, Clone, Default)]
pub struct NoopSecretResolver;

impl NoopSecretResolver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecretResolver for NoopSecretResolver {
    async fn resolve(&self, key: &str) -> Result<SecretValue, DomainError> {
        Ok(SecretValue::new(key.to_string(), "noop".to_string()))
    }
}
