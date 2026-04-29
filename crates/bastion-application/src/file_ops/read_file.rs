//! Read file use case.

use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use std::sync::Arc;

pub struct ReadFileUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl ReadFileUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        sandbox_id: &SandboxId,
        path: &str,
        provider: &dyn SandboxProvider,
    ) -> Result<Vec<u8>, DomainError> {
        let sandbox = self.repository
            .find_by_id(sandbox_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))?;

        if !sandbox.is_active() {
            return Err(DomainError::Validation(format!(
                "Sandbox {} is not active", sandbox_id
            )));
        }

        provider.read_file(sandbox_id, path).await
    }
}
