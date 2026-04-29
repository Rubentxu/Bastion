//! Provider capabilities value object.

use serde::{Deserialize, Serialize};

/// Capabilities reported by a provider backend.
///
/// Used for provider selection and feature gating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_snapshots: bool,
    pub supports_streaming: bool,
    pub supports_pause_resume: bool,
    pub max_timeout_ms: u64,
    pub max_memory_mb: u64,
    pub max_cpu_count: u32,
    pub supports_networking: bool,
    pub requires_kvm: bool,
    pub avg_startup_ms: u32,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 16_384,
            max_cpu_count: 16,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 1500,
        }
    }
}
