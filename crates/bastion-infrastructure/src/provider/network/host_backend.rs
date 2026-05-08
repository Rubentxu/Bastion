//! Host network backend — container uses host networking directly.

use async_trait::async_trait;

use bastion_domain::provider::network::{NetworkBackend, NetworkConfig, NetworkKind};
use bastion_domain::sandbox::value_objects::NetworkSpec;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

/// Host network backend — no network abstraction needed.
///
/// Containers using host networking (e.g., `--network=host`) use the host's
/// network namespace directly, so no setup or teardown is required.
#[derive(Debug, Default)]
pub struct HostBackend;

impl HostBackend {
    /// Create a new host backend.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl NetworkBackend for HostBackend {
    async fn setup(
        &self,
        _sandbox_id: &SandboxId,
        _network_spec: &NetworkSpec,
    ) -> Result<NetworkConfig, DomainError> {
        // No-op: host networking uses the host's network namespace directly
        Ok(NetworkConfig {
            interface_name: "host".to_string(),
            guest_ip: String::new(),
            gateway_ip: String::new(),
            subnet_mask: 0,
        })
    }

    async fn teardown(&self, _sandbox_id: &SandboxId) -> Result<(), DomainError> {
        // No-op: nothing to clean up
        Ok(())
    }

    fn kind(&self) -> NetworkKind {
        NetworkKind::Host
    }
}
