//! Doctor check evaluators for provider readiness.

pub mod binary_available;
pub mod image_available;
pub mod kvm_available;
pub mod provider_alive;
pub mod worker_binary_valid;

pub use binary_available::evaluate as evaluate_binary_available;
pub use image_available::evaluate as evaluate_image_available;
pub use kvm_available::evaluate as evaluate_kvm_available;
pub use provider_alive::evaluate as evaluate_provider_alive;
pub use worker_binary_valid::evaluate as evaluate_worker_binary_valid;

use bastion_domain::catalog::doctor::SystemContext;
use crate::provider::ProviderRegistry;

/// Context passed to all doctor check evaluators.
#[derive(Clone)]
pub struct DoctorContext<'a> {
    /// Provider registry for accessing provider configurations.
    pub provider_registry: &'a ProviderRegistry,
    /// System context captured at check time.
    pub system_context: SystemContext,
}

impl<'a> DoctorContext<'a> {
    /// Create a new doctor context.
    pub fn new(provider_registry: &'a ProviderRegistry, system_context: SystemContext) -> Self {
        Self {
            provider_registry,
            system_context,
        }
    }
}

/// Generate a trace ID for correlation.
pub fn generate_trace_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Gather system context information for doctor checks.
pub fn gather_system_context() -> SystemContext {
    use std::process::Command;

    let os = std::env::consts::OS.to_string();
    let architecture = std::env::consts::ARCH.to_string();

    let os_version = get_os_version();
    let kernel = Command::new("uname")
        .arg("-r")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let kvm_path = std::path::Path::new("/dev/kvm");
    let has_kvm = kvm_path.exists();

    let has_nested_virt = check_nested_virt();
    let relevant_binaries = check_relevant_binaries();
    let installed_providers = check_installed_providers();

    SystemContext {
        os,
        os_version,
        architecture,
        kernel,
        has_kvm,
        has_nested_virt: Some(has_nested_virt),
        relevant_binaries,
        installed_providers,
    }
}

fn get_os_version() -> String {
    use std::process::Command;
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if line.starts_with("PRETTY_NAME=") || line.starts_with("VERSION=") {
                if let Some(value) = line.splitn(2, '=').nth(1) {
                    let value = value.trim_matches('"').trim();
                    if !value.is_empty() {
                        return value.to_string();
                    }
                }
            }
        }
    }
    Command::new("uname")
        .arg("-v")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn check_nested_virt() -> bool {
    use std::fs;
    if let Ok(content) = fs::read_to_string("/sys/module/kvm_intel/parameters/nested") {
        return content.trim() == 'Y'.to_string();
    }
    if let Ok(content) = fs::read_to_string("/sys/module/kvm_amd/parameters/nested") {
        return content.trim() == 'Y'.to_string();
    }
    false
}

fn check_relevant_binaries() -> std::collections::HashMap<String, bastion_domain::catalog::doctor::BinaryInfo> {
    use std::collections::HashMap;
    use std::process::Command;

    let binaries = ["podman", "docker", "firecracker", "runsc", "kubectl", "helm"];
    let mut result = HashMap::new();

    for name in binaries {
        let path = Command::new("which")
            .arg(name)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

        let version = path.as_ref().and_then(|p| get_binary_version(name));

        result.insert(
            name.to_string(),
            bastion_domain::catalog::doctor::BinaryInfo {
                name: name.to_string(),
                path,
                version,
            },
        );
    }
    result
}

fn get_binary_version(binary: &str) -> Option<String> {
    use std::process::Command;
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(binary).arg("version").output())
        .ok()?;
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(version.lines().next().unwrap_or(&version).to_string())
    } else {
        None
    }
}

fn check_installed_providers() -> std::collections::HashMap<String, bastion_domain::catalog::doctor::ProviderInfo> {
    use std::collections::HashMap;
    use std::process::Command;

    let mut providers = HashMap::new();

    let check_provider = |name: &str, binary: &str| {
        let path = Command::new("which").arg(binary).output().ok();
        let available = path.as_ref().map(|o| o.status.success()).unwrap_or(false);
        bastion_domain::catalog::doctor::ProviderInfo {
            name: name.to_string(),
            version: available.then(|| get_binary_version(binary)).flatten(),
            available,
        }
    };

    providers.insert("podman".to_string(), check_provider("podman", "podman"));
    providers.insert("docker".to_string(), check_provider("docker", "docker"));
    providers.insert("firecracker".to_string(), check_provider("firecracker", "firecracker"));
    providers.insert("gvisor".to_string(), check_provider("gvisor", "runsc"));

    providers
}
