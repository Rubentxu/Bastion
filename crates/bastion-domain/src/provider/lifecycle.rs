//! Sandbox lifecycle trait — manages creation, termination, and health checks.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::sandbox::entity::Sandbox;
use crate::sandbox::snapshot::SnapshotInfo;
use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
use crate::shared::DomainError;
use crate::shared::id::SandboxId;
use crate::provider::capabilities::ProviderCapabilities;

/// Unsupported operation helper
fn unsupported(op: &str) -> DomainError {
    DomainError::UnsupportedOperation(op.to_string())
}

/// Sandbox lifecycle trait — manages creation, termination, health checks, and metadata.
///
/// This trait separates sandbox lifecycle concerns from task execution, allowing
/// providers to implement only the functionality they support.
#[async_trait]
pub trait SandboxLifecycle: Send + Sync + std::fmt::Debug {
    // ── Lifecycle ──────────────────────────────────────────────

    /// Create a new isolated sandbox.
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        resources: &ResourcesSpec,
        network: &NetworkSpec,
        env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError>;

    /// Terminate and clean up a sandbox.
    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError>;

    /// Check if a sandbox is alive.
    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError>;

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
    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError>;

    /// Get current sandbox info from the backend provider.
    ///
    /// Returns the `Sandbox` entity with potentially updated status
    /// from the actual backend (e.g., Docker, Firecracker, gVisor).
    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError>;

    /// Extend or shorten the lifetime of a sandbox.
    ///
    /// Updates the `expires_at` field of the sandbox. The backend
    /// may enforce minimum/maximum timeout limits.
    async fn set_timeout(&self, id: &SandboxId, timeout_ms: u64) -> Result<(), DomainError>;

    // ── Snapshot Operations (defaults: unsupported) ─────────────

    /// Create a snapshot of the sandbox state.
    async fn create_snapshot(&self, id: &SandboxId, name: &str) -> Result<SnapshotInfo, DomainError> {
        let _ = (id, name);
        Err(unsupported("create_snapshot"))
    }

    /// Restore a sandbox from a snapshot.
    async fn restore_snapshot(&self, snapshot_id: &str) -> Result<Sandbox, DomainError> {
        let _ = snapshot_id;
        Err(unsupported("restore_snapshot"))
    }

    /// Check if a snapshot exists.
    async fn snapshot_exists(&self, snapshot_id: &str) -> Result<bool, DomainError> {
        let _ = snapshot_id;
        Err(unsupported("snapshot_exists"))
    }

    /// Delete a snapshot (remove the image).
    async fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), DomainError> {
        let _ = snapshot_id;
        Err(unsupported("delete_snapshot"))
    }

    /// List all snapshots managed by this provider.
    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
        Err(unsupported("list_snapshots"))
    }
}
