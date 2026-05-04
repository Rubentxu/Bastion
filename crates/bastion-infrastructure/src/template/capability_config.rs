//! TOML-deserializable capability configuration structs.
//!
//! These types allow loading capability definitions from `.bastion/capabilities/*.toml` files
//! and mapping them to ToolchainPlan instances.

use std::collections::HashMap;
use serde::Deserialize;
use bastion_domain::template::{ToolchainPlan, ToolchainStep, ToolVerifyStep, ManagerType};

/// TOML-deserializable capability configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityConfig {
    /// Capability name (e.g., "jvm-build", "node-build").
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Ordered list of toolchains to try (first with matching manager wins).
    #[serde(default)]
    pub toolchains: Vec<ToolchainDef>,
}

/// A toolchain definition from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolchainDef {
    /// Package manager: "apt", "asdf", "sdkman", "brew", "nix", "ca_store".
    pub manager: String,
    /// Priority (lower = higher priority). Default 100.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Static list of packages (used by apt, brew, etc.).
    #[serde(default)]
    pub packages: Option<Vec<String>>,
    /// Explicit steps to run.
    #[serde(default)]
    pub steps: Option<Vec<ToolchainStepDef>>,
    /// Verification steps to run after toolchain installation.
    #[serde(default)]
    pub verification: Option<Vec<ToolVerifyStepDef>>,
    /// Environment variables to set.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Path entries to prepend to PATH.
    #[serde(default)]
    pub path_prefix: Option<Vec<String>>,
}

fn default_priority() -> u32 {
    100
}

/// A single step in a toolchain.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolchainStepDef {
    /// Human-readable description of this step.
    pub description: String,
    /// Command to execute.
    pub command: String,
    /// Timeout in milliseconds. Default 300000 (5 minutes).
    #[serde(default = "default_step_timeout")]
    pub timeout_ms: u64,
    /// Expected exit code. Default 0.
    #[serde(default = "default_exit_code")]
    pub expected_exit_code: i32,
    /// Environment variables for this step only.
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

fn default_step_timeout() -> u64 {
    300_000
}

fn default_exit_code() -> i32 {
    0
}

/// A verification step to run after toolchain installation.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolVerifyStepDef {
    /// Label for this verification (e.g., "Check Java version").
    pub label: String,
    /// Command to execute.
    pub command: String,
    /// If set, stdout must contain this string.
    #[serde(default)]
    pub expected_output_contains: Option<String>,
    /// Expected exit code. Default 0.
    #[serde(default = "default_exit_code")]
    pub expected_exit_code: i32,
}

