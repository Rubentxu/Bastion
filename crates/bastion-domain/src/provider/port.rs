//! SandboxProvider port — the primary interface for sandbox backends.
//!
//! This trait follows the **Dependency Inversion Principle**: the domain defines
//! the interface, infrastructure adapters implement it.

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use super::capabilities::ProviderCapabilities;
use crate::execution::command::{CommandResult, CommandSpec};
use crate::execution::stream::CommandChunk;
use crate::file_ops::FileEntry;
use crate::sandbox::entity::Sandbox;
use crate::sandbox::snapshot::SnapshotInfo;
use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// Stream type for command output chunks.
pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// Universal port for sandbox providers.
///
/// Implementations: PodmanAdapter, FirecrackerAdapter, GVisorAdapter, KubernetesAdapter.
///
/// This trait follows Interface Segregation by splitting concerns into logical groups,
/// while keeping a unified interface for simplicity in the MVP.
///
/// When the `use-segregated-traits` feature is enabled (default), providers implement
/// `SandboxLifecycle + TaskExecutor` and receive a blanket `SandboxProvider` impl via
/// `provider/compat.rs`. This trait remains the canonical port interface for dependency
/// injection and dynamic dispatch.
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

    /// Cancel a running command in the sandbox.
    ///
    /// Sends SIGTERM to the command process group. If it doesn't exit within
    /// the grace period, sends SIGKILL.
    ///
    /// Returns true if the command was successfully cancelled, false if no
    /// running command was found for the sandbox.
    async fn cancel_command(
        &self,
        id: &SandboxId,
        grace_period_ms: u64,
    ) -> Result<bool, DomainError> {
        let _ = (id, grace_period_ms);
        Err(DomainError::UnsupportedOperation(
            "cancel_command".to_string(),
        ))
    }

    // ── File Operations ────────────────────────────────────────

    /// Write content to a file inside the sandbox.
    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError>;

    /// Read content from a file inside the sandbox.
    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError>;

    /// List files in a directory inside the sandbox.
    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError>;

    /// Copy a host directory into the sandbox at the given target path.
    ///
    /// For container-based providers (Docker/Podman), this uses put_archive.
    /// Other providers may use cp or return UnsupportedOperation.
    async fn copy_to(
        &self,
        _id: &SandboxId,
        _host_dir: &std::path::Path,
        _target: &str,
    ) -> Result<(), DomainError> {
        Err(DomainError::UnsupportedOperation("copy_to".to_string()))
    }

    // ── Snapshot Operations ─────────────────────────────────────

    /// Create a snapshot of the sandbox state.
    async fn create_snapshot(
        &self,
        _id: &SandboxId,
        _name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        Err(DomainError::UnsupportedOperation("snapshots".to_string()))
    }

    /// Restore a sandbox from a snapshot.
    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        Err(DomainError::UnsupportedOperation("snapshots".to_string()))
    }

    /// Check if a snapshot exists.
    async fn snapshot_exists(&self, _snapshot_id: &str) -> Result<bool, DomainError> {
        Err(DomainError::UnsupportedOperation("snapshots".to_string()))
    }

    /// Delete a snapshot (remove the image).
    async fn delete_snapshot(&self, _snapshot_id: &str) -> Result<(), DomainError> {
        Err(DomainError::UnsupportedOperation("snapshots".to_string()))
    }

    /// List all snapshots managed by this provider.
    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
        Err(DomainError::UnsupportedOperation("snapshots".to_string()))
    }

    // ── Metadata ───────────────────────────────────────────────

    /// Report what this provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    // ── Provider-scoped Operations ────────────────────────────────

    /// List sandboxes managed by this provider (not from repository).
    ///
    /// Returns sandboxes that are currently running or have recently
    /// been managed by this provider instance.
    async fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "list_sandboxes".to_string(),
        ))
    }

    /// Get current sandbox info from the backend provider.
    ///
    /// Returns the `Sandbox` entity with potentially updated status
    /// from the actual backend (e.g., Docker, Firecracker, gVisor).
    async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
        Err(DomainError::UnsupportedOperation("get_info".to_string()))
    }

    /// Extend or shorten the lifetime of a sandbox.
    ///
    /// Updates the `expires_at` field of the sandbox. The backend
    /// may enforce minimum/maximum timeout limits.
    async fn set_timeout(&self, _id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        Err(DomainError::UnsupportedOperation("set_timeout".to_string()))
    }
}
