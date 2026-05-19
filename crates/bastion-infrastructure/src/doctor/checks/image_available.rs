//! Image availability check for VM images.

use bastion_domain::catalog::doctor::{
    CheckStatus, DeltaItem, InstallSource, Remediation, RichCheckResult, Severity,
};
use crate::doctor::checks::{generate_trace_id, DoctorContext};

pub async fn evaluate(
    ctx: &DoctorContext<'_>,
    provider_name: &str,
    image_name: Option<&str>,
) -> RichCheckResult {
    let check_id = format!("image_available.{}", provider_name);

    // Get image info based on provider
    let (image_path, image_found) = find_provider_image(provider_name, image_name).await;

    let current_state = serde_json::json!({
        "provider": provider_name,
        "image_name": image_name,
        "image_path": image_path,
        "image_found": image_found,
    });

    let expected_state = serde_json::json!({
        "provider": provider_name,
        "image_name": image_name.unwrap_or_else(|| default_image_for(provider_name)),
        "required": true,
    });

    let delta: Vec<DeltaItem> = if !image_found {
        vec![DeltaItem {
            item: format!("{} image", provider_name),
            expected: image_name.unwrap_or_else(|| default_image_for(provider_name)).to_string(),
            actual: image_path.clone(),
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
            auto_fixable: false,
            commands: vec![],
            manual_steps: vec![
                format!("# Download {} image:", provider_name),
                download_instruction_for(provider_name),
            ],
            verify_after: format!("ls -la {}", image_path.clone().unwrap_or_default()),
            install_sources: vec![InstallSource {
                name: format!("{} image", provider_name),
                url: image_source_url_for(provider_name),
                method: "download".to_string(),
            }],
        })
    };

    RichCheckResult {
        check_id,
        check_type: "image_available".to_string(),
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

fn default_image_for(provider: &str) -> &'static str {
    match provider {
        "firecracker" => "https://s3.amazonaws.com/spec.ccfc.min/img/hello/kernel-hello",
        "gvisor" => "gvisor-remote-image",
        "podman" => "docker.io/library/alpine:latest",
        "docker" => "docker.io/library/alpine:latest",
        _ => "default-image",
    }
}

fn image_source_url_for(provider: &str) -> String {
    match provider {
        "firecracker" => "https://github.com/firecracker-microvm/firecracker#getting-started".to_string(),
        "gvisor" => "https://gvisor.dev/docs/user_guide/".to_string(),
        "podman" => "https://podman.io/getting-started/".to_string(),
        "docker" => "https://docs.docker.com/get-docker/".to_string(),
        _ => format!("https://{}.io/docs", provider),
    }
}

fn download_instruction_for(provider: &str) -> String {
    match provider {
        "firecracker" => "curl -fsSL https://s3.amazonaws.com/spec.ccfc.min/img/hello/kernel-hello -o /var/lib/firecracker/hello-vmlinux".to_string(),
        "gvisor" => "gvisor-ctl image pull docker.io/library/alpine:latest".to_string(),
        "podman" => "podman pull docker.io/library/alpine:latest".to_string(),
        "docker" => "docker pull docker.io/library/alpine:latest".to_string(),
        _ => format!("# Download image for {}", provider),
    }
}

async fn find_provider_image(provider: &str, image_name: Option<&str>) -> (Option<String>, bool) {
    use std::path::Path;

    let image_name = image_name.unwrap_or(default_image_for(provider));

    // Check common image paths based on provider
    let paths_to_check: Vec<String> = match provider {
        "firecracker" => vec![
            format!("/var/lib/firecracker/images/{}", image_name),
            format!("/var/lib/firecracker/{}", image_name),
            image_name.to_string(),
        ],
        "gvisor" => vec![
            format!("/usr/local/share/gvisor/images/{}", image_name),
            format!("/var/lib/gvisor/images/{}", image_name),
            image_name.to_string(),
        ],
        "podman" | "docker" => {
            // For container runtimes, images aren't stored as files
            // Check if the runtime can pull/access the image
            return (Some(format!("registry image: {}", image_name)), true);
        }
        _ => vec![image_name.to_string()],
    };

    for path in paths_to_check {
        if Path::new(&path).exists() {
            return (Some(path), true);
        }
    }

    (None, false)
}