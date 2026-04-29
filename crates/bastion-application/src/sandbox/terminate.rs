//! Terminate sandbox use case.

use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use std::sync::Arc;

pub struct TerminateSandboxUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl TerminateSandboxUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        sandbox_id: &SandboxId,
        provider: &dyn SandboxProvider,
    ) -> Result<(), DomainError> {
        tracing::info!(sandbox_id = %sandbox_id, "Terminating sandbox");

        let mut sandbox = self.repository
            .find_by_id(sandbox_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))?;

        provider.terminate(sandbox_id).await?;
        sandbox.terminate()?;
        self.repository.delete(sandbox_id).await?;

        tracing::info!(sandbox_id = %sandbox_id, "Sandbox terminated");
        Ok(())
    }
}
