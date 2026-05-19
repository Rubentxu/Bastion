//! Snapshot port — sandbox state snapshots.
//!
//! This trait focuses on creating, restoring, and managing sandbox snapshots.

use async_trait::async_trait;

use crate::sandbox::entity::Sandbox;
use crate::sandbox::snapshot::SnapshotInfo;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// Snapshot port — manages sandbox state snapshots.
///
/// Implementors: PodmanProvider, DockerProvider (for container snapshot support).
///
/// ## Design
///
/// This port handles snapshot lifecycle: creating snapshots, restoring from them,
/// checking existence, deletion, and listing. Lifecycle operations (create sandbox,
/// terminate) are handled by `LifecyclePort`.
#[async_trait]
pub trait SnapshotPort: Send + Sync + std::fmt::Debug {
    /// Create a snapshot of the sandbox state.
    async fn create_snapshot(
        &self,
        id: &SandboxId,
        name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        let _ = (id, name);
        Err(DomainError::UnsupportedOperation(
            "create_snapshot".to_string(),
        ))
    }

    /// Restore a sandbox from a snapshot.
    async fn restore_snapshot(&self, snapshot_id: &str) -> Result<Sandbox, DomainError> {
        let _ = snapshot_id;
        Err(DomainError::UnsupportedOperation(
            "restore_snapshot".to_string(),
        ))
    }

    /// Check if a snapshot exists.
    async fn snapshot_exists(&self, snapshot_id: &str) -> Result<bool, DomainError> {
        let _ = snapshot_id;
        Err(DomainError::UnsupportedOperation(
            "snapshot_exists".to_string(),
        ))
    }

    /// Delete a snapshot (remove the image).
    async fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), DomainError> {
        let _ = snapshot_id;
        Err(DomainError::UnsupportedOperation(
            "delete_snapshot".to_string(),
        ))
    }

    /// List all snapshots managed by this provider.
    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "list_snapshots".to_string(),
        ))
    }
}
