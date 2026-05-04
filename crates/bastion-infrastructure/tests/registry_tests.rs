//! Integration tests for ProviderRegistry and CapabilityRegistry.
//!
//! These tests verify TOML loading, resolution, and fallback behavior.

use std::path::Path;
use tempfile::TempDir;
use bastion_infrastructure::provider::config::{ProviderConfig, ProviderCapabilitiesConfig};
use bastion_infrastructure::provider::registry::ProviderRegistry;
use bastion_infrastructure::provider::factory::ProviderFactory;
use bastion_infrastructure::template::capability_config::CapabilityConfig;
use bastion_infrastructure::template::capability_registry::CapabilityRegistry;
use bastion_domain::template::ToolchainStrategy;

/// Helper: write a file to a temp dir and return its path.
fn write_temp_file(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Helper: create a temp dir with capability TOMLs.
fn create_capability_toml_dir() -> TempDir {
    let dir = TempDir::new().unwrap();

    let jvm_build_toml = r#"
name = "jvm-build"
description = "Java/JVM build environment"

[[toolchains]]
manager = "sdkman"
priority = 10

[[toolchains.steps]]
description = "Install SDKMAN"
command = "curl -s 'https://get.sdkman.io' | bash"
timeout_ms = 60000
expected_exit_code = 0

[[toolchains.steps]]
description = "Install Java"
command = "source $HOME/.sdkman/bin/sdkman-init.sh && sdk install java 17.0.7-tem"
timeout_ms = 300000
expected_exit_code = 0

[[toolchains.verification]]
label = "java-version"
command = "java -version"
expected_output_contains = "17"
expected_exit_code = 0

[[toolchains]]
manager = "apt"
priority = 20
packages = ["openjdk-17-jdk", "maven"]

[[toolchains]]
manager = "asdf"
priority = 30

[[toolchains.steps]]
description = "Install Java via asdf"
command = '. "$HOME/.asdf/asdf.sh" && asdf plugin add java && asdf install java 17.0.7'
timeout_ms = 600000
expected_exit_code = 0
"#;
    write_temp_file(&dir, "jvm-build.toml", jvm_build_toml);

    let node_build_toml = r#"
name = "node-build"
description = "Node.js build environment"

[[toolchains]]
manager = "asdf"
priority = 10

[[toolchains.steps]]
description = "Install Node.js via asdf"
command = '. "$HOME/.asdf/asdf.sh" && asdf plugin add nodejs && asdf install nodejs 20.0.0'
timeout_ms = 600000
expected_exit_code = 0
"#;
    write_temp_file(&dir, "node-build.toml", node_build_toml);

    dir
}

// =============================================================================
// ProviderConfig TOML Deserialization Tests
// =============================================================================

mod provider_config_tests {
    use super::*;

    #[test]
    fn test_provider_config_valid_toml() {
        let toml_str = r#"
name = "test-podman"
kind = "podman"
plugin = "builtin"
default = true

[capabilities]
supports_snapshots = true
supports_streaming = false
avg_startup_ms = 2000
max_memory_mb = 8192
max_cpu_count = 8
requires_kvm = true
"#;
        let config: ProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "test-podman");
        assert_eq!(config.kind, "podman");
        assert_eq!(config.plugin, "builtin");
        assert!(config.default.is_some_and(|d| d));
        assert!(config.capabilities.supports_snapshots.is_some_and(|v| v));
        assert!(!config.capabilities.supports_streaming.unwrap_or(true));
        assert_eq!(config.capabilities.avg_startup_ms, Some(2000));
        assert_eq!(config.capabilities.max_memory_mb, Some(8192));
        assert_eq!(config.capabilities.max_cpu_count, Some(8));
        assert!(config.capabilities.requires_kvm.is_some_and(|v| v));
    }

    #[test]
    fn test_provider_config_minimal_toml() {
        let toml_str = r#"
name = "minimal"
kind = "podman"
"#;
        let config: ProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "minimal");
        assert_eq!(config.kind, "podman");
        assert_eq!(config.plugin, "builtin"); // default
        assert!(config.default.is_none());
        // All capability fields should be None (use defaults)
        assert!(config.capabilities.supports_snapshots.is_none());
        assert!(config.capabilities.supports_streaming.is_none());
    }

    #[test]
    fn test_provider_config_invalid_toml() {
        let toml_str = r#"
name = "bad"
kind = "podman"
invalid field that will cause parse error
"#;
        let result: Result<ProviderConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_provider_capabilities_into_domain() {
        let caps_config = ProviderCapabilitiesConfig {
            supports_snapshots: Some(true),
            supports_streaming: Some(false),
            avg_startup_ms: Some(3000),
            max_memory_mb: Some(32768),
            max_cpu_count: Some(32),
            requires_kvm: Some(true),
        };

        let domain = caps_config.into_domain();
        assert!(domain.supports_snapshots);
        assert!(!domain.supports_streaming);
        assert_eq!(domain.avg_startup_ms, 3000);
        assert_eq!(domain.max_memory_mb, 32768);
        assert_eq!(domain.max_cpu_count, 32);
        assert!(domain.requires_kvm);
    }
}

