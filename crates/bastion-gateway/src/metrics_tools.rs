//! Metrics MCP tools — historical metrics and resource usage.
//!
//! Exposes 2 tools for querying metrics and resource usage:
//! - `sandbox_metrics_history`: Historical metrics since a timestamp
//! - `sandbox_resource_usage`: Per-sandbox resource usage from heartbeat bridge

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;

use bastion_infrastructure::metrics::MetricRecord;

use crate::metrics_tools_types::*;
use crate::server::BastionGateway;

// ─── Tool router function ───────────────────────────────────────────────────

/// Returns the metrics tools router, combining all metrics MCP tools.
pub fn metrics_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::sandbox_metrics_history_tool_attr(),
            BastionGateway::sandbox_metrics_history,
        ))
        .with_route((
            BastionGateway::sandbox_resource_usage_tool_attr(),
            BastionGateway::sandbox_resource_usage,
        ))
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// Get historical metrics since a given timestamp.
    ///
    /// Returns metric records including CPU, memory, disk usage, and
    /// command counts. Optionally filter by sandbox ID.
    #[tool(
        description = "Get historical metrics since a given timestamp. Returns CPU, memory, disk usage, and command counts. Optionally filter by sandbox_id."
    )]
    async fn sandbox_metrics_history(
        &self,
        Parameters(params): Parameters<MetricsHistoryParams>,
    ) -> String {
        // Parse the since timestamp
        let since: DateTime<Utc> = match params.since.parse() {
            Ok(dt) => dt,
            Err(e) => {
                return serde_json::json!({
                    "error": format!("invalid timestamp format '{}': {}", params.since, e)
                })
                .to_string();
            }
        };

        // Query MetricsHub if available — use tokio async lock (Send-safe guards)
        let records: Vec<MetricRecordResponse> =
            if let Some(ref metrics_hub_lock) = self.gateway_config.metrics_hub {
                let hub = metrics_hub_lock.lock().await;
                match hub.get_metrics_history(since).await {
                    Ok(history) => history
                        .into_iter()
                        .map(|r: MetricRecord| MetricRecordResponse {
                            timestamp: r.timestamp.to_rfc3339(),
                            sandbox_id: r.sandbox_id,
                            cpu_percent: r.cpu_percent,
                            mem_used_mb: r.mem_used_mb,
                            mem_limit_mb: r.mem_limit_mb,
                            disk_used_mb: r.disk_used_mb,
                            commands_executed: r.commands_executed,
                            errors_total: r.errors_total,
                        })
                        .collect(),
                    Err(e) => {
                        return serde_json::json!({
                            "error": format!("failed to get metrics history: {}", e)
                        })
                        .to_string();
                    }
                }
            } else {
                vec![]
            };

        let response = MetricsHistoryResponse {
            count: records.len(),
            records,
            since: params.since,
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Get per-sandbox resource usage from worker heartbeats.
    ///
    /// Returns CPU, memory, disk usage, load average, uptime, and
    /// last heartbeat timestamp for a specific sandbox.
    #[tool(
        description = "Get per-sandbox resource usage (CPU, memory, disk, load, uptime) from worker heartbeats for a specific sandbox_id."
    )]
    async fn sandbox_resource_usage(
        &self,
        Parameters(params): Parameters<ResourceUsageParams>,
    ) -> String {
        // Query HeartbeatBridge via MetricsHub if available — use tokio async Mutex (Send-safe)
        let response = if let Some(ref metrics_hub_lock) = self.gateway_config.metrics_hub {
            let hub = metrics_hub_lock.lock().await;
            match hub.get_sandbox_resources(&params.sandbox_id).await {
                Ok(Some(resources)) => ResourceUsageResponse {
                    sandbox_id: params.sandbox_id.clone(),
                    cpu_percent: resources.cpu_percent,
                    mem_used_mb: resources.mem_used_mb,
                    mem_limit_mb: resources.mem_limit_mb,
                    disk_used_mb: resources.disk_used_mb,
                    loadavg_1m: resources.loadavg_1m,
                    uptime_seconds: resources.uptime_seconds,
                    last_heartbeat: resources.last_heartbeat.to_rfc3339(),
                    active: true,
                },
                Ok(None) => ResourceUsageResponse {
                    sandbox_id: params.sandbox_id.clone(),
                    cpu_percent: 0.0,
                    mem_used_mb: 0.0,
                    mem_limit_mb: 0.0,
                    disk_used_mb: 0.0,
                    loadavg_1m: 0.0,
                    uptime_seconds: 0,
                    last_heartbeat: String::new(),
                    active: false,
                },
                Err(e) => {
                    return serde_json::json!({
                        "error": format!("failed to get resource usage: {}", e)
                    })
                    .to_string();
                }
            }
        } else {
            ResourceUsageResponse {
                sandbox_id: params.sandbox_id.clone(),
                cpu_percent: 0.0,
                mem_used_mb: 0.0,
                mem_limit_mb: 0.0,
                disk_used_mb: 0.0,
                loadavg_1m: 0.0,
                uptime_seconds: 0,
                last_heartbeat: String::new(),
                active: false,
            }
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        let deserialized: MetricsHistoryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.count, 1);
        assert_eq!(deserialized.records[0].sandbox_id.as_deref(), Some("sb-1"));
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
        let deserialized: ResourceUsageResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.sandbox_id, "test-sandbox");
        assert!((deserialized.cpu_percent - 42.5).abs() < 0.01);
        assert!(deserialized.active);
    }

    #[test]
    fn test_timestamp_parsing() {
        let valid_timestamp = "2024-01-01T00:00:00Z";
        let dt: Result<DateTime<Utc>, _> = valid_timestamp.parse();
        assert!(dt.is_ok());

        let invalid_timestamp = "not-a-timestamp";
        let dt: Result<DateTime<Utc>, _> = invalid_timestamp.parse();
        assert!(dt.is_err());
    }

    #[test]
    fn test_metrics_history_response_empty() {
        let response = MetricsHistoryResponse {
            records: vec![],
            count: 0,
            since: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"count\":0"));
        assert!(json.contains("\"records\":[]"));
    }
}
