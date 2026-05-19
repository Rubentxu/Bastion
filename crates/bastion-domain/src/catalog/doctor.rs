//! Doctor catalog domain types.
//!
//! Doctor descriptors are TOML-loaded health check primitives that evaluate
//! against live sandbox state or existing experience records.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::assertion::CheckResult;

// Re-export CheckResult from assertion module for use in DoctorResult
pub use super::assertion::CheckResult as AssertionCheckResult;

/// Severity level for a doctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    #[default]
    Warning,
    Info,
}

/// Status result of a doctor run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Fail,
    Skip,
    Error,
}

/// Descriptor for a reusable doctor, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorDescriptor {
    /// Unique doctor identifier (e.g. "sandbox.alive").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this doctor validates.
    pub description: String,
    /// Category for grouping (e.g. "sandbox", "gateway", "provider").
    pub category: String,
    /// Severity level of this doctor.
    pub severity: Severity,
    /// Ordered list of checks to evaluate.
    pub checks: Vec<DoctorCheck>,
}

/// A single check within a doctor descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DoctorCheck {
    /// Check if a sandbox is alive via provider.
    Aliveness {
        /// Optional sandbox ID override. If omitted, uses the sandbox_id from doctor_run.
        sandbox_id: Option<String>,
    },
    /// Check resource usage against thresholds.
    Resources {
        /// Maximum total sandboxes across all pools.
        max_total: Option<usize>,
        /// Maximum idle sandboxes per template.
        max_idle_per_template: Option<usize>,
    },
    /// Delegate to an existing assertion by name.
    AssertionDriven {
        /// The assertion ID to evaluate against experience records.
        assertion_id: String,
    },
    /// Check if a provider is alive/responsive.
    ProviderAlive {
        /// Provider name (e.g., "podman", "firecracker").
        provider: String,
    },
    /// Check if a binary is available in PATH or expected location.
    BinaryAvailable {
        /// Binary name.
        name: String,
        /// Optional expected path to the binary.
        expected_path: Option<String>,
    },
    /// Check if a VM image is available for a provider.
    ImageAvailable {
        /// Provider name.
        provider: String,
        /// Optional specific image name/path.
        image: Option<String>,
    },
    /// Check if KVM virtualization is available.
    KvmAvailable,
    /// Check if provider capabilities meet minimum requirements.
    CapabilitiesMet {
        /// Provider name.
        provider: String,
        /// Minimum required memory in MB.
        min_memory_mb: Option<u64>,
        /// Minimum required CPU count.
        min_cpu_count: Option<u32>,
    },
    /// Check if provider configuration is valid.
    ConfigValid {
        /// Provider name.
        provider: String,
    },
    /// Check if worker binary is valid for a provider.
    WorkerBinaryValid {
        /// Provider name.
        provider: String,
    },
}

/// Result of running a doctor against a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorResult {
    /// The doctor ID that was evaluated.
    pub doctor_id: String,
    /// The sandbox ID that was checked.
    pub sandbox_id: Option<String>,
    /// Overall status of the doctor run.
    pub status: DoctorStatus,
    /// Severity from the doctor descriptor.
    pub severity: Severity,
    /// Trace ID for correlation.
    pub trace_id: String,
    /// Per-check results (simple format).
    pub check_results: Vec<CheckResult>,
    /// Human-readable summary of why the doctor passed/failed.
    pub rationale: String,
    /// When the doctor was executed.
    pub executed_at: chrono::DateTime<chrono::Utc>,
    /// Rich results for AI agents.
    pub rich_check_results: Vec<RichCheckResult>,
    /// Simple summary for humans.
    pub summary: String,
    /// AI-specific flag indicating attention is needed.
    pub requires_ai_attention: bool,
    /// AI-specific flag indicating the issue may be self-remediated.
    pub potential_self_remediation: bool,
}

/// Status result of a single check within RichCheckResult.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warning,
    Skip,
}

