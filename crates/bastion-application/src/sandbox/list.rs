//! List sandboxes use case.

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::DomainError;
use std::sync::Arc;

pub struct ListSandboxesUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl ListSandboxesUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(&self) -> Result<Vec<Sandbox>, DomainError> {
        self.repository.find_active().await
    }
}
