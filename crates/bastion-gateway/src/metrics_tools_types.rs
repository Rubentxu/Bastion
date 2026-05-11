//! Metrics tool parameter types and responses.
//!
//! Defines the parameter and response types for the 2 metrics MCP tools:
//! - `sandbox_metrics_history`: Historical metrics since a timestamp
//! - `sandbox_resource_usage`: Per-sandbox resource usage from heartbeat bridge

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ─── Parameter types ─────────────────────────────────────────────────────────

/// Parameters for `sandbox_metrics_history`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MetricsHistoryParams {
    /// ISO 8601 timestamp to get metrics since (e.g. "2024-01-01T00:00:00Z").
    pub since: String,
    /// Optional sandbox ID to filter metrics for a specific sandbox.
    #[serde(default)]
    pub sandbox_id: Option<String>,
}

/// Parameters for `sandbox_resource_usage`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResourceUsageParams {
    /// Sandbox ID to get resource usage for.
    pub sandbox_id: String,
}

// ─── Response types ─────────────────────────────────────────────────────────

/// A single historical metric record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRecordResponse {
    /// Timestamp of the metric.
    pub timestamp: String,
    /// Optional sandbox ID.
    pub sandbox_id: Option<String>,
    /// CPU usage percentage.
    pub cpu_percent: Option<f64>,
    /// Memory used in MB.
    pub mem_used_mb: Option<f64>,
    /// Memory limit in MB.
    pub mem_limit_mb: Option<f64>,
    /// Disk used in MB.
    pub disk_used_mb: Option<f64>,
    /// Commands executed count.
    pub commands_executed: Option<u64>,
    /// Errors total count.
    pub errors_total: Option<u64>,
}

/// Response for `sandbox_metrics_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsHistoryResponse {
    /// List of metric records.
    pub records: Vec<MetricRecordResponse>,
    /// Number of records returned.
    pub count: usize,
    /// Filter timestamp used.
    pub since: String,
}

/// Response for `sandbox_resource_usage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsageResponse {
    /// Sandbox ID.
    pub sandbox_id: String,
    /// CPU usage as percentage (0-100).
    pub cpu_percent: f64,
    /// Memory used in MB.
    pub mem_used_mb: f64,
    /// Memory limit in MB.
    pub mem_limit_mb: f64,
    /// Disk used in MB.
    pub disk_used_mb: f64,
    /// 1-minute load average.
    pub loadavg_1m: f64,
    /// Uptime in seconds.
    pub uptime_seconds: u64,
    /// Last heartbeat timestamp.
    pub last_heartbeat: String,
    /// Whether the sandbox is actively reporting.
    pub active: bool,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_record_response_serialization() {
        let record = MetricRecordResponse {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            sandbox_id: Some("test-sandbox".to_string()),
            cpu_percent: Some(42.5),
            mem_used_mb: Some(256.0),
            mem_limit_mb: Some(512.0),
            disk_used_mb: Some(100.0),
            commands_executed: Some(10),
            errors_total: Some(0),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("test-sandbox"));
        assert!(json.contains("42.5"));
    }

    #[test]
    fn test_metrics_history_response_serialization() {
        let response = MetricsHistoryResponse {
            records: vec![MetricRecordResponse {
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                sandbox_id: Some("sb-1".to_string()),
                cpu_percent: Some(25.0),
                mem_used_mb: Some(128.0),
                mem_limit_mb: Some(512.0),
                disk_used_mb: Some(50.0),
                commands_executed: Some(5),
                errors_total: Some(0),
            }],
            count: 1,
            since: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"count\":1"));
        assert!(json.contains("sb-1"));
    }

    #[test]
    fn test_resource_usage_response_serialization() {
        let response = ResourceUsageResponse {
            sandbox_id: "test-sandbox".to_string(),
            cpu_percent: 42.5,
            mem_used_mb: 256.0,
            mem_limit_mb: 512.0,
            disk_used_mb: 100.0,
            loadavg_1m: 0.75,
            uptime_seconds: 3600,
            last_heartbeat: "2024-01-01T01:00:00Z".to_string(),
            active: true,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test-sandbox"));
        assert!(json.contains("42.5"));
        assert!(json.contains("\"active\":true"));
    }

    #[test]
    fn test_metrics_history_params_deserialization() {
        let json = r#"{"since": "2024-01-01T00:00:00Z", "sandbox_id": "test-sb"}"#;
        let params: MetricsHistoryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.since, "2024-01-01T00:00:00Z");
        assert_eq!(params.sandbox_id, Some("test-sb".to_string()));
    }

    #[test]
    fn test_resource_usage_params_deserialization() {
        let json = r#"{"sandbox_id": "my-sandbox"}"#;
        let params: ResourceUsageParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.sandbox_id, "my-sandbox");
    }
}
