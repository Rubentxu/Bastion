//! Delta sync infrastructure for sandbox file synchronization.
//!
//! Provides DeltaSyncBackend trait and implementations for bidirectional
//! file synchronization between host and sandbox.

pub mod tar_stream;
pub mod rsync;

pub use rsync::RsyncBackend;
pub use tar_stream::TarStreamBackend;

use std::path::Path;
use async_trait::async_trait;
use bastion_domain::shared::DomainError;

/// Progress event for sync operations.
#[derive(Debug, Clone)]
pub struct SyncProgress {
    /// Number of bytes transferred so far.
    pub bytes_transferred: u64,
    /// Total bytes to transfer.
    pub total_bytes: u64,
    /// Current file being transferred.
    pub current_file: Option<String>,
    /// Number of files transferred.
    pub files_transferred: u64,
    /// Total number of files.
    pub total_files: u64,
}

/// Sync direction.
#[derive(Debug, Clone, Copy)]
pub enum SyncDirection {
    Push, // Host to sandbox
    Pull, // Sandbox to host
}

/// DeltaSyncBackend trait for file synchronization backends.
#[async_trait]
pub trait DeltaSyncBackend: Send + Sync {
    /// Perform a sync operation.
    async fn sync(
        &self,
        source: &Path,
        target: &Path,
        exclude: &[String],
    ) -> Result<u64, DomainError>;

    /// Get the backend name.
    fn name(&self) -> &'static str;

    /// Check if this backend is available (e.g., rsync binary exists).
    async fn is_available(&self) -> bool;
}
