//! Embedded default provider and capability configurations.
//!
//! These defaults are compiled into the binary via `include_str!` and used
//! when the `.bastion/` config directory is empty or missing.

/// Embedded default podman provider config.
pub const DEFAULT_PODMAN_PROVIDER: &str = include_str!("../../../../.bastion/providers/podman.toml");

/// Embedded default firecracker provider config.
#[allow(non_upper_case_globals)]
pub const DEFAULT_FIREcracker_PROVIDER: &str = include_str!("../../../../.bastion/providers/firecracker.toml");

/// Embedded default gvisor provider config.
pub const DEFAULT_GVISOR_PROVIDER: &str = include_str!("../../../../.bastion/providers/gvisor.toml");

/// Embedded default jvm-build capability config.
pub const DEFAULT_JVM_BUILD: &str = include_str!("../../../../.bastion/capabilities/jvm-build.toml");

/// Embedded default node-build capability config.
pub const DEFAULT_NODE_BUILD: &str = include_str!("../../../../.bastion/capabilities/node-build.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_podman_toml_valid() {
        let parsed: crate::provider::config::ProviderConfig = toml::from_str(DEFAULT_PODMAN_PROVIDER).unwrap();
        assert_eq!(parsed.name, "podman");
        assert_eq!(parsed.kind, "podman");
    }

    #[test]
    fn test_embedded_jvm_build_toml_valid() {
        let parsed: crate::template::capability_config::CapabilityConfig = toml::from_str(DEFAULT_JVM_BUILD).unwrap();
        assert_eq!(parsed.name, "jvm-build");
        assert!(!parsed.toolchains.is_empty());
    }

    #[test]
    fn test_embedded_node_build_toml_valid() {
        let parsed: crate::template::capability_config::CapabilityConfig = toml::from_str(DEFAULT_NODE_BUILD).unwrap();
        assert_eq!(parsed.name, "node-build");
    }
}
