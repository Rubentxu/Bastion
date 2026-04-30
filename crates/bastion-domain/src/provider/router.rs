//! Command routing trait for registry-based command execution.
//!
//! This trait breaks the dependency cycle between bastion-infrastructure (PodmanProvider)
//! and bastion-gateway (RegistryService). The domain defines the interface.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::execution::command::CommandResult;
use crate::file_ops::FileEntry;
use crate::shared::DomainError;

/// Trait for routing commands to sandbox workers via the registry.
///
/// Implemented by the gateway's RegistryService, used by providers like PodmanProvider.
/// This allows infrastructure providers to route commands through the worker registry
/// instead of using exec-based fallback.
#[async_trait]
pub trait CommandRouter: Send + Sync + std::fmt::Debug {
    /// Execute a command in a sandbox via the worker registry.
    async fn route_run_command(
        &self,
        sandbox_id: &str,
        command: &str,
        args: &[String],
        working_dir: &str,
        env: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<CommandResult, DomainError>;

    /// Write a file to a sandbox via the worker registry.
    async fn route_write_file(
        &self,
        sandbox_id: &str,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError>;

    /// Read a file from a sandbox via the worker registry.
    async fn route_read_file(
        &self,
        sandbox_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, DomainError>;

    /// List files in a sandbox directory via the worker registry.
    async fn route_list_files(
        &self,
        sandbox_id: &str,
        directory: &str,
    ) -> Result<Vec<FileEntry>, DomainError>;

    /// Set the secret for a sandbox (so the registry can verify workers).
    fn set_sandbox_secret(&self, sandbox_id: &str, secret: &str);

    /// Check if a worker is connected for a sandbox.
    fn is_worker_connected(&self, sandbox_id: &str) -> bool;
}