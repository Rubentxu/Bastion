//! Task executor trait — manages command execution and file operations.

use async_trait::async_trait;
use std::path::Path;
use std::pin::Pin;

use crate::execution::command::{CommandResult, CommandSpec};
use crate::execution::stream::CommandChunk;
use crate::file_ops::FileEntry;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;
use futures::Stream;

/// Stream type for command output chunks.
pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// Unsupported operation helper
fn unsupported(op: &str) -> DomainError {
    DomainError::UnsupportedOperation(op.to_string())
}

/// Task executor trait — manages command execution, streaming, and file operations.
///
/// This trait separates execution concerns from lifecycle management, allowing
/// providers to implement only the execution functionality they support.
#[async_trait]
pub trait TaskExecutor: Send + Sync + std::fmt::Debug {
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
        Err(unsupported("cancel_command"))
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
        id: &SandboxId,
        host_dir: &Path,
        target: &str,
    ) -> Result<(), DomainError> {
        let _ = (id, host_dir, target);
        Err(unsupported("copy_to"))
    }
}
