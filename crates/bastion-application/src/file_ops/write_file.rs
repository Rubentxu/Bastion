//! Write file use case.

use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;
use std::sync::Arc;

pub struct WriteFileUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl WriteFileUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        sandbox_id: &SandboxId,
        path: &str,
        content: &[u8],
        provider: &dyn SandboxProvider,
    ) -> Result<(), DomainError> {
        let sandbox = self
            .repository
            .find_by_id(sandbox_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))?;

        if !sandbox.is_active() {
            return Err(DomainError::Validation(format!(
                "Sandbox {} is not active",
                sandbox_id
            )));
        }

        provider.write_file(sandbox_id, path, content).await
    }
}
