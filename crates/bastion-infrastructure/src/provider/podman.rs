//! Podman provider adapter.
//!
//! Implements SandboxProvider for Podman containers.
//! Uses the Podman API via Unix socket for container lifecycle management.

use async_trait::async_trait;
use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use std::collections::HashMap;

/// Podman-based sandbox provider (stub — implementation pending).
#[allow(dead_code)]
#[derive(Debug)]
pub struct PodmanProvider {
    socket_path: String,
    default_image: String,
}

impl PodmanProvider {
    pub fn new(socket_path: impl Into<String>, default_image: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            default_image: default_image.into(),
        }
    }
}

#[async_trait]
impl SandboxProvider for PodmanProvider {
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        resources: &ResourcesSpec,
        network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        tracing::info!(
            sandbox_id = %id,
            template = %template,
            "Creating Podman sandbox (stub)"
        );

        // TODO: Implement Podman container creation via API
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("podman"),
            resources.clone(),
            network.clone(),
        );
        sandbox.set_timeout(timeout_ms);
        sandbox.mark_running()?;

        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        tracing::info!(sandbox_id = %id, "Terminating Podman sandbox (stub)");
        // TODO: Implement Podman container removal
        Ok(())
    }

    async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
        // TODO: Implement Podman container inspection
        Ok(true)
    }

    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Running command via Podman (stub)"
        );

        // TODO: Implement gRPC call to worker inside container
        Ok(CommandResult::success(
            format!("Executed: {} (stub)\n", command.command).into_bytes()
        ))
    }

    async fn run_command_stream(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        // TODO: Implement streaming via gRPC
        Err(DomainError::UnsupportedOperation("streaming not yet implemented".to_string()))
    }

    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        tracing::info!(
            sandbox_id = %id,
            path = %path,
            size = content.len(),
            "Writing file via Podman (stub)"
        );
        // TODO: Implement gRPC WriteFile
        Ok(())
    }

    async fn read_file(
        &self,
        id: &SandboxId,
        path: &str,
    ) -> Result<Vec<u8>, DomainError> {
        tracing::info!(sandbox_id = %id, path = %path, "Reading file via Podman (stub)");
        // TODO: Implement gRPC ReadFile
        Ok(vec![])
    }

    async fn list_files(
        &self,
        id: &SandboxId,
        dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        tracing::info!(sandbox_id = %id, dir = %dir, "Listing files via Podman (stub)");
        // TODO: Implement gRPC ListFiles
        Ok(vec![])
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 16_384,
            max_cpu_count: 16,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 1500,
        }
    }

    fn name(&self) -> &str {
        "podman"
    }
}
