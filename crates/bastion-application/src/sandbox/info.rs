//! Get sandbox info use case.

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use bastion_domain::sandbox::repository::SandboxRepository;
use std::sync::Arc;

pub struct GetSandboxInfoUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl GetSandboxInfoUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(&self, sandbox_id: &SandboxId) -> Result<Sandbox, DomainError> {
        self.repository
            .find_by_id(sandbox_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))
    }
}