/// Detailed result of a single check, designed for AI agent context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichCheckResult {
    /// Unique identifier for this check.
    pub check_id: String,
    /// Type of check performed.
    pub check_type: String,
    /// Status of the check.
    pub status: CheckStatus,
    /// What actually exists/found in the system.
    pub current_state: serde_json::Value,
    /// What the config/provider expects.
    pub expected_state: serde_json::Value,
    /// What's missing or wrong.
    pub delta: Vec<DeltaItem>,
    /// Remediation steps for AI agents.
    pub remediation: Option<Remediation>,
    /// System context information.
    pub system_context: SystemContext,
    /// Trace ID for correlation.
    pub trace_id: String,
    /// When the check was executed.
    pub executed_at: chrono::DateTime<chrono::Utc>,
}

/// A single delta item describing what's missing or wrong.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaItem {
    /// The item name or path.
    pub item: String,
    /// What was expected.
    pub expected: String,
    /// What was actually found (None if not found).
    pub actual: Option<String>,
    /// Severity level of this delta.
    pub severity: Severity,
}

/// Remediation instructions for AI agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remediation {
    /// Confidence level: "high", "medium", or "low".
    pub confidence: String,
    /// Whether this can be fixed automatically.
    pub auto_fixable: bool,
    /// Commands to run for automatic remediation.
    pub commands: Vec<String>,
    /// Manual steps if auto-fix is not possible.
    pub manual_steps: Vec<String>,
    /// Command to verify the fix worked.
    pub verify_after: String,
    /// Sources for installing missing components.
    pub install_sources: Vec<InstallSource>,
}

/// Source for installing a missing component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSource {
    /// Name of the component.
    pub name: String,
    /// URL to download or install from.
    pub url: String,
    /// Installation method: "script", "package_manager", or "source".
    pub method: String,
}

/// System context information for debugging and remediation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    /// Operating system name (e.g., "linux", "macos", "windows").
    pub os: String,
    /// Operating system version.
    pub os_version: String,
    /// CPU architecture (e.g., "x86_64", "aarch64").
    pub architecture: String,
    /// Kernel version or name.
    pub kernel: String,
    /// Whether KVM virtualization is available.
    pub has_kvm: bool,
    /// Whether nested virtualization is supported (if known).
    pub has_nested_virt: Option<bool>,
    /// Map of relevant binary names to their info.
    pub relevant_binaries: HashMap<String, BinaryInfo>,
    /// Map of installed provider names to their info.
    pub installed_providers: HashMap<String, ProviderInfo>,
}

/// Information about a binary on the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryInfo {
    /// Binary name.
    pub name: String,
    /// Path to the binary if found.
    pub path: Option<String>,
    /// Version string if available.
    pub version: Option<String>,
}

/// Information about an installed provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Provider name.
    pub name: String,
    /// Provider version if available.
    pub version: Option<String>,
    /// Whether the provider is currently available.
    pub available: bool,
}

impl DoctorResult {
    /// Create a new doctor result.
    pub fn new(
        doctor_id: String,
        sandbox_id: Option<String>,
        trace_id: String,
        severity: Severity,
    ) -> Self {
        Self {
            doctor_id,
            sandbox_id,
            status: DoctorStatus::Pass,
            severity,
            trace_id,
            check_results: Vec::new(),
            rationale: String::new(),
            executed_at: chrono::Utc::now(),
            rich_check_results: Vec::new(),
            summary: String::new(),
            requires_ai_attention: false,
            potential_self_remediation: false,
        }
    }

    /// Add a rich check result.
    pub fn add_rich_check_result(&mut self, result: RichCheckResult) {
        self.rich_check_results.push(result);
    }

