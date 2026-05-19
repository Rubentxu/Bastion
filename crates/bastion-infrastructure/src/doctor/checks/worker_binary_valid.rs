//! Worker binary validity check.

use bastion_domain::catalog::doctor::{
    CheckStatus, DeltaItem, InstallSource, Remediation, RichCheckResult, Severity,
};
use crate::doctor::checks::{generate_trace_id, DoctorContext};

pub async fn evaluate(
    ctx: &DoctorContext<'_>,
    provider_name: &str,
) -> RichCheckResult {
    let check_id = format!("worker_binary_valid.{}", provider_name);

    // Get provider config to find worker binary path
    let config = ctx.provider_registry.get_config(provider_name);
    let worker_binary_path = config
        .as_ref()
        .and_then(|c| c.worker_binary.clone())
        .or_else(|| default_worker_binary_for(provider_name));

    // Check if worker binary exists and is valid
    let (is_valid, validation_details) = validate_worker_binary(&worker_binary_path).await;

    let current_state = serde_json::json!({
        "provider": provider_name,
        "worker_binary_path": worker_binary_path,
        "is_valid": is_valid,
        "validation_details": validation_details,
    });

    let expected_state = serde_json::json!({
        "provider": provider_name,
        "worker_binary_required": true,
        "expected_path": config.as_ref().and_then(|c| c.worker_binary.clone()),
    });

    let delta: Vec<DeltaItem> = if !is_valid {
        vec![DeltaItem {
            item: format!("worker binary for '{}'", provider_name),
            expected: "valid worker binary".to_string(),
            actual: validation_details.get("error").cloned(),
            severity: Severity::Critical,
        }]
    } else {
        vec![]
    };

    let remediation = if delta.is_empty() {
        None
    } else {
        Some(Remediation {
            confidence: "high".to_string(),
            auto_fixable: true,
            commands: vec![
                format!("cargo build -p bastion-worker --features {}", provider_name),
                format!("# Or for release: cargo build -p bastion-worker --release --features {}", provider_name),
            ],
            manual_steps: vec![
                "# Ensure Rust toolchain is installed:".to_string(),
                "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh".to_string(),
                "# Then build the worker:".to_string(),
                format!("cargo build -p bastion-worker --features {}", provider_name),
                "# Verify the binary:".to_string(),
                format!("ls -la {} || echo 'Binary not found at expected path'", worker_binary_path.as_deref().unwrap_or("unknown")),
            ],
            verify_after: format!("{} --version 2>&1 || file {}", worker_binary_path.as_deref().unwrap_or("unknown"), worker_binary_path.as_deref().unwrap_or("unknown")),
            install_sources: vec![InstallSource {
                name: "bastion-worker".to_string(),
                url: "https://github.com/rbentxu/bastion#building-from-source".to_string(),
                method: "source".to_string(),
            }],
        })
    };

    RichCheckResult {
        check_id,
        check_type: "worker_binary_valid".to_string(),
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

fn default_worker_binary_for(provider: &str) -> Option<String> {
    Some(format!("/usr/local/bin/bastion-worker-{}", provider))
}

async fn validate_worker_binary(path: &Option<String>) -> (bool, std::collections::HashMap<String, String>) {
    use std::collections::HashMap;
    use std::process::Command;

    let mut details = HashMap::new();

    let path = match path {
        Some(p) => p,
        None => {
            details.insert("error".to_string(), "No worker binary path configured".to_string());
            return (false, details);
        }
    };

    // Check if file exists
    if !std::path::Path::new(path).exists() {
        details.insert("error".to_string(), format!("File not found at {}", path));
        details.insert("exists".to_string(), "false".to_string());
        return (false, details);
    }

    details.insert("exists".to_string(), "true".to_string());

    // Check if file is executable
    use std::os::unix::fs::PermissionsExt;
    if !std::path::Path::new(path).metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false) {
        details.insert("error".to_string(), format!("File at {} is not executable", path));
        details.insert("executable".to_string(), "false".to_string());
        return (false, details);
    }

    details.insert("executable".to_string(), "true".to_string());

    // Try to get version or help output
    let version_output = Command::new(path)
        .arg("--version")
        .output();

    match version_output {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            details.insert("version".to_string(), version);
            details.insert("valid".to_string(), "true".to_string());
            (true, details)
        }
        Ok(output) => {
            // Exit code non-zero doesn't necessarily mean invalid
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !stdout.is_empty() || !stderr.is_empty() {
                details.insert("output".to_string(), format!("stdout: {}, stderr: {}", stdout, stderr));
            }
            details.insert("valid".to_string(), "true (no version flag)".to_string());
            (true, details)
        }
        Err(e) => {
            details.insert("error".to_string(), format!("Failed to execute: {}", e));
            details.insert("valid".to_string(), "false".to_string());
            (false, details)
        }
    }
}