// =============================================================================
// CapabilityConfig TOML Deserialization Tests
// =============================================================================

mod capability_config_tests {
    use super::*;

    #[test]
    fn test_capability_config_with_explicit_steps() {
        let toml_str = r#"
name = "python-build"
description = "Python build environment"

[[toolchains]]
manager = "asdf"
priority = 10

[[toolchains.steps]]
description = "Install Python via asdf"
command = '. "$HOME/.asdf/asdf.sh" && asdf plugin add python && asdf install python 3.11.0'
timeout_ms = 600000
expected_exit_code = 0

[[toolchains.verification]]
label = "python-version"
command = "python --version"
expected_output_contains = "3.11"
expected_exit_code = 0
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "python-build");
        assert_eq!(config.description.as_deref(), Some("Python build environment"));
        assert_eq!(config.toolchains.len(), 1);

        let toolchain = &config.toolchains[0];
        assert_eq!(toolchain.manager, "asdf");
        assert_eq!(toolchain.priority, 10);
        assert!(toolchain.steps.is_some());
        assert!(toolchain.verification.is_some());

        let steps = toolchain.steps.as_ref().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].description, "Install Python via asdf");

        let verification = toolchain.verification.as_ref().unwrap();
        assert_eq!(verification.len(), 1);
        assert_eq!(verification[0].label, "python-version");
    }

    #[test]
    fn test_capability_config_apt_packages() {
        let toml_str = r#"
name = "go-build"

[[toolchains]]
manager = "apt"
priority = 5
packages = ["golang-go", "git"]
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "go-build");
        assert_eq!(config.toolchains.len(), 1);

        let toolchain = &config.toolchains[0];
        assert_eq!(toolchain.manager, "apt");
        assert!(toolchain.packages.is_some());
        let packages = toolchain.packages.as_ref().unwrap();
        assert_eq!(packages.len(), 2);
        assert!(packages.contains(&"golang-go".to_string()));
    }

    #[test]
    fn test_capability_config_multiple_toolchains() {
        let toml_str = r#"
name = "multi-toolchain"

[[toolchains]]
manager = "sdkman"
priority = 10

[[toolchains.steps]]
description = "Install via sdkman"
command = "sdk install java"

[[toolchains]]
manager = "apt"
priority = 20
packages = ["openjdk-17-jdk"]

[[toolchains]]
manager = "brew"
priority = 30
packages = ["openjdk@17"]
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.toolchains.len(), 3);

        // Sorted by priority: sdkman (10), apt (20), brew (30)
        assert_eq!(config.toolchains[0].manager, "sdkman");
        assert_eq!(config.toolchains[1].manager, "apt");
        assert_eq!(config.toolchains[2].manager, "brew");
    }

    #[test]
    fn test_capability_config_invalid_toml() {
        let toml_str = r#"
name = "bad"
invalid syntax here
"#;
        let result: Result<CapabilityConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }
}

// =============================================================================
// ProviderRegistry Tests
// =============================================================================

mod provider_registry_tests {
    use super::*;

