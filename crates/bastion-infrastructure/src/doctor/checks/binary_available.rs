//! Binary availability check.

use bastion_domain::catalog::doctor::{
    BinaryInfo, CheckStatus, DeltaItem, InstallSource, Remediation, RichCheckResult, Severity,
};
use crate::doctor::checks::{generate_trace_id, DoctorContext};

pub async fn evaluate(
    ctx: &DoctorContext<'_>,
    binary_name: &str,
    expected_path: Option<&str>,
) -> RichCheckResult {
    let check_id = format!("binary_available.{}", binary_name);

    // Get binary info from system context
    let binary_info: BinaryInfo = ctx
        .system_context
        .relevant_binaries
        .get(binary_name)
        .cloned()
        .unwrap_or_else(|| BinaryInfo {
            name: binary_name.to_string(),
            path: None,
            version: None,
        });

    // Check if binary is available
    let is_available = binary_info.path.is_some();
    let path_matches = expected_path
        .map(|expected| binary_info.path.as_ref().map(|p| p.contains(expected)).unwrap_or(false))
        .unwrap_or(true);

    let current_state = serde_json::json!({
        "binary_name": binary_name,
        "path": binary_info.path,
        "version": binary_info.version,
        "is_available": is_available,
    });

    let expected_state = serde_json::json!({
        "binary_name": binary_name,
        "expected_path": expected_path.unwrap_or("any path"),
        "required": true,
    });

    let mut delta: Vec<DeltaItem> = Vec::new();

    if !is_available {
        delta.push(DeltaItem {
            item: format!("binary '{}'", binary_name),
            expected: "binary found in PATH".to_string(),
            actual: None,
            severity: Severity::Critical,
        });
    } else if !path_matches {
        delta.push(DeltaItem {
            item: format!("binary path for '{}'", binary_name),
            expected: expected_path.unwrap_or("any path").to_string(),
            actual: binary_info.path.clone(),
            severity: Severity::Warning,
        });
    }

    let remediation = if delta.is_empty() {
        None
    } else if !is_available {
        Some(Remediation {
            confidence: "high".to_string(),
            auto_fixable: false,
            commands: vec![],
            manual_steps: vec![
                format!("# Install {} using your package manager:", binary_name),
                format!("# Ubuntu/Debian: sudo apt-get install {}", binary_name),
                format!("# Fedora/RHEL: sudo dnf install {}", binary_name),
                format!("# Arch: sudo pacman -S {}", binary_name),
                format!("# macOS: brew install {}", binary_name),
            ],
            verify_after: format!("which {} && {} --version", binary_name, binary_name),
            install_sources: vec![InstallSource {
                name: binary_name.to_string(),
                url: install_url_for(binary_name),
                method: "package_manager".to_string(),
            }],
        })
    } else {
        Some(Remediation {
            confidence: "medium".to_string(),
            auto_fixable: false,
            commands: vec![],
            manual_steps: vec![format!(
                "Binary found at {} but expected at {}",
                binary_info.path.as_ref().unwrap_or(&"unknown".to_string()),
                expected_path.unwrap_or("default PATH")
            )],
            verify_after: format!("{} --version", binary_name),
            install_sources: vec![],
        })
    };

    RichCheckResult {
        check_id,
        check_type: "binary_available".to_string(),
        status: if delta.is_empty() { CheckStatus::Pass } else { CheckStatus::Fail },
        current_state,
        expected_state,
        delta,
        remediation,
        system_context: ctx.system_context.clone(),
        trace_id: generate_trace_id(),
        executed_at: chrono::Utc::now(),
    }
}

fn install_url_for(binary: &str) -> String {
    match binary {
        "podman" => "https://podman.io/docs/installation".to_string(),
        "docker" => "https://docs.docker.com/engine/install/".to_string(),
        "firecracker" => "https://github.com/firecracker-microvm/firecracker".to_string(),
        "runsc" => "https://gvisor.dev/docs/install/".to_string(),
        "kubectl" => "https://kubernetes.io/docs/tasks/tools/install-kubectl/".to_string(),
        "helm" => "https://helm.sh/docs/intro/install/".to_string(),
        _ => format!("https://{}.io", binary),
    }
}