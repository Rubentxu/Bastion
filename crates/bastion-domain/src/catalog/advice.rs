//! Advice catalog domain types.
//!
//! Advice descriptors are TOML-loaded guidance primitives that provide
//! context-aware suggestions based on assertion failures, doctor failures,
//! and experience patterns.

use serde::{Deserialize, Serialize};

/// Severity for advice relevance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AdviceSeverity {
    Critical,
    #[default]
    Warning,
    Hint,
}

impl AdviceSeverity {
    /// Sort key for severity ordering (lower = more severe).
    pub fn sort_key(&self) -> u8 {
        match self {
            AdviceSeverity::Critical => 0,
            AdviceSeverity::Warning => 1,
            AdviceSeverity::Hint => 2,
        }
    }
}

/// Trigger condition for advice activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdviceTrigger {
    /// Triggered when an assertion fails.
    AssertionFailed {
        /// The assertion ID that must fail to activate this advice.
        assertion_id: String,
    },
    /// Triggered when a doctor fails.
    DoctorFailed {
        /// The doctor ID that must fail to activate this advice.
        doctor_id: String,
    },
    /// Triggered by experience pattern (MVP: tool name + status + count threshold).
    ExperiencePattern {
        /// The tool name to match (e.g., "sandbox_run", "sandbox_prepare").
        tool_name: String,
        /// The experience status to match ("failure", "success", "timeout", "cancelled").
        status: String,
        /// Minimum number of matching experiences needed to activate.
        threshold: u32,
    },
}

/// Descriptor for a reusable advice, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdviceDescriptor {
    /// Stable advice identifier (e.g., "maven.build.failure").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this advice addresses.
    pub description: String,
    /// Category for grouping (e.g., "maven", "sandbox", "provider").
    pub category: String,
    /// Severity level of this advice.
    pub severity: AdviceSeverity,
    /// Ordered list of trigger conditions that activate this advice.
    pub triggers: Vec<AdviceTrigger>,
    /// The advice message to present to the user.
    pub message: String,
    /// Ordered list of suggested actions to resolve the issue.
    pub suggested_actions: Vec<String>,
    /// Optional hint for additional context.
    #[serde(default)]
    pub hint: Option<String>,
}

/// Result of advice evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdviceResult {
    /// The advice ID that was matched.
    pub advice_id: String,
    /// The trigger condition that caused the match.
    pub triggered_by: AdviceTrigger,
    /// The advice message.
    pub message: String,
    /// Additional context from the trigger (e.g., assertion_id that failed).
    #[serde(default)]
    pub context: serde_json::Value,
    /// Ordered list of suggested actions.
    pub suggested_actions: Vec<String>,
    /// Severity from the advice descriptor.
    pub severity: AdviceSeverity,
}

impl AdviceResult {
    /// Create a new advice result.
    pub fn new(
        advice_id: String,
        triggered_by: AdviceTrigger,
        message: String,
        suggested_actions: Vec<String>,
        severity: AdviceSeverity,
    ) -> Self {
        Self {
            advice_id,
            triggered_by,
            message,
            context: serde_json::Value::Null,
            suggested_actions,
            severity,
        }
    }

    /// Set the context value.
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = context;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_default() {
        assert_eq!(AdviceSeverity::default(), AdviceSeverity::Warning);
    }

    #[test]
    fn test_severity_sort_key() {
        assert_eq!(AdviceSeverity::Critical.sort_key(), 0);
        assert_eq!(AdviceSeverity::Warning.sort_key(), 1);
        assert_eq!(AdviceSeverity::Hint.sort_key(), 2);
    }

    #[test]
    fn test_severity_ordering() {
        let mut severities = vec![
            AdviceSeverity::Hint,
            AdviceSeverity::Critical,
            AdviceSeverity::Warning,
        ];
        severities.sort_by_key(|s| s.sort_key());
        assert_eq!(
            severities,
            vec![
                AdviceSeverity::Critical,
                AdviceSeverity::Warning,
                AdviceSeverity::Hint
            ]
        );
    }

