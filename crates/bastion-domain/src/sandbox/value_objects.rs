//! Value objects for the Sandbox bounded context.
//!
//! Value objects are immutable and compared by value, not identity.

use serde::{Deserialize, Serialize};

use crate::shared::DomainError;

/// Status of a sandbox instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStatus {
    Pending,
    Running,
    Paused,
    Stopped,
    Failed,
}

impl std::fmt::Display for SandboxStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl SandboxStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Pending)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }
}

/// Resource specification for a sandbox.
///
/// cpu_count must be >= 1. Use `ResourcesSpec::new()` for validated construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesSpec {
    pub cpu_count: u32,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

impl ResourcesSpec {
    pub fn new(cpu_count: u32, memory_mb: u64, disk_mb: u64) -> Result<Self, DomainError> {
        if cpu_count == 0 {
            return Err(DomainError::Validation(
                "cpu_count must be at least 1".into(),
            ));
        }
        Ok(Self {
            cpu_count,
            memory_mb,
            disk_mb,
        })
    }

    pub fn cpu_count(&self) -> u32 {
        self.cpu_count
    }

    pub fn memory_mb(&self) -> u64 {
        self.memory_mb
    }

    pub fn disk_mb(&self) -> u64 {
        self.disk_mb
    }
}

impl Default for ResourcesSpec {
    fn default() -> Self {
        Self {
            cpu_count: 1,
            memory_mb: 512,
            disk_mb: 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resources_spec_default() {
        let spec = ResourcesSpec::default();
        assert_eq!(spec.cpu_count, 1);
    }

    #[test]
    fn test_resources_spec_new_rejects_zero_cpu() {
        let err = ResourcesSpec::new(0, 512, 1024).expect_err("zero cpu should be rejected");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_resources_spec_new_accepts_valid() {
        let spec = ResourcesSpec::new(4, 8192, 20480).expect("valid spec");
        assert_eq!(spec.cpu_count, 4);
        assert_eq!(spec.memory_mb, 8192);
    }
}

/// Network specification for a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSpec {
    pub allow_internet: bool,
    pub allowed_hosts: Vec<String>,
    pub denied_hosts: Vec<String>,
    pub expose_ports: bool,
    pub exposed_ports: Vec<u16>,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            allow_internet: true,
            allowed_hosts: vec![],
            denied_hosts: vec![],
            expose_ports: false,
            exposed_ports: vec![],
        }
    }
}

impl NetworkSpec {
    pub fn allow_internet(&self) -> bool {
        self.allow_internet
    }

    pub fn allowed_hosts(&self) -> &[String] {
        &self.allowed_hosts
    }
}

/// Filter for listing sandboxes managed by a provider.
#[derive(Debug, Clone, Default)]
pub struct SandboxFilter {
    pub provider_name: Option<String>,
    pub status: Option<SandboxStatus>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}