    #[test]
    fn test_provider_registry_load_from_empty_dir() {
        let dir = TempDir::new().unwrap();
        // Create factory with a dummy provider that won't be used
        let factory = ProviderFactory::new("dummy");
        // We can't easily add a provider without a real SandboxProvider
        // So we just test that load_from_dir with empty dir returns 0
        let registry = ProviderRegistry::new(factory);
        let result = registry.load_from_dir(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_provider_registry_load_from_nonexistent_dir() {
        let factory = ProviderFactory::new("dummy");
        let registry = ProviderRegistry::new(factory);
        let result = registry.load_from_dir(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_ok()); // Should not panic, just return 0
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_provider_registry_default_name() {
        let factory = ProviderFactory::new("test-provider");
        let registry = ProviderRegistry::new(factory);
        assert_eq!(registry.default_name(), "test-provider");
    }
}

// =============================================================================
// CapabilityRegistry Tests
// =============================================================================

mod capability_registry_tests {
    use super::*;

    #[test]
    fn test_capability_registry_resolve_jvm_build_auto() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        // Auto strategy should select highest priority (sdkman, priority 10)
        let plan = registry.resolve("jvm-build", ToolchainStrategy::Auto);
        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(plan.capability, "jvm-build");
        assert_eq!(plan.adapter_used, "sdkman"); // First toolchain by priority
        assert!(!plan.steps.is_empty());
    }

    #[test]
    fn test_capability_registry_resolve_jvm_build_system_package() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        // SystemPackage strategy should select apt (priority 20)
        let plan = registry.resolve("jvm-build", ToolchainStrategy::SystemPackage);
        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(plan.adapter_used, "apt");
    }

    #[test]
    fn test_capability_registry_resolve_jvm_build_version_manager() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        // VersionManager strategy should select sdkman (priority 10) - it's a version manager too
        let plan = registry.resolve("jvm-build", ToolchainStrategy::VersionManager);
        assert!(plan.is_some());
        assert_eq!(plan.unwrap().adapter_used, "sdkman");
    }

    #[test]
    fn test_capability_registry_resolve_unknown_capability() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        let plan = registry.resolve("unknown-capability", ToolchainStrategy::Auto);
        assert!(plan.is_none());
    }

    #[test]
    fn test_capability_registry_empty_registry() {
        let registry = CapabilityRegistry::new();
        assert!(registry.list_capabilities().is_empty());
        assert!(!registry.contains("jvm-build"));

        let plan = registry.resolve("jvm-build", ToolchainStrategy::Auto);
        assert!(plan.is_none());
    }

    #[test]
    fn test_capability_registry_backward_compat_fallback() {
        // When TOML dir is empty/non-existent, registry should return None
        // This allows the fallback to ToolResolver to work
        let registry = CapabilityRegistry::new();

        let plan = registry.resolve("jvm-build", ToolchainStrategy::Auto);
        assert!(plan.is_none()); // Registry doesn't have it, so fallback will try ToolResolver
    }

    #[test]
    fn test_capability_registry_node_build_asdf() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        let plan = registry.resolve("node-build", ToolchainStrategy::Auto);
        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(plan.capability, "node-build");
        assert_eq!(plan.adapter_used, "asdf");
    }

    #[test]
    fn test_capability_registry_into_toolchain_plan() {
        let toml_str = r#"
name = "test-cap"
description = "Test capability"

[[toolchains]]
manager = "brew"
priority = 5
packages = ["node", "npm"]

[[toolchains.verification]]
label = "node-installed"
command = "node --version"
expected_output_contains = "v"
expected_exit_code = 0
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        let plan = config.into_toolchain_plan("test-cap");
        assert!(plan.is_some());
        let plan = plan.unwrap();
        assert_eq!(plan.capability, "test-cap");
        assert_eq!(plan.adapter_used, "brew");
        assert!(!plan.steps.is_empty());
        assert_eq!(plan.verification.len(), 1);
        assert_eq!(plan.verification[0].label, "node-installed");
    }

    #[test]
    fn test_capability_registry_contains_and_list() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        assert!(registry.contains("jvm-build"));
        assert!(registry.contains("node-build"));
        assert!(!registry.contains("python-build"));

        let caps = registry.list_capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&"jvm-build".to_string()));
        assert!(caps.contains(&"node-build".to_string()));
    }

    #[test]
    fn test_capability_registry_get_config() {
        let dir = create_capability_toml_dir();
        let registry = CapabilityRegistry::new();
        registry.load_from_dir(dir.path()).expect("Failed to load capabilities");

        let config = registry.get_config("jvm-build");
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.name, "jvm-build");
        assert!(config.description.is_some());

        let missing = registry.get_config("nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_capability_registry_empty_dir_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let registry = CapabilityRegistry::new();
        // Should not panic on empty directory
        let result = registry.load_from_dir(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
