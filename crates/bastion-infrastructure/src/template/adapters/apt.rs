//! AptAdapter — installs tools via apt-get (Debian/Ubuntu).

use std::collections::HashMap;

use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{ManagerType, SupportLevel, ToolManagerAdapter, ToolchainPlan, ToolchainRequest, ToolchainStep, ToolVerifyStep};

/// Adapter for apt-get based tool installation.
pub struct AptAdapter;

#[async_trait]
impl ToolManagerAdapter for AptAdapter {
    fn id(&self) -> &'static str { "apt" }
    fn name(&self) -> &'static str { "APT Package Manager" }
    fn manager_type(&self) -> ManagerType { ManagerType::Apt }

    fn supports(&self, req: &ToolchainRequest) -> SupportLevel {
        match req.capability.as_str() {
            "jvm-build" | "node-build" | "python-build" => SupportLevel::Full,
            "rust-build" | "go-build" => SupportLevel::Partial,
            _ => SupportLevel::None,
        }
    }

    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError> {
        let mut steps = Vec::new();
        let mut verification = Vec::new();
        let mut env = HashMap::new();

        // Common tools for all builds
        let base_packages = "git curl ca-certificates";
        // Capability-specific packages
        let (packages, java_home, verifications) = match req.capability.as_str() {
            "jvm-build" => {
                env.insert("JAVA_HOME".into(), "/usr/lib/jvm/default-java".into());
                (
                    format!("default-jdk maven {}", base_packages),
                    Some("/usr/lib/jvm/default-java"),
                    vec![
                        ("Java version", "java -version", Some("openjdk")),
                        ("Maven version", "mvn -version", Some("Apache Maven")),
                        ("Git version", "git --version", Some("git version")),
                    ],
                )
            }
            "node-build" => {
                (
                    format!("nodejs npm {}", base_packages),
                    None,
                    vec![
                        ("Node version", "node --version", None),
                        ("NPM version", "npm --version", None),
                    ],
                )
            }
            "python-build" => {
                (
                    format!("python3 python3-pip python3-venv {}", base_packages),
                    None,
                    vec![
                        ("Python version", "python3 --version", None),
                        ("Pip version", "pip3 --version", None),
                    ],
                )
            }
            _ => {
                return Err(DomainError::UnsupportedOperation(format!(
                    "apt doesn't know how to install {}", req.capability
                )));
            }
        };

        // Step 1: apt-get update + install
        steps.push(ToolchainStep {
            description: "Update package lists and install tools".into(),
            command: format!("apt-get update && apt-get install -y --no-install-recommends {}", packages),
            env: HashMap::new(),
            timeout_ms: 300_000,
            expected_exit_code: 0,
        });

        // Verification steps
        for (label, cmd, expected) in verifications {
            verification.push(ToolVerifyStep {
                label: label.into(),
                command: cmd.into(),
                expected_output_contains: expected.map(|s| s.into()),
                expected_exit_code: 0,
            });
        }

        let mut path_prefix = vec![];
        if let Some(jh) = java_home {
            path_prefix.push(format!("{}/bin", jh));
        }

        Ok(ToolchainPlan {
            capability: req.capability.clone(),
            adapter_used: "apt".into(),
            steps,
            verification,
            env,
            path_prefix,
        })
    }
}
