//! Rich doctor check result with full context for AI agents.
//!
//! Provides detailed state, expected state, delta, and remediation
//! for each check to enable autonomous fixing.

pub mod checks;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Status of a rich check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichCheckStatus {
    Pass,
    Fail,
    Skip,
    Error,
}

/// A rich check result that provides full context for AI agent remediation.
/// Unlike the simple CheckResult (which just has pass/fail), this includes
/// current state, expected state, delta, and actionable remediation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichCheckResult {
    /// Unique check identifier (e.g., "provider_alive.podman")
    pub check_id: String,
    /// Type of check (e.g., "provider_alive", "binary_available")
    pub check_type: String,
    /// Pass/Fail/Skip/Error status
    pub status: RichCheckStatus,
    /// Current observed state as JSON
    pub current_state: serde_json::Value,
    /// Expected state from config as JSON
    pub expected_state: serde_json::Value,
    /// Delta between current and expected (what's wrong)
    pub delta: HashMap<String, String>,
    /// Actionable remediation commands (if failed)
    pub remediation: Option<RichRemediation>,
    /// System context snapshot at check time
    pub system_context: SystemContext,
    /// Trace ID for correlation
    pub trace_id: String,
    /// When the check was executed
    pub executed_at: DateTime<Utc>,
}

/// Remediation steps for a failed check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichRemediation {
    /// Summary of what needs to be fixed
    pub summary: String,
    /// Step-by-step commands to fix the issue
    pub steps: Vec<RemediationStep>,
    /// Estimated time to fix (human-readable)
    pub estimated_time: Option<String>,
    /// URL to relevant documentation
    pub docs_url: Option<String>,
}

/// A single remediation step with command and verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationStep {
    /// Step number (1-based)
    pub step: usize,
    /// Command to execute (copy-pasteable)
    pub command: String,
    /// What this step does
    pub description: String,
    /// Command to verify the step worked (optional)
    pub verify_after: Option<String>,
}

/// System context captured at check time.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemContext {
    /// Operating system (e.g., "Linux")
    pub os: Option<String>,
    /// OS version (e.g., "5.4.0-generic")
    pub os_version: Option<String>,
    /// Architecture (e.g., "x86_64", "aarch64")
    pub architecture: Option<String>,
    /// Kernel version
    pub kernel: Option<String>,
    /// Whether /dev/kvm exists
    pub kvm_exists: bool,
    /// /dev/kvm permissions (octal string like "0666")
    pub kvm_permissions: Option<String>,
    /// Whether user is in kvm group
    pub in_kvm_group: bool,
    /// Installed binaries found (binary name -> path)
    pub installed_binaries: HashMap<String, Option<String>>,
}

impl SystemContext {
    /// Create a new empty system context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an installed binary to the context.
    pub fn with_binary(mut self, name: &str, path: Option<String>) -> Self {
        self.installed_binaries.insert(name.to_string(), path);
        self
    }
}

/// Gather system context for doctor checks.
///
/// Collects OS info, architecture, KVM status, and installed binaries.
pub fn gather_system_context() -> SystemContext {
    let mut ctx = SystemContext::new();

    // Get OS and version
    #[cfg(target_os = "linux")]
    {
        ctx.os = Some("Linux".to_string());

        // Read /etc/os-release for distribution info
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if line.starts_with("VERSION=") {
                    ctx.os_version = Some(line.trim_start_matches("VERSION=").trim_matches('"').to_string());
                }
                if line.starts_with("PRETTY_NAME=") {
                    if ctx.os_version.is_none() {
                        ctx.os_version = Some(line.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string());
                    }
                }
            }
        }

        // Get kernel version
        if let Ok(uname) = std::process::Command::new("uname")
            .arg("-r")
            .output()
        {
            ctx.kernel = Some(String::from_utf8_lossy(&uname.stdout).trim().to_string());
        }
    }

    #[cfg(target_os = "macos")]
    {
        ctx.os = Some("macOS".to_string());
        if let Ok(sw_vers) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
        {
            ctx.os_version = Some(String::from_utf8_lossy(&sw_vers.stdout).trim().to_string());
        }
    }

    // Get architecture
    #[cfg(target_arch = "x86_64")]
    {
        ctx.architecture = Some("x86_64".to_string());
    }
    #[cfg(target_arch = "aarch64")]
    {
        ctx.architecture = Some("aarch64".to_string());
    }

    // Check KVM status
    let kvm_path = "/dev/kvm";
    ctx.kvm_exists = Path::new(kvm_path).exists();
    if ctx.kvm_exists {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(kvm_path) {
            ctx.kvm_permissions = Some(format!("{:o}", metadata.permissions().mode() & 0o777));
        }
        // Check if current user is in kvm group
        if let Ok(output) = std::process::Command::new("id")
            .args(["-Gn"])
            .output()
        {
            let groups = String::from_utf8_lossy(&output.stdout);
            ctx.in_kvm_group = groups.split_whitespace().any(|g| g == "kvm" || g == "libvirt");
        }
    }

    // Check installed binaries
    let binaries_to_check = ["podman", "docker", "runsc", "firecracker", "which"];
    for binary in binaries_to_check {
        let path = std::process::Command::new("which")
            .arg(binary)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        ctx.installed_binaries.insert(binary.to_string(), path);
    }

    ctx
}

impl RichCheckResult {
    /// Create a new rich check result.
    pub fn new(
        check_id: String,
        check_type: String,
        status: RichCheckStatus,
        current_state: serde_json::Value,
        expected_state: serde_json::Value,
        delta: HashMap<String, String>,
        remediation: Option<RichRemediation>,
        system_context: SystemContext,
        trace_id: String,
    ) -> Self {
        Self {
            check_id,
            check_type,
            status,
            current_state,
            expected_state,
            delta,
            remediation,
            system_context,
            trace_id,
            executed_at: Utc::now(),
        }
    }

    /// Create a passing result.
    pub fn pass(
        check_id: String,
        check_type: String,
        current_state: serde_json::Value,
        expected_state: serde_json::Value,
        system_context: SystemContext,
        trace_id: String,
    ) -> Self {
        Self::new(
            check_id,
            check_type,
            RichCheckStatus::Pass,
            current_state,
            expected_state,
            HashMap::new(),
            None,
            system_context,
            trace_id,
        )
    }

    /// Create a failing result.
    pub fn fail(
        check_id: String,
        check_type: String,
        current_state: serde_json::Value,
        expected_state: serde_json::Value,
        delta: HashMap<String, String>,
        remediation: RichRemediation,
        system_context: SystemContext,
        trace_id: String,
    ) -> Self {
        Self::new(
            check_id,
            check_type,
            RichCheckStatus::Fail,
            current_state,
            expected_state,
            delta,
            Some(remediation),
            system_context,
            trace_id,
        )
    }
}
