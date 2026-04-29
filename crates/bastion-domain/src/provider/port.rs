//! SandboxProvider port — the primary interface for sandbox backends.
//!
//! This trait follows the **Dependency Inversion Principle**: the domain defines
//! the interface, infrastructure adapters implement it.

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use crate::execution::command::{CommandResult, CommandSpec};
use crate::execution::stream::CommandChunk;
use crate::file_ops::FileEntry;
use crate::sandbox::entity::Sandbox;
use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use crate::shared::DomainError;
use crate::shared::id::SandboxId;
use super::capabilities::ProviderCapabilities;

/// Stream type for command output chunks.
pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// Universal port for sandbox providers.
///
/// Implementations: PodmanAdapter, FirecrackerAdapter, GVisorAdapter, KubernetesAdapter.
///
/// This trait follows Interface Segregation by splitting concerns into logical groups,
/// while keeping a unified interface for simplicity in the MVP.
#[async_trait]
pub trait SandboxProvider: Send + Sync + std::fmt::Debug {
    // ── Lifecycle ──────────────────────────────────────────────

    /// Create a new isolated sandbox.
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        resources: &ResourcesSpec,
        network: &NetworkSpec,
        env_vars: &std::collections::HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError>;

    /// Terminate and clean up a sandbox.
    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError>;

    /// Check if a sandbox is alive.
    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError>;

    // ── Execution ──────────────────────────────────────────────

    /// Execute a command and wait for completion.
    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError>;

    /// Execute a command with streaming output.
    async fn run_command_stream(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandStream, DomainError>;

    // ── File Operations ────────────────────────────────────────

    /// Write content to a file inside the sandbox.
    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError>;

    /// Read content from a file inside the sandbox.
    async fn read_file(
        &self,
        id: &SandboxId,
        path: &str,
    ) -> Result<Vec<u8>, DomainError>;

    /// List files in a directory inside the sandbox.
    async fn list_files(
        &self,
        id: &SandboxId,
        dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError>;

    // ── Metadata ───────────────────────────────────────────────

    /// Report what this provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Human-readable provider name.
    fn name(&self) -> &str;
}