    #[test]
    fn test_trigger_assertion_failed_serialization() {
        let trigger = AdviceTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"assertion_failed\""));
        assert!(json.contains("\"assertion_id\":\"maven.build.success\""));
    }

    #[test]
    fn test_trigger_assertion_failed_deserialization() {
        let json = r#"{"type":"assertion_failed","assertion_id":"maven.build.success"}"#;
        let trigger: AdviceTrigger = serde_json::from_str(json).unwrap();
        match trigger {
            AdviceTrigger::AssertionFailed { assertion_id } => {
                assert_eq!(assertion_id, "maven.build.success");
            }
            other => panic!("Expected AssertionFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_trigger_doctor_failed_serialization() {
        let trigger = AdviceTrigger::DoctorFailed {
            doctor_id: "sandbox.alive".to_string(),
        };
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"doctor_failed\""));
        assert!(json.contains("\"doctor_id\":\"sandbox.alive\""));
    }

    #[test]
    fn test_trigger_experience_pattern_serialization() {
        let trigger = AdviceTrigger::ExperiencePattern {
            tool_name: "sandbox_run".to_string(),
            status: "failure".to_string(),
            threshold: 3,
        };
        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("\"type\":\"experience_pattern\""));
        assert!(json.contains("\"tool_name\":\"sandbox_run\""));
        assert!(json.contains("\"status\":\"failure\""));
        assert!(json.contains("\"threshold\":3"));
    }

    #[test]
    fn test_advice_descriptor_round_trip() {
        let descriptor = AdviceDescriptor {
            id: "maven.build.failure".to_string(),
            name: "Maven Build Failure".to_string(),
            description: "Triggered when a Maven build assertion fails".to_string(),
            category: "maven".to_string(),
            severity: AdviceSeverity::Warning,
            triggers: vec![AdviceTrigger::AssertionFailed {
                assertion_id: "maven.build.success".to_string(),
            }],
            message: "Build failed. Check the output for compilation errors.".to_string(),
            suggested_actions: vec![
                "Review Maven output for compilation errors".to_string(),
                "Check for syntax errors".to_string(),
            ],
            hint: Some("Check the full build log".to_string()),
        };

        let json = serde_json::to_string(&descriptor).unwrap();
        let parsed: AdviceDescriptor = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, descriptor.id);
        assert_eq!(parsed.name, descriptor.name);
        assert_eq!(parsed.severity, descriptor.severity);
        assert_eq!(parsed.triggers.len(), 1);
        assert_eq!(parsed.suggested_actions.len(), 2);
        assert_eq!(parsed.hint.as_deref(), Some("Check the full build log"));
    }

    #[test]
    fn test_advice_result_creation() {
        let trigger = AdviceTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        let result = AdviceResult::new(
            "maven.build.failure".to_string(),
            trigger.clone(),
            "Build failed".to_string(),
            vec!["Check logs".to_string()],
            AdviceSeverity::Warning,
        );

        assert_eq!(result.advice_id, "maven.build.failure");
        assert_eq!(result.severity, AdviceSeverity::Warning);
        assert!(result.context.is_null());
    }

    #[test]
    fn test_advice_result_with_context() {
        let trigger = AdviceTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        let context = serde_json::json!({
            "assertion_id": "maven.build.success",
            "exit_code": 1
        });
        let result = AdviceResult::new(
            "maven.build.failure".to_string(),
            trigger,
            "Build failed".to_string(),
            vec![],
            AdviceSeverity::Critical,
        )
        .with_context(context.clone());

        assert_eq!(result.context, context);
    }

    #[test]
    fn test_advice_result_sorting_by_severity() {
        let results = vec![
            AdviceResult::new(
                "hint.advice".to_string(),
                AdviceTrigger::AssertionFailed {
                    assertion_id: "x".to_string(),
                },
                "Hint".to_string(),
                vec![],
                AdviceSeverity::Hint,
            ),
            AdviceResult::new(
                "critical.advice".to_string(),
                AdviceTrigger::AssertionFailed {
                    assertion_id: "x".to_string(),
                },
                "Critical".to_string(),
                vec![],
                AdviceSeverity::Critical,
            ),
            AdviceResult::new(
                "warning.advice".to_string(),
                AdviceTrigger::AssertionFailed {
                    assertion_id: "x".to_string(),
                },
                "Warning".to_string(),
                vec![],
                AdviceSeverity::Warning,
            ),
        ];

        let mut sorted = results.clone();
        sorted.sort_by_key(|r| r.severity.sort_key());

        assert_eq!(sorted[0].advice_id, "critical.advice");
        assert_eq!(sorted[1].advice_id, "warning.advice");
        assert_eq!(sorted[2].advice_id, "hint.advice");
    }
}
