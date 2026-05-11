//! TAP device network backend.

use std::process::Command;

use async_trait::async_trait;

use bastion_domain::provider::network::{NetworkBackend, NetworkConfig, NetworkKind};
use bastion_domain::sandbox::value_objects::NetworkSpec;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// TAP device backend for Firecracker VMs.
///
/// Creates and manages TAP devices using the `ip tuntap` command.
/// Each sandbox gets a unique TAP device named `tap-<sandbox_id_prefix>`.
#[derive(Debug)]
pub struct TapBackend {
    /// Gateway IP address to assign to the TAP interface.
    gateway_ip: String,
    /// Subnet mask in CIDR notation.
    subnet_mask: u8,
}

impl TapBackend {
    /// Create a new TAP backend.
    ///
    /// * `gateway_ip` — IP address for the host-side TAP interface (e.g., "10.0.2.1")
    /// * `subnet_mask` — Subnet mask in CIDR notation (e.g., 24 for /24)
    pub fn new(gateway_ip: String, subnet_mask: u8) -> Self {
        Self {
            gateway_ip,
            subnet_mask,
        }
    }

    /// Generate TAP device name for a sandbox.
    ///
    /// Format: `tap-<sandbox_id>` with underscores converted to dashes,
    /// truncated to max 15 characters.
    fn tap_name_for_sandbox(&self, sandbox_id: &SandboxId) -> String {
        format!(
            "tap-{}",
            sandbox_id
                .to_string()
                .replace('_', "-")
                .chars()
                .take(12)
                .collect::<String>()
        )
    }

    /// Validate a TAP device name.
    ///
    /// TAP names must be ≤15 characters and contain only alphanumeric
    /// characters and dashes.
    fn tap_name_valid(name: &str) -> bool {
        if name.len() > 15 {
            return false;
        }
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }
}

#[async_trait]
impl NetworkBackend for TapBackend {
    async fn setup(
        &self,
        sandbox_id: &SandboxId,
        _network_spec: &NetworkSpec,
    ) -> Result<NetworkConfig, DomainError> {
        let tap_name = self.tap_name_for_sandbox(sandbox_id);

        // Validate TAP name
        if !Self::tap_name_valid(&tap_name) {
            return Err(DomainError::Validation(format!(
                "Invalid TAP device name: {} (must be ≤15 chars, alphanumeric/dash/underscore only)",
                tap_name
            )));
        }

        // Create TAP device: ip tuntap add dev <tap> mode tap
        let output = Command::new("ip")
            .args(["tuntap", "add", "dev", &tap_name, "mode", "tap"])
            .output()
            .map_err(|e| DomainError::Internal(format!("Failed to create TAP device: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // If "File exists", the device already exists — that's OK, continue
            if !stderr.contains("File exists") {
                return Err(DomainError::Internal(format!(
                    "Failed to create TAP device '{}': {}",
                    tap_name, stderr
                )));
            }
            tracing::debug!(tap_name, "TAP device already exists, continuing");
        }

        // Bring up the TAP device: ip link set <tap> up
        let _ = Command::new("ip")
            .args(["link", "set", &tap_name, "up"])
            .output();

        // Assign IP address: ip addr add <gateway_ip>/<mask> dev <tap>
        let _ = Command::new("ip")
            .args([
                "addr",
                "add",
                &format!("{}/{}", self.gateway_ip, self.subnet_mask),
                "dev",
                &tap_name,
            ])
            .output();

        tracing::info!(
            tap_name,
            gateway_ip = %self.gateway_ip,
            "TAP device created for sandbox"
        );

        Ok(NetworkConfig {
            interface_name: tap_name,
            guest_ip: format!(
                "{}.2",
                &self.gateway_ip[..self.gateway_ip.rfind('.').map(|i| i + 1).unwrap_or(0)]
            ),
            gateway_ip: self.gateway_ip.clone(),
            subnet_mask: self.subnet_mask,
        })
    }

    async fn teardown(&self, sandbox_id: &SandboxId) -> Result<(), DomainError> {
        let tap_name = self.tap_name_for_sandbox(sandbox_id);

        // Bring down the TAP device: ip link set <tap> down
        let _ = Command::new("ip")
            .args(["link", "set", &tap_name, "down"])
            .output();

        // Delete TAP device: ip tuntap del dev <tap> mode tap
        let _ = Command::new("ip")
            .args(["tuntap", "del", "dev", &tap_name, "mode", "tap"])
            .output();

        tracing::info!(tap_name, "TAP device destroyed for sandbox");

        Ok(())
    }

    fn kind(&self) -> NetworkKind {
        NetworkKind::Tap
    }
}
