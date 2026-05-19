//! Metadata port — provider capabilities and sandbox metadata.
//!
//! This trait focuses on provider capabilities, naming, and sandbox metadata queries.

use async_trait::async_trait;

use crate::provider::capabilities::ProviderCapabilities;
use crate::sandbox::entity::Sandbox;
use crate::sandbox::value_objects::SandboxFilter;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// Metadata port — provider capabilities and sandbox metadata.
///
/// Implementors: All providers.
///
/// ## Design
///
/// This port handles metadata concerns: what capabilities the provider has,
/// its human-readable name, and queries for sandbox information. Lifecycle
/// operations are handled by `LifecyclePort`.
#[async_trait]
pub trait MetadataPort: Send + Sync + std::fmt::Debug {
    /// Report what this provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// List sandboxes managed by this provider (not from repository).
    ///
    /// Returns sandboxes that are currently running or have recently
    /// been managed by this provider instance.
    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        let _ = filter;
        Err(DomainError::UnsupportedOperation(
            "list_sandboxes".to_string(),
        ))
    }

    /// Get current sandbox info from the backend provider.
    ///
    /// Returns the `Sandbox` entity with potentially updated status
    /// from the actual backend (e.g., Docker, Firecracker, gVisor).
    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        let _ = id;
        Err(DomainError::UnsupportedOperation("get_info".to_string()))
    }

    /// Extend or shorten the lifetime of a sandbox.
    ///
    /// Updates the `expires_at` field of the sandbox. The backend
    /// may enforce minimum/maximum timeout limits.
    async fn set_timeout(&self, id: &SandboxId, timeout_ms: u64) -> Result<(), DomainError> {
        let _ = (id, timeout_ms);
        Err(DomainError::UnsupportedOperation("set_timeout".to_string()))
    }
}
