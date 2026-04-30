//! Run command with streaming output use case.

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::port::CommandStream;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use std::sync::Arc;

pub struct RunCommandStreamUseCase {
    repository: Arc<dyn SandboxRepository>,
}

impl RunCommandStreamUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        sandbox_id: &SandboxId,
        command: &CommandSpec,
        provider: &dyn SandboxProvider,
    ) -> Result<CommandStream, DomainError> {
        let sandbox = self
            .repository
            .find_by_id(sandbox_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))?;

        if !sandbox.is_active() {
            return Err(DomainError::Validation(format!(
                "Sandbox {} is not active (status: {})",
                sandbox_id, sandbox.status
            )));
        }

        tracing::info!(
            sandbox_id = %sandbox_id,
            command = %command.command,
            "Executing streaming command"
        );

        provider.run_command_stream(sandbox_id, command).await
    }
}
