//! List files use case.

use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;
use std::sync::Arc;

pub struct ListFilesUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl ListFilesUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        sandbox_id: &SandboxId,
        dir: &str,
        provider: &dyn SandboxProvider,
    ) -> Result<Vec<FileEntry>, DomainError> {
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

        provider.list_files(sandbox_id, dir).await
    }
}
