//! TOML-deserializable provider configuration structs.
//!
//! These types allow loading provider definitions from `.bastion/providers/*.toml` files.

use serde::Deserialize;
use bastion_domain::provider::capabilities::ProviderCapabilities;

/// TOML-deserializable provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    /// Provider name (used for registration key).
    pub name: String,
    /// Provider kind (e.g., "podman", "firecracker", "gvisor").
    pub kind: String,
    /// Plugin source: "builtin" or "wasm:path". Defaults to "builtin" if omitted.
    #[serde(default = "default_plugin")]
    pub plugin: String,
    /// Optional default flag. If true, this becomes the default provider.
    #[serde(default)]
    pub default: Option<bool>,
    /// Path to the provider socket (provider-specific).
    #[serde(default)]
    pub socket: Option<String>,
    /// Default image for this provider.
    #[serde(default)]
    pub image: Option<String>,
    /// Path to the worker binary for container injection.
    #[serde(default)]
    pub worker_binary: Option<String>,
    /// Provider capabilities.
    #[serde(default)]
    pub capabilities: ProviderCapabilitiesConfig,
}

fn default_plugin() -> String {
    "builtin".to_string()
}

/// Provider capabilities deserialized from TOML.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderCapabilitiesConfig {
    #[serde(default)]
    pub supports_snapshots: Option<bool>,
    #[serde(default)]
    pub supports_streaming: Option<bool>,
    #[serde(default)]
    pub avg_startup_ms: Option<u32>,
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    #[serde(default)]
    pub max_cpu_count: Option<u32>,
    #[serde(default)]
    pub requires_kvm: Option<bool>,
}

impl ProviderCapabilitiesConfig {
    /// Convert to domain ProviderCapabilities, applying defaults for missing fields.
    pub fn into_domain(self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: self.supports_snapshots.unwrap_or(false),
            supports_streaming: self.supports_streaming.unwrap_or(true),
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: self.max_memory_mb.unwrap_or(16384),
            max_cpu_count: self.max_cpu_count.unwrap_or(16),
            supports_networking: true,
            requires_kvm: self.requires_kvm.unwrap_or(false),
            avg_startup_ms: self.avg_startup_ms.unwrap_or(1500),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_parsing() {
        let toml_str = r#"
name = "podman"
kind = "podman"
plugin = "builtin"
default = true

[capabilities]
supports_snapshots = false
supports_streaming = true
avg_startup_ms = 1500
max_memory_mb = 16384
"#;
        let config: ProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "podman");
        assert_eq!(config.kind, "podman");
        assert!(config.default.is_some_and(|d| d));
        assert_eq!(config.capabilities.avg_startup_ms, Some(1500));
    }

    #[test]
    fn test_provider_capabilities_defaults() {
        let empty: ProviderCapabilitiesConfig = toml::from_str("").unwrap_or_default();
        let caps = empty.into_domain();
        // Check defaults are applied
        assert_eq!(caps.avg_startup_ms, 1500);
        assert_eq!(caps.max_memory_mb, 16384);
    }
}
