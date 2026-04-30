//! In-memory sandbox repository for PoC and testing.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// In-memory implementation of SandboxRepository.
/// Suitable for PoC and testing. NOT for production.
#[derive(Debug, Default)]
pub struct InMemorySandboxRepository {
    sandboxes: Arc<RwLock<HashMap<String, Sandbox>>>,
}

impl InMemorySandboxRepository {
    pub fn new() -> Self {
        Self {
            sandboxes: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl SandboxRepository for InMemorySandboxRepository {
    async fn save(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
        let mut sandboxes = self.sandboxes.write().await;
        sandboxes.insert(sandbox.id.to_string(), sandbox.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: &SandboxId) -> Result<Option<Sandbox>, DomainError> {
        let sandboxes = self.sandboxes.read().await;
        Ok(sandboxes.get(&id.to_string()).cloned())
    }

    async fn update(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
        let mut sandboxes = self.sandboxes.write().await;
        sandboxes.insert(sandbox.id.to_string(), sandbox.clone());
        Ok(())
    }

    async fn delete(&self, id: &SandboxId) -> Result<(), DomainError> {
        let mut sandboxes = self.sandboxes.write().await;
        sandboxes
            .remove(&id.to_string())
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;
        Ok(())
    }

    async fn find_active(&self) -> Result<Vec<Sandbox>, DomainError> {
        let sandboxes = self.sandboxes.read().await;
        Ok(sandboxes.values().cloned().collect())
    }
}
