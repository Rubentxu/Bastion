//! KVM availability check.

use bastion_domain::catalog::doctor::{
    CheckStatus, DeltaItem, InstallSource, Remediation, RichCheckResult, Severity,
};
use crate::doctor::checks::{generate_trace_id, DoctorContext};

pub async fn evaluate(ctx: &DoctorContext<'_>) -> RichCheckResult {
    let check_id = "kvm_available".to_string();

    let kvm_path = std::path::Path::new("/dev/kvm");
    let kvm_exists = kvm_path.exists();

    // Check if we have nested virtualization (important for VMs within VMs)
    let nested_virt = ctx.system_context.has_nested_virt.unwrap_or(false);

    // Check CPU virtualization extensions
    let cpu_virt_ext = check_cpu_virtualization_extensions();

    let current_state = serde_json::json!({
        "kvm_device_exists": kvm_exists,
        "kvm_path": "/dev/kvm",
        "has_nested_virt": nested_virt,
        "cpu_virtualization_extensions": cpu_virt_ext,
    });

    let expected_state = serde_json::json!({
        "kvm_required": true,
        "kvm_path": "/dev/kvm",
        "nested_virt_desirable": true,
    });

    let mut delta: Vec<DeltaItem> = Vec::new();

    if !kvm_exists {
        delta.push(DeltaItem {
            item: "/dev/kvm".to_string(),
            expected: "KVM device available".to_string(),
            actual: Some("KVM device not found".to_string()),
            severity: Severity::Critical,
        });
    }

    if !cpu_virt_ext {
        delta.push(DeltaItem {
            item: "CPU virtualization extensions".to_string(),
            expected: "VT-x (Intel) or AMD-V (AMD) enabled".to_string(),
            actual: Some("CPU virtualization not detected".to_string()),
            severity: Severity::Critical,
        });
    }

    if !nested_virt {
        delta.push(DeltaItem {
            item: "Nested virtualization".to_string(),
            expected: "Nested virtualization supported".to_string(),
            actual: Some("Nested virtualization not enabled".to_string()),
            severity: Severity::Warning,
        });
    }

    let remediation = if delta.is_empty() {
        None
    } else {
        Some(Remediation {
            confidence: if !kvm_exists || !cpu_virt_ext {
                "high".to_string()
            } else {
                "medium".to_string()
            },
            auto_fixable: false,
            commands: if !nested_virt {
                vec![
                    "# Enable nested virtualization for Intel CPUs:".to_string(),
                    "sudo modprobe kvm_intel nested=1".to_string(),
                    "# Or persist across reboots:".to_string(),
                    "echo 'options kvm_intel nested=1' | sudo tee /etc/modprobe.d/kvm-intel.conf".to_string(),
                ]
            } else {
                vec![]
            },
            manual_steps: if !kvm_exists || !cpu_virt_ext {
                vec![
                    "# Check if your CPU supports virtualization:".to_string(),
                    "grep -E '(vmx|svm)' /proc/cpuinfo".to_string(),
                    "# If no output, your CPU doesn't support hardware virtualization".to_string(),
                    "# If yes, check if KVM module is loaded:".to_string(),
                    "lsmod | grep kvm".to_string(),
                    "# Load KVM modules if not loaded:".to_string(),
                    "sudo modprobe kvm".to_string(),
                    "sudo modprobe kvm-intel  # for Intel CPUs".to_string(),
                    "sudo modprobe kvm-amd    # for AMD CPUs".to_string(),
                ]
            } else {
                vec![]
            },
            verify_after: "ls -la /dev/kvm".to_string(),
            install_sources: vec![
                InstallSource {
                    name: "KVM".to_string(),
                    url: "https://www.linux-kvm.org/page/Main_Page".to_string(),
                    method: "package_manager".to_string(),
                },
                InstallSource {
                    name: "QEMU".to_string(),
                    url: "https://www.qemu.org/download/".to_string(),
                    method: "package_manager".to_string(),
                },
            ],
        })
    };

    RichCheckResult {
        check_id,
        check_type: "kvm_available".to_string(),
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

fn check_cpu_virtualization_extensions() -> bool {
    use std::fs;

    // Check for Intel VT-x (vmx) or AMD-V (svm)
    if let Ok(content) = fs::read_to_string("/proc/cpuinfo") {
        for line in content.lines() {
            if line.starts_with("flags") && (line.contains("vmx") || line.contains("svm")) {
                return true;
            }
        }
    }

    false
}