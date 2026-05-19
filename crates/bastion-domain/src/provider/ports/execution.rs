//! Execution port — command execution and streaming.
//!
//! This trait focuses on running commands within sandboxes and managing
//! their lifecycle (cancellation).

use async_trait::async_trait;
use std::pin::Pin;

use crate::execution::command::{CommandResult, CommandSpec};
use crate::execution::stream::CommandChunk;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;
use futures::Stream;

/// Stream type for command output chunks.
pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// Execution port — manages command execution and cancellation.
///
/// Implementors: PodmanProvider, DockerProvider, FirecrackerProvider, GVisorProvider.
///
/// ## Design
///
/// This port handles only the execution concerns: running commands synchronously,
/// running commands with streaming output, and cancelling running commands.
/// File operations are handled by `FilePort`.
#[async_trait]
pub trait ExecutionPort: Send + Sync + std::fmt::Debug {
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
}
