//! SdkmanAdapter — installs JVM tools via SDKMAN!.

use std::collections::HashMap;

use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{SupportLevel, ToolManagerAdapter, ToolchainPlan, ToolchainRequest, ToolchainStep, ToolVerifyStep};

/// Adapter for SDKMAN! based JVM tool installation.
pub struct SdkmanAdapter;

const SDKMAN_INSTALL: &str = "curl -s \"https://get.sdkman.io\" | bash";
const SDKMAN_SOURCE: &str = "source ~/.sdkman/bin/sdkman-init.sh";

#[async_trait]
impl ToolManagerAdapter for SdkmanAdapter {
    fn id(&self) -> &'static str { "sdkman" }
    fn name(&self) -> &'static str { "SDKMAN! Java Version Manager" }

    fn supports(&self, req: &ToolchainRequest) -> SupportLevel {
        match req.capability.as_str() {
            "jvm-build" => SupportLevel::Full,
            "java-build" => SupportLevel::Full,
            _ => SupportLevel::None, // SDKMAN is JVM-only
        }
    }

    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError> {
        let mut steps = Vec::new();
        let mut env = HashMap::new();

        // Step 1: Install SDKMAN
        steps.push(ToolchainStep {
            description: "Install SDKMAN!".into(),
            command: SDKMAN_INSTALL.into(),
            env: HashMap::new(),
            timeout_ms: 120_000,
            expected_exit_code: 0,
        });

        // Get version constraints (default if not specified)
        let java_version = req.constraints.get("java").map(|s| s.as_str()).unwrap_or("17.0.8-tem");
        let maven_version = req.constraints.get("maven").map(|s| s.as_str()).unwrap_or("3.9.5");
        let gradle_version = req.constraints.get("gradle").map(|s| s.as_str());

        // Step 2: Install Java
        steps.push(ToolchainStep {
            description: format!("Install Java {}", java_version),
            command: format!("{SDKMAN_SOURCE} && sdk install java {java_version}"),
            env: HashMap::new(),
            timeout_ms: 600_000,
            expected_exit_code: 0,
        });

        // Step 3: Install Maven
        steps.push(ToolchainStep {
            description: format!("Install Maven {}", maven_version),
            command: format!("{SDKMAN_SOURCE} && sdk install maven {maven_version}"),
            env: HashMap::new(),
            timeout_ms: 300_000,
            expected_exit_code: 0,
        });

        // Step 4: Optional Gradle
        if let Some(gv) = gradle_version {
            steps.push(ToolchainStep {
                description: format!("Install Gradle {}", gv),
                command: format!("{SDKMAN_SOURCE} && sdk install gradle {gv}"),
                env: HashMap::new(),
                timeout_ms: 300_000,
                expected_exit_code: 0,
            });
        }

        // Environment
        let java_home = format!("/root/.sdkman/candidates/java/{java_version}");
        env.insert("JAVA_HOME".into(), java_home.clone());
        env.insert("SDKMAN_DIR".into(), "/root/.sdkman".into());

        let verification = vec![
            ToolVerifyStep {
                label: "Java version".into(),
                command: format!("{SDKMAN_SOURCE} && java -version"),
                expected_output_contains: Some("openjdk".into()),
                expected_exit_code: 0,
            },
            ToolVerifyStep {
                label: "Maven version".into(),
                command: format!("{SDKMAN_SOURCE} && mvn -version"),
                expected_output_contains: Some("Apache Maven".into()),
                expected_exit_code: 0,
            },
        ];

        Ok(ToolchainPlan {
            capability: req.capability.clone(),
            adapter_used: "sdkman".into(),
            steps,
            verification,
            env,
            path_prefix: vec![format!("{}/bin", java_home)],
        })
    }
}
