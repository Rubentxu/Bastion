//! Network backend trait — manages network configuration.

use async_trait::async_trait;

use crate::sandbox::value_objects::NetworkSpec;
use crate::shared::id::SandboxId;
use crate::shared::DomainError;

/// Network configuration returned by a network backend after setup.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Name of the host-side network interface (e.g., "tap-abc123").
    pub interface_name: String,
    /// IP address assigned to the guest/VM side.
    pub guest_ip: String,
    /// Gateway IP address.
    pub gateway_ip: String,
    /// Subnet mask in CIDR notation (e.g., 24 for /24).
    pub subnet_mask: u8,
}

/// The kind of network backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkKind {
    /// TAP device backend (used by Firecracker).
    Tap,
    /// Bridge backend (not yet implemented).
    Bridge,
    /// Host network backend (container uses host networking directly).
    Host,
}

/// Network backend trait — abstracts network setup/teardown for sandbox providers.
///
/// Implementations handle TAP device creation, bridge setup, or host networking
/// depending on the backend type.
#[cfg(feature = "use-segregated-traits")]
#[async_trait]
pub trait NetworkBackend: Send + Sync + std::fmt::Debug {
    /// Set up networking for a sandbox.
    ///
    /// Creates the necessary network infrastructure (TAP device, bridge, etc.)
    /// and returns the configuration needed to connect the sandbox.
    async fn setup(
        &self,
        sandbox_id: &SandboxId,
        network_spec: &NetworkSpec,
    ) -> Result<NetworkConfig, DomainError>;

    /// Tear down networking for a sandbox.
    ///
    /// Cleans up any network resources created during `setup`.
    /// This operation should be idempotent — succeeding even if the resources
    /// have already been cleaned up.
    async fn teardown(&self, sandbox_id: &SandboxId) -> Result<(), DomainError>;

    /// Return the kind of this backend.
    fn kind(&self) -> NetworkKind;
}
