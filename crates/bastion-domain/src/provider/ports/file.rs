//! File port — file operations within sandboxes.
//!
//! This trait focuses on reading, writing, and listing files inside sandboxes.

use async_trait::async_trait;
use std::path::Path;

use crate::file_ops::FileEntry;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// File port — manages file operations within sandboxes.
///
/// Implementors: PodmanProvider, DockerProvider, FirecrackerProvider, GVisorProvider.
///
/// ## Design
///
/// This port handles only file operations: reading, writing, listing, and copying.
/// Command execution is handled by `ExecutionPort`.
#[async_trait]
pub trait FilePort: Send + Sync + std::fmt::Debug {
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
        Err(DomainError::UnsupportedOperation("copy_to".to_string()))
    }
}