    /// Set the summary.
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = summary.into();
    }

    /// Add a check result.
    pub fn add_check_result(&mut self, result: CheckResult) {
        self.check_results.push(result);
    }

    /// Set the rationale.
    pub fn set_rationale(&mut self, rationale: impl Into<String>) {
        self.rationale = rationale.into();
    }

    /// Finalize the result based on check outcomes.
    pub fn finalize(&mut self) {
        self.status = if self.check_results.is_empty() {
            DoctorStatus::Skip
        } else if self.check_results.iter().all(|r| r.passed) {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_default() {
        assert_eq!(Severity::default(), Severity::Warning);
    }

    #[test]
    fn test_doctor_check_serialization() {
        // Test Aliveness variant
        let check = DoctorCheck::Aliveness { sandbox_id: None };
        let json = serde_json::to_string(&check).unwrap();
        assert!(json.contains("\"type\":\"aliveness\""));

        // Test Resources variant
        let check = DoctorCheck::Resources {
            max_total: Some(100),
            max_idle_per_template: Some(10),
        };
        let json = serde_json::to_string(&check).unwrap();
        assert!(json.contains("\"type\":\"resources\""));
        assert!(json.contains("\"max_total\":100"));

        // Test AssertionDriven variant
        let check = DoctorCheck::AssertionDriven {
            assertion_id: "command.exit_code.zero".to_string(),
        };
        let json = serde_json::to_string(&check).unwrap();
        assert!(json.contains("\"type\":\"assertion_driven\""));
        assert!(json.contains("\"assertion_id\":\"command.exit_code.zero\""));
    }

    #[test]
    fn test_doctor_check_deserialization() {
        // Test Aliveness
        let json = r#"{"type":"aliveness","sandbox_id":null}"#;
        let check: DoctorCheck = serde_json::from_str(json).unwrap();
        assert!(matches!(check, DoctorCheck::Aliveness { sandbox_id: None }));

        // Test Resources
        let json = r#"{"type":"resources","max_total":200,"max_idle_per_template":20}"#;
        let check: DoctorCheck = serde_json::from_str(json).unwrap();
        assert!(matches!(
            check,
            DoctorCheck::Resources {
                max_total: Some(200),
                max_idle_per_template: Some(20)
            }
        ));

        // Test AssertionDriven
        let json = r#"{"type":"assertion_driven","assertion_id":"maven.build.success"}"#;
        let check: DoctorCheck = serde_json::from_str(json).unwrap();
        match check {
            DoctorCheck::AssertionDriven { assertion_id } => {
                assert_eq!(assertion_id, "maven.build.success");
            }
            other => panic!("Expected AssertionDriven, got {:?}", other),
        }
    }

    #[test]
    fn test_doctor_descriptor_round_trip() {
        let descriptor = DoctorDescriptor {
            id: "sandbox.alive".to_string(),
            name: "Sandbox Alive".to_string(),
            description: "Checks that a sandbox is alive".to_string(),
            category: "sandbox".to_string(),
            severity: Severity::Critical,
            checks: vec![
                DoctorCheck::Aliveness { sandbox_id: None },
                DoctorCheck::Resources {
                    max_total: Some(100),
                    max_idle_per_template: Some(10),
                },
            ],
        };

        let json = serde_json::to_string(&descriptor).unwrap();
        let parsed: DoctorDescriptor = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, descriptor.id);
        assert_eq!(parsed.name, descriptor.name);
        assert_eq!(parsed.checks.len(), 2);
    }

    #[test]
    fn test_doctor_result_finalize_all_pass() {
        let mut result = DoctorResult::new(
            "sandbox.alive".to_string(),
            Some("sb-123".to_string()),
            "trace-1".to_string(),
            Severity::Critical,
        );
        result.add_check_result(CheckResult {
            check: "Aliveness".to_string(),
            passed: true,
            reason: None,
        });
        result.add_check_result(CheckResult {
            check: "Resources".to_string(),
            passed: true,
            reason: None,
        });
        result.finalize();

        assert_eq!(result.status, DoctorStatus::Pass);
    }

    #[test]
    fn test_doctor_result_finalize_one_fail() {
        let mut result = DoctorResult::new(
            "sandbox.alive".to_string(),
            Some("sb-123".to_string()),
            "trace-1".to_string(),
            Severity::Critical,
        );
        result.add_check_result(CheckResult {
            check: "Aliveness".to_string(),
            passed: true,
            reason: None,
        });
        result.add_check_result(CheckResult {
            check: "Resources".to_string(),
            passed: false,
            reason: Some("CPU usage exceeded 90%".to_string()),
        });
        result.finalize();

        assert_eq!(result.status, DoctorStatus::Fail);
    }

    #[test]
    fn test_doctor_result_finalize_empty() {
        let mut result = DoctorResult::new(
            "empty.doctor".to_string(),
            Some("sb-123".to_string()),
            "trace-1".to_string(),
            Severity::Warning,
        );
        result.finalize();

        assert_eq!(result.status, DoctorStatus::Skip);
    }
}
