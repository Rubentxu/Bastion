//! Value objects for the Sandbox bounded context.
//!
//! Value objects are immutable and compared by value, not identity.

use serde::{Deserialize, Serialize};

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

/// Resource specification for a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesSpec {
    pub cpu_count: u32,
    pub memory_mb: u64,
    pub disk_mb: u64,
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

/// Filter for listing sandboxes managed by a provider.
#[derive(Debug, Clone, Default)]
pub struct SandboxFilter {
    /// Filter by provider name (exact match).
    pub provider_name: Option<String>,
    /// Filter by sandbox status.
    pub status: Option<SandboxStatus>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
    /// Cursor for pagination (opaque string from previous response).
    pub cursor: Option<String>,
}
