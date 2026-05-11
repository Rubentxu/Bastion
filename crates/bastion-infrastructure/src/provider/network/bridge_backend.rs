//! Bridge network backend — not yet implemented.

use async_trait::async_trait;

use bastion_domain::provider::network::{NetworkBackend, NetworkConfig, NetworkKind};
use bastion_domain::sandbox::value_objects::NetworkSpec;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Bridge network backend placeholder.
///
/// This backend is not yet implemented. It would create a bridge device
/// and connect VMs/containers to it for cross-sandbox networking.
#[derive(Debug, Default)]
pub struct BridgeBackend;

impl BridgeBackend {
    /// Create a new bridge backend.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl NetworkBackend for BridgeBackend {
    async fn setup(
        &self,
        _sandbox_id: &SandboxId,
        _network_spec: &NetworkSpec,
    ) -> Result<NetworkConfig, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "BridgeBackend not yet implemented".into(),
        ))
    }

    async fn teardown(&self, _sandbox_id: &SandboxId) -> Result<(), DomainError> {
        Err(DomainError::UnsupportedOperation(
            "BridgeBackend not yet implemented".into(),
        ))
    }

    fn kind(&self) -> NetworkKind {
        NetworkKind::Bridge
    }
}