impl CapabilityConfig {
    /// Convert this capability config into a ToolchainPlan.
    ///
    /// Uses the first toolchain in the list (sorted by priority).
    /// Generates commands based on the manager type:
    /// - "apt": generates `apt-get update && apt-get install -y <packages>`
    /// - "asdf": uses explicit steps if provided
    /// - "sdkman": uses explicit steps if provided
    pub fn into_toolchain_plan(&self, capability: &str) -> Option<ToolchainPlan> {
        let toolchain = self.toolchains.first()?;

        let mut steps = Vec::new();
        let mut verification = Vec::new();
        let env = toolchain.env.clone().unwrap_or_default();
        let path_prefix = toolchain.path_prefix.clone().unwrap_or_default();

        // Generate steps based on manager type
        match toolchain.manager.as_str() {
            "apt" => {
                // Generate apt-get steps from packages
                if let Some(packages) = &toolchain.packages
                    && !packages.is_empty()
                {
                    let packages_str = packages.join(" ");
                    steps.push(ToolchainStep {
                        description: "Update apt package index".to_string(),
                        command: "apt-get update".to_string(),
                        env: Default::default(),
                        timeout_ms: 120_000,
                        expected_exit_code: 0,
                    });
                    steps.push(ToolchainStep {
                        description: format!("Install packages: {}", packages_str),
                        command: format!("apt-get install -y {}", packages_str),
                        env: Default::default(),
                        timeout_ms: 300_000,
                        expected_exit_code: 0,
                    });
                }
            }
            "brew" => {
                if let Some(packages) = &toolchain.packages {
                    for pkg in packages {
                        steps.push(ToolchainStep {
                            description: format!("Install {} via brew", pkg),
                            command: format!("brew install {}", pkg),
                            env: Default::default(),
                            timeout_ms: 300_000,
                            expected_exit_code: 0,
                        });
                    }
                }
            }
            _ => {
                // For asdf, sdkman, nix, ca_store: use explicit steps if provided
                if let Some(explicit_steps) = &toolchain.steps {
                    for step in explicit_steps {
                        let mut step_env = step.env.clone().unwrap_or_default();
                        // Merge with toolchain-level env
                        for (k, v) in &env {
                            step_env.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        steps.push(ToolchainStep {
                            description: step.description.clone(),
                            command: step.command.clone(),
                            env: step_env,
                            timeout_ms: step.timeout_ms,
                            expected_exit_code: step.expected_exit_code,
                        });
                    }
                }
            }
        }

        // Convert verification steps
        if let Some(verify_steps) = &toolchain.verification {
            for v in verify_steps {
                verification.push(ToolVerifyStep {
                    label: v.label.clone(),
                    command: v.command.clone(),
                    expected_output_contains: v.expected_output_contains.clone(),
                    expected_exit_code: v.expected_exit_code,
                });
            }
        }

        Some(ToolchainPlan {
            capability: capability.to_string(),
            adapter_used: toolchain.manager.clone(),
            steps,
            verification,
            env,
            path_prefix,
        })
    }
}

/// Maps a manager string to the corresponding ManagerType.
pub fn manager_to_type(manager: &str) -> ManagerType {
    match manager.to_lowercase().as_str() {
        "apt" => ManagerType::Apt,
        "asdf" => ManagerType::Asdf,
        "sdkman" => ManagerType::Sdkman,
        "brew" => ManagerType::Brew,
        "nix" => ManagerType::Nix,
        "ca_store" | "content-addressed" => ManagerType::CaStore,
        _ => ManagerType::CaStore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_config_apt() {
        let toml_str = r#"
name = "jvm-build"
description = "Java build environment"

[[toolchains]]
manager = "apt"
priority = 1
packages = ["openjdk-17-jdk", "maven"]

[[toolchains.verification]]
label = "Check Java version"
command = "java -version 2>&1"
expected_output_contains = "17.0"
expected_exit_code = 0
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "jvm-build");
        assert_eq!(config.toolchains.len(), 1);
        assert_eq!(config.toolchains[0].manager, "apt");

        let plan = config.into_toolchain_plan("jvm-build").unwrap();
        assert_eq!(plan.adapter_used, "apt");
        assert!(plan.steps.len() >= 2); // apt-get update + install
        assert_eq!(plan.verification.len(), 1);
    }

    #[test]
    fn test_capability_config_asdf_explicit_steps() {
        let toml_str = r#"
name = "node-build"

[[toolchains]]
manager = "asdf"
priority = 1

[[toolchains.steps]]
description = "Install Node.js via asdf"
command = '. "$HOME/.asdf/asdf.sh" && asdf plugin add nodejs && asdf install nodejs 20.0.0'
timeout_ms = 600000
expected_exit_code = 0
"#;
        let config: CapabilityConfig = toml::from_str(toml_str).unwrap();
        let plan = config.into_toolchain_plan("node-build").unwrap();
        assert_eq!(plan.adapter_used, "asdf");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "Install Node.js via asdf");
    }

    #[test]
    fn test_empty_toolchains_returns_none() {
        let config = CapabilityConfig {
            name: "empty".to_string(),
            description: None,
            toolchains: vec![],
        };
        assert!(config.into_toolchain_plan("empty").is_none());
    }
}
