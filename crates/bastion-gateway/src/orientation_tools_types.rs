//! Orientation tool parameter types and responses.
//!
//! Defines the parameter and response types for the 7 orientation MCP tools:
//! - `sandbox_orient_me`: Comprehensive environment briefing
//! - `sandbox_suggest_template`: Template recommendation
//! - `sandbox_capacity_check`: Capacity pre-check
//! - `sandbox_optimal_config`: Optimal config for use case
//! - `sandbox_get_config`: Current config (secrets redacted)
//! - `sandbox_set_config`: Update config
//! - `sandbox_config_history`: Config audit trail

use chrono::{DateTime, Utc};
use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ─── Parameter types ─────────────────────────────────────────────────────────

/// Parameters for `sandbox_suggest_template`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TemplateSuggestParams {
    /// Free-text description of the task (e.g. "build a Java Maven project", "run Python tests").
    pub task_description: String,
}

/// Parameters for `sandbox_capacity_check`.
#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct CapacityCheckParams {
    /// Number of sandboxes to check capacity for.
    pub count: u32,
    /// Optional provider name to check (e.g. "podman", "firecracker").
    #[serde(default)]
    pub provider: Option<String>,
}

/// Parameters for `sandbox_optimal_config`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct OptimalConfigParams {
    /// Use case identifier (e.g. "ci_build", "local_dev", "data_processing").
    pub use_case: String,
}

/// Parameters for `sandbox_set_config`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetConfigParams {
    /// JSON object containing key-value pairs to update.
    pub updates: serde_json::Value,
}

// ─── Response types ─────────────────────────────────────────────────────────

/// Response for `sandbox_suggest_template`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSuggestResponse {
    /// The recommended template name.
    pub template: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f64,
    /// An alternative template if the primary is unavailable.
    pub alternative: String,
    /// Human-readable explanation of the recommendation.
    pub reasoning: String,
}

/// Response for `sandbox_capacity_check`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityCheckResponse {
    /// Whether the requested capacity is available.
    pub available: bool,
    /// Current number of active sandboxes.
    pub current_count: u32,
    /// Maximum capacity limit.
    pub max_capacity: u32,
    /// Recommended action if capacity is not available.
    pub recommended_action: String,
}

/// Response for `sandbox_optimal_config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimalConfigResponse {
    /// The optimal configuration as a JSON object.
    pub config: serde_json::Value,
    /// Any warnings about the configuration.
    pub warnings: Vec<String>,
    /// Whether a gateway restart is required for the config to take effect.
    pub restart_required: bool,
    /// Human-readable explanation of the recommendation.
    pub reasoning: String,
}

/// Response for `sandbox_get_config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetConfigResponse {
    /// The current configuration as a JSON object.
    pub config: serde_json::Value,
    /// Notes about the configuration (e.g. which keys are read-only).
    pub notes: Vec<String>,
}

/// Response for `sandbox_set_config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetConfigResponse {
    /// List of config keys that were successfully applied.
    pub applied: Vec<String>,
    /// List of config keys that failed to apply.
    pub failed: Vec<String>,
    /// Whether a gateway restart is required for any of the changes.
    pub requires_restart: bool,
    /// Optional hint about restart requirements.
    pub restart_hint: Option<String>,
}

/// A single configuration change entry in the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeEntry {
    /// When the change was made.
    pub timestamp: DateTime<Utc>,
    /// Dot-notation key that changed (e.g. "pool.max_total").
    pub key: String,
    /// Previous value as string, or null if the key was new.
    pub old_value: Option<String>,
    /// New value as string.
    pub new_value: String,
    /// Who/what made the change (e.g. "sandbox_set_config").
    pub changed_by: String,
}

/// Response for `sandbox_config_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigHistoryResponse {
    /// List of configuration changes in chronological order.
    pub changes: Vec<ConfigChangeEntry>,
}

/// Response for `sandbox_orient_me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrientMeResponse {
    /// Gateway version string.
    pub gateway_version: String,
    /// Primary provider name.
    pub provider: String,
    /// Pool status summary.
    pub pool_status: serde_json::Value,
    /// Available template recommendations.
    pub available_templates: Vec<String>,
    /// Capabilities supported by this gateway.
    pub capabilities: Vec<String>,
    /// Known limitations of this gateway.
    pub known_limitations: Vec<String>,
    /// Whether worker heartbeat monitoring is available.
    pub worker_heartbeat_available: bool,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_suggest_response_serialization() {
        let response = TemplateSuggestResponse {
            template: "node:20-slim".to_string(),
            confidence: 0.95,
            alternative: "ubuntu:24.04".to_string(),
            reasoning: "Matched Node.js ecosystem".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("node:20-slim"));
        assert!(json.contains("0.95"));
    }

    #[test]
    fn test_capacity_check_response_serialization() {
        let response = CapacityCheckResponse {
            available: true,
            current_count: 5,
            max_capacity: 20,
            recommended_action: "proceed".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"available\":true"));
        assert!(json.contains("\"current_count\":5"));
    }

    #[test]
    fn test_config_change_entry_serialization() {
        let entry = ConfigChangeEntry {
            timestamp: Utc::now(),
            key: "pool.max_total".to_string(),
            old_value: Some("10".to_string()),
            new_value: "15".to_string(),
            changed_by: "sandbox_set_config".to_string(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("pool.max_total"));
        assert!(json.contains("15"));
    }

    #[test]
    fn test_orient_me_response_serialization() {
        let response = OrientMeResponse {
            gateway_version: "1.0.0".to_string(),
            provider: "podman".to_string(),
            pool_status: serde_json::json!({"active": 5, "idle": 10}),
            available_templates: vec!["debian:bookworm-slim".to_string()],
            capabilities: vec!["sandbox_create".to_string(), "sandbox_run".to_string()],
            known_limitations: vec![],
            worker_heartbeat_available: false,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("podman"));
    }

    #[test]
    fn test_template_suggest_params_deserialization() {
        let json = r#"{"task_description": "build a Java Maven project"}"#;
        let params: TemplateSuggestParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.task_description, "build a Java Maven project");
    }

    #[test]
    fn test_set_config_params_deserialization() {
        let json = r#"{"updates": {"pool.max_total": 15, "pool.min_idle": 2}}"#;
        let params: SetConfigParams = serde_json::from_str(json).unwrap();
        assert!(params.updates.is_object());
        assert_eq!(params.updates["pool.max_total"], 15);
    }
}
