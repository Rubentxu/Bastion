//! AsdfAdapter — installs tools via asdf version manager.

use std::collections::HashMap;

use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{
    ManagerType, SupportLevel, ToolManagerAdapter, ToolVerifyStep, ToolchainPlan, ToolchainRequest,
    ToolchainStep,
};

/// Adapter for asdf-vm based tool installation.
pub struct AsdfAdapter;

const ASDF_SETUP: &str = "git clone https://github.com/asdf-vm/asdf.git ~/.asdf --branch v0.14.0";
const ASDF_SOURCE: &str = "export ASDF_DIR=\"$HOME/.asdf\" && . \"$ASDF_DIR/asdf.sh\"";

/// Install prerequisites needed for asdf (git, curl, etc.)
const PREREQ_INSTALL: &str = "apt-get update && apt-get install -y git curl unzip";

#[async_trait]
impl ToolManagerAdapter for AsdfAdapter {
    fn id(&self) -> &'static str {
        "asdf"
    }
    fn name(&self) -> &'static str {
        "asdf Version Manager"
    }
    fn manager_type(&self) -> ManagerType {
        ManagerType::Asdf
    }

    fn supports(&self, req: &ToolchainRequest) -> SupportLevel {
        match req.capability.as_str() {
            "jvm-build" | "node-build" | "python-build" | "ruby-build" | "go-build"
            | "rust-build" => SupportLevel::Full,
            _ => SupportLevel::None,
        }
    }

    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError> {
        let mut steps = Vec::new();
        let mut verification = Vec::new();
        let env = HashMap::new();

        // Step 0: Install prerequisites (git, curl needed by asdf)
        steps.push(ToolchainStep {
            description: "Install prerequisites for asdf (git, curl)".into(),
            command: PREREQ_INSTALL.into(),
            env: HashMap::new(),
            timeout_ms: 120_000,
            expected_exit_code: 0,
        });

        // Step 1: Install asdf itself
        steps.push(ToolchainStep {
            description: "Clone asdf-vm".into(),
            command: ASDF_SETUP.into(),
            env: HashMap::new(),
            timeout_ms: 120_000,
            expected_exit_code: 0,
        });

        steps.push(ToolchainStep {
            description: "Setup asdf in shell".into(),
            command: "echo '. ~/.asdf/asdf.sh' >> ~/.bashrc".into(),
            env: HashMap::new(),
            timeout_ms: 5_000,
            expected_exit_code: 0,
        });

        // Tool-specific steps
        match req.capability.as_str() {
            "jvm-build" => {
                // Java via asdf-java
                steps.push(ToolchainStep {
                    description: "Add asdf-java plugin".into(),
                    command: format!("{ASDF_SOURCE} && asdf plugin add java https://github.com/halcyon/asdf-java.git"),
                    env: HashMap::new(),
                    timeout_ms: 60_000,
                    expected_exit_code: 0,
                });
                steps.push(ToolchainStep {
                    description: "Install Java 17 via asdf".into(),
                    command: format!("{ASDF_SOURCE} && asdf install java adoptopenjdk-17.0.8+7 && asdf global java adoptopenjdk-17.0.8+7"),
                    env: HashMap::new(),
                    timeout_ms: 600_000,
                    expected_exit_code: 0,
                });
                steps.push(ToolchainStep {
                    description: "Install Maven via asdf".into(),
                    command: format!("{ASDF_SOURCE} && asdf plugin add maven && asdf install maven 3.9.5 && asdf global maven 3.9.5"),
                    env: HashMap::new(),
                    timeout_ms: 300_000,
                    expected_exit_code: 0,
                });

                verification.push(ToolVerifyStep {
                    label: "Java version".into(),
                    command: format!("{ASDF_SOURCE} && java -version"),
                    expected_output_contains: Some("openjdk".into()),
                    expected_exit_code: 0,
                });
                verification.push(ToolVerifyStep {
                    label: "Maven version".into(),
                    command: format!("{ASDF_SOURCE} && mvn -version"),
                    expected_output_contains: Some("Apache Maven".into()),
                    expected_exit_code: 0,
                });
            }
            "node-build" => {
                steps.push(ToolchainStep {
                    description: "Install Node.js via asdf".into(),
                    command: format!("{ASDF_SOURCE} && asdf plugin add nodejs && asdf install nodejs 20.0.0 && asdf global nodejs 20.0.0"),
                    env: HashMap::new(),
                    timeout_ms: 300_000,
                    expected_exit_code: 0,
                });
                verification.push(ToolVerifyStep {
                    label: "Node version".into(),
                    command: format!("{ASDF_SOURCE} && node --version"),
                    expected_output_contains: None,
                    expected_exit_code: 0,
                });
            }
            _ => {
                return Err(DomainError::UnsupportedOperation(format!(
                    "asdf doesn't know how to install {}",
                    req.capability
                )));
            }
        }

        Ok(ToolchainPlan {
            capability: req.capability.clone(),
            adapter_used: "asdf".into(),
            steps,
            verification,
            env,
            path_prefix: vec!["~/.asdf/shims".into()],
        })
    }
}
