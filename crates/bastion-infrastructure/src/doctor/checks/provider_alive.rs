//! Provider alive check for podman/docker daemon.

use bastion_domain::catalog::doctor::{
    CheckStatus, DeltaItem, InstallSource, Remediation, RichCheckResult, Severity,
};
use crate::doctor::checks::{generate_trace_id, DoctorContext};

pub async fn evaluate(
    ctx: &DoctorContext<'_>,
    provider_name: &str,
) -> RichCheckResult {
    let check_id = format!("provider_alive.{}", provider_name);

    let config = ctx.provider_registry.get_config(provider_name);
    let socket_path = config
        .as_ref()
        .and_then(|c| c.socket.clone())
        .unwrap_or_else(|| default_socket_for(provider_name));

    let ping_result = ping_daemon(provider_name, &socket_path).await;

    let current_state = serde_json::json!({
        "provider": provider_name,
        "socket_path": socket_path,
        "ping_result": ping_result,
    });

    let expected_state = serde_json::json!({
        "provider": provider_name,
        "daemon_required": true,
        "ping_timeout_ms": 5000,
    });

    let delta: Vec<DeltaItem> = match &ping_result {
        Ok(_) => vec![],
        Err(e) => vec![DeltaItem {
            item: format!("{} daemon", provider_name),
            expected: "daemon responding to ping".to_string(),
            actual: Some(e.clone()),
            severity: Severity::Critical,
        }],
    };

    let remediation = if delta.is_empty() {
        None
    } else {
        Some(Remediation {
            confidence: "high".to_string(),
            auto_fixable: true,
            commands: vec![
                format!("systemctl --user start {}socket", provider_name),
                format!("systemctl --user enable {}socket", provider_name),
                "# Or for system-wide:".to_string(),
                format!("sudo systemctl start {}socket", provider_name),
                format!("sudo systemctl enable {}socket", provider_name),
            ],
            manual_steps: vec![],
            verify_after: format!("{} version", provider_name),
            install_sources: vec![InstallSource {
                name: provider_name.to_string(),
                url: format!("https://{}.io/docs/installation", provider_name),
                method: "package_manager".to_string(),
            }],
        })
    };

    RichCheckResult {
        check_id,
        check_type: "provider_alive".to_string(),
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

fn default_socket_for(provider: &str) -> String {
    match provider {
        "podman" => "/run/user/1000/podman/podman.sock".to_string(),
        "docker" => "/var/run/docker.sock".to_string(),
        _ => "/var/run/unknown.sock".to_string(),
    }
}

async fn ping_daemon(provider: &str, socket: &str) -> Result<String, String> {
    match provider {
        "podman" => ping_podman(socket).await,
        "docker" => ping_docker(socket).await,
        _ => Err(format!("Unknown provider: {}", provider)),
    }
}

async fn ping_podman(socket_path: &str) -> Result<String, String> {
    use std::os::unix::net::UnixStream;
    use std::path::Path;

    if !Path::new(socket_path).exists() {
        return Err(format!("Socket not found at {}", socket_path));
    }

    UnixStream::connect(socket_path)
        .map_err(|e| format!("Failed to connect to socket: {}", e))?;

    Ok("podman daemon is responsive".to_string())
}

async fn ping_docker(socket_path: &str) -> Result<String, String> {
    use std::os::unix::net::UnixStream;

    if !std::path::Path::new(socket_path).exists() {
        return Err(format!("Socket not found at {}", socket_path));
    }

    UnixStream::connect(socket_path)
        .map_err(|e| format!("Failed to connect to docker socket: {}", e))?;

    Ok("docker daemon is responsive".to_string())
}
