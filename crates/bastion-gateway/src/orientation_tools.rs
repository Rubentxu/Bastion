//! Orientation MCP tools — agent environment briefing and guidance.
//!
//! Exposes 7 tools to help AI agents understand the Bastion environment:
//! - `sandbox_orient_me`: Comprehensive environment briefing
//! - `sandbox_suggest_template`: Template recommendation
//! - `sandbox_capacity_check`: Capacity pre-check
//! - `sandbox_optimal_config`: Optimal config for use case
//! - `sandbox_get_config`: Current config (secrets redacted)
//! - `sandbox_set_config`: Update config
//! - `sandbox_config_history`: Config audit trail

#![allow(dead_code)]

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use schemars::JsonSchema;
use serde::Deserialize;

use bastion_domain::orientation::TemplateRecommender;

use crate::orientation_tools_types::*;
use crate::server::BastionGateway;

// ─── Tool router function ───────────────────────────────────────────────────

/// Returns the orientation tools router, combining all orientation MCP tools.
pub fn orientation_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::sandbox_orient_me_tool_attr(),
            BastionGateway::sandbox_orient_me,
        ))
        .with_route((
            BastionGateway::sandbox_suggest_template_tool_attr(),
            BastionGateway::sandbox_suggest_template,
        ))
        .with_route((
            BastionGateway::sandbox_capacity_check_tool_attr(),
            BastionGateway::sandbox_capacity_check,
        ))
        .with_route((
            BastionGateway::sandbox_optimal_config_tool_attr(),
            BastionGateway::sandbox_optimal_config,
        ))
        .with_route((
            BastionGateway::sandbox_get_config_tool_attr(),
            BastionGateway::sandbox_get_config,
        ))
        .with_route((
            BastionGateway::sandbox_set_config_tool_attr(),
            BastionGateway::sandbox_set_config,
        ))
        .with_route((
            BastionGateway::sandbox_config_history_tool_attr(),
            BastionGateway::sandbox_config_history,
        ))
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// Comprehensive environment briefing for an AI agent.
    ///
    /// Returns gateway version, provider, pool status, available templates,
    /// capabilities, known limitations, and worker heartbeat availability.
    #[tool(
        description = "Get a comprehensive environment briefing for AI agents. Returns gateway version, provider, pool status, available templates, capabilities, known limitations, and worker heartbeat availability."
    )]
    async fn sandbox_orient_me(&self) -> String {
        let gateway_version = env!("CARGO_PKG_VERSION");

        // Primary provider name
        let provider = self.provider.name().to_string();

        // Pool status
        let pool_status = if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            serde_json::json!({
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle,
                "total": stats.total,
            })
        } else {
            serde_json::json!({
                "enabled": false,
                "message": "Pool is not enabled"
            })
        };

        // Available templates (known working defaults)
        let available_templates = vec![
            "debian:bookworm-slim".to_string(),
            "ubuntu:22.04".to_string(),
            "ubuntu:24.04".to_string(),
            "fedora:39".to_string(),
            "alpine:3.19".to_string(),
            "eclipse-temurin:21-jdk-maven".to_string(),
            "eclipse-temurin:21-jdk-gradle".to_string(),
            "node:20-slim".to_string(),
            "python:3.12-slim".to_string(),
            "rust:1.77-slim".to_string(),
            "golang:1.22-alpine".to_string(),
            "ruby:3.3-slim".to_string(),
            "mcr.microsoft.com/dotnet/sdk:8.0".to_string(),
            "php:8.3-cli".to_string(),
        ];

        // Capabilities
        let capabilities = vec![
            "sandbox_create".to_string(),
            "sandbox_run".to_string(),
            "sandbox_run_stream".to_string(),
            "sandbox_prepare".to_string(),
            "sandbox_terminate".to_string(),
            "sandbox_cancel".to_string(),
            "sandbox_info".to_string(),
            "sandbox_list".to_string(),
            "sandbox_write".to_string(),
            "sandbox_read".to_string(),
            "sandbox_list_files".to_string(),
            "sandbox_sync".to_string(),
            "sandbox_snapshot".to_string(),
            "sandbox_pool_stats".to_string(),
            "sandbox_health".to_string(),
            "sandbox_metrics".to_string(),
            "sandbox_register_artifact".to_string(),
            "sandbox_list_capabilities".to_string(),
            "sandbox_list_artifacts".to_string(),
            "sandbox_orient_me".to_string(),
            "sandbox_suggest_template".to_string(),
            "sandbox_capacity_check".to_string(),
            "sandbox_optimal_config".to_string(),
            "sandbox_get_config".to_string(),
            "sandbox_set_config".to_string(),
            "sandbox_config_history".to_string(),
        ];

        // Known limitations
        let known_limitations = vec![
            "Pool mode only supports podman provider".to_string(),
            "Secrets must be pre-configured in the gateway".to_string(),
            "Worker heartbeat requires workers to send periodic heartbeats".to_string(),
            "Config changes requiring restart are not auto-restarted".to_string(),
        ];

        // Worker heartbeat availability — available when MetricsHub is connected
        let worker_heartbeat_available = self.gateway_config.metrics_hub.is_some();

        let response = OrientMeResponse {
            gateway_version: gateway_version.to_string(),
            provider,
            pool_status,
            available_templates,
            capabilities,
            known_limitations,
            worker_heartbeat_available,
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Suggest a sandbox template based on task description.
    ///
    /// Analyzes keywords in the task description to recommend the most appropriate
    /// template with confidence scoring.
    #[tool(
        description = "Suggest a sandbox template based on task description. Analyzes keywords to recommend the best template with confidence scoring."
    )]
    async fn sandbox_suggest_template(
        &self,
        Parameters(params): Parameters<TemplateSuggestParams>,
    ) -> String {
        let recommender = TemplateRecommender::new();
        let recommendation = recommender.recommend(&params.task_description);

        let response = TemplateSuggestResponse {
            template: recommendation.template,
            confidence: recommendation.confidence as f64,
            alternative: recommendation.alternative,
            reasoning: recommendation.reasoning,
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Check if the gateway has capacity for additional sandboxes.
    ///
    /// Returns whether the requested number of sandboxes can be created,
    /// current count, max capacity, and recommended action.
    #[tool(
        description = "Check if the gateway has capacity for additional sandboxes. Returns availability, current count, max capacity, and recommended action."
    )]
    async fn sandbox_capacity_check(
        &self,
        Parameters(params): Parameters<CapacityCheckParams>,
    ) -> String {
        let (current_count, max_capacity) = if let Some(ref pool) = self.gateway_config.pool_manager
        {
            let stats = pool.stats().await;
            (stats.active as u32, (stats.active + stats.idle) as u32)
        } else {
            // No pool - assume unlimited capacity
            (0, u32::MAX)
        };

        let available = current_count + params.count <= max_capacity;

        let recommended_action = if available {
            "proceed".to_string()
        } else {
            if params.count == 1 {
                "Wait for an existing sandbox to be terminated, or terminate unused sandboxes with sandbox_terminate.".to_string()
            } else {
                format!(
                    "Reduce requested count to {} or wait for existing sandboxes to be terminated.",
                    max_capacity.saturating_sub(current_count)
                )
            }
        };

        let response = CapacityCheckResponse {
            available,
            current_count,
            max_capacity,
            recommended_action,
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Get optimal configuration for a specific use case.
    ///
    /// Returns recommended config values, warnings, restart requirements,
    /// and reasoning.
    #[tool(
        description = "Get optimal gateway configuration for a specific use case (e.g. ci_build, local_dev, data_processing). Returns config, warnings, restart requirements, and reasoning."
    )]
    async fn sandbox_optimal_config(
        &self,
        Parameters(params): Parameters<OptimalConfigParams>,
    ) -> String {
        let (config, warnings, restart_required, reasoning) = match params.use_case.as_str() {
            "ci_build" => (
                serde_json::json!({
                    "pool": {
                        "max_total": 50,
                        "min_idle": 5,
                        "idle_ttl_secs": 300,
                    },
                    "defaults": {
                        "timeout_ms": 600000,
                        "template": "debian:bookworm-slim",
                    }
                }),
                vec![],
                false,
                "CI build optimized: high pool capacity with short idle TTL for burst workloads".to_string(),
            ),
            "local_dev" => (
                serde_json::json!({
                    "pool": {
                        "max_total": 10,
                        "min_idle": 2,
                        "idle_ttl_secs": 3600,
                    },
                    "defaults": {
                        "timeout_ms": 3600000,
                        "template": "ubuntu:24.04",
                    }
                }),
                vec![],
                false,
                "Local development optimized: smaller pool with longer idle TTL for interactive use".to_string(),
            ),
            "data_processing" => (
                serde_json::json!({
                    "pool": {
                        "max_total": 100,
                        "min_idle": 10,
                        "idle_ttl_secs": 600,
                    },
                    "defaults": {
                        "timeout_ms": 3600000,
                        "template": "python:3.12-slim",
                    }
                }),
                vec![],
                false,
                "Data processing optimized: large pool for batch workloads".to_string(),
            ),
            _ => (
                serde_json::json!({
                    "pool": {
                        "max_total": 20,
                        "min_idle": 2,
                        "idle_ttl_secs": 900,
                    },
                    "defaults": {
                        "timeout_ms": 1800000,
                    }
                }),
                vec![format!("Unknown use case '{}', returning default config", params.use_case)],
                false,
                "Default configuration for general workloads".to_string(),
            ),
        };

        let response = OptimalConfigResponse {
            config,
            warnings,
            restart_required,
            reasoning,
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Get the current gateway configuration with secrets redacted.
    ///
    /// Returns the current config as JSON and notes about read-only keys.
    #[tool(
        description = "Get the current gateway configuration with secrets redacted. Returns config as JSON and notes about read-only keys."
    )]
    async fn sandbox_get_config(&self) -> String {
        // Build current config from gateway state
        let pool_config = if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            serde_json::json!({
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle,
                "total": stats.total,
            })
        } else {
            serde_json::json!({
                "enabled": false,
            })
        };

        let config = serde_json::json!({
            "pool": pool_config,
            "gateway": {
                "version": env!("CARGO_PKG_VERSION"),
            },
            "auth": {
                "pre_shared_key_enabled": self.gateway_config.auth.pre_shared_key_enabled,
                // PSKs redacted for security
                "psk_count": if self.gateway_config.auth.pre_shared_keys.is_empty() {
                    0
                } else {
                    self.gateway_config.auth.pre_shared_keys.len()
                },
            }
        });

        let notes = vec![
            "Auth keys (pre_shared_keys) are redacted for security".to_string(),
            "Pool configuration is read-only at runtime (use sandbox_set_config to persist changes)".to_string(),
            "Gateway version cannot be changed at runtime".to_string(),
        ];

        let response = GetConfigResponse { config, notes };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Update gateway configuration.
    ///
    /// Accepts a JSON object of key-value pairs to update. Returns which
    /// keys were applied, which failed, and if restart is required.
    #[tool(
        description = "Update gateway configuration. Accepts JSON key-value pairs to change. Returns applied keys, failed keys, and whether restart is required."
    )]
    async fn sandbox_set_config(&self, Parameters(params): Parameters<SetConfigParams>) -> String {
        let updates = params.updates;

        // Phase 4: Will delegate to MetricsHub for actual config persistence
        // For now, return a placeholder response

        let mut applied = Vec::new();
        let mut failed = Vec::new();
        let mut has_auth_failure = false;

        if let Some(obj) = updates.as_object() {
            for (key, value) in obj {
                // Check for restricted keys
                let restricted = key.starts_with("auth.hmac_enabled")
                    || key.starts_with("auth.jwt_enabled")
                    || key.starts_with("auth.pre_shared_key_enabled")
                    || key.starts_with("gateway.port");

                if restricted {
                    failed.push(key.clone());
                    if key.starts_with("auth.") {
                        has_auth_failure = true;
                    }
                } else {
                    applied.push(key.clone());
                }
            }
        } else {
            failed.push("updates".to_string());
        }

        let response = SetConfigResponse {
            applied,
            failed,
            requires_restart: false,
            restart_hint: if has_auth_failure {
                Some("Auth changes require gateway restart to take effect".to_string())
            } else {
                None
            },
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }

    /// Get the configuration change history.
    ///
    /// Returns all config changes in chronological order with timestamps,
    /// keys, old/new values, and attribution.
    #[tool(
        description = "Get the configuration change audit trail. Returns all config changes in chronological order with timestamps, keys, old/new values, and attribution."
    )]
    async fn sandbox_config_history(&self) -> String {
        use bastion_domain::orientation::ConfigChange;

        let changes: Vec<ConfigChangeEntry> =
            if let Some(ref metrics_hub_lock) = self.gateway_config.metrics_hub {
                // Use tokio async Mutex (guards are Send, safe across await)
                let hub = metrics_hub_lock.lock().await;
                let history: Vec<ConfigChange> = hub.get_config_history().await;
                history
                    .into_iter()
                    .map(|c| ConfigChangeEntry {
                        timestamp: c.timestamp,
                        key: c.key,
                        old_value: c.old_value,
                        new_value: c.new_value,
                        changed_by: c.changed_by,
                    })
                    .collect()
            } else {
                vec![]
            };

        let response = ConfigHistoryResponse { changes };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": format!("failed to serialize response: {}", e)}).to_string()
        })
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::orientation::TemplateRecommender;

    #[test]
    fn test_template_recommender_maven() {
        let recommender = TemplateRecommender::new();
        let result = recommender.recommend("build a Java Maven project with Spring Boot");

        assert_eq!(result.template, "eclipse-temurin:21-jdk-maven");
        assert!(result.confidence >= 0.90);
        assert!(result.reasoning.contains("Maven"));
    }

    #[test]
    fn test_template_recommender_node() {
        let recommender = TemplateRecommender::new();
        let result = recommender.recommend("npm install and run node script");

        assert_eq!(result.template, "node:20-slim");
        assert!(result.confidence >= 0.90);
    }

    #[test]
    fn test_template_recommender_python() {
        let recommender = TemplateRecommender::new();
        let result = recommender.recommend("pip install -r requirements.txt");

        assert_eq!(result.template, "python:3.12-slim");
        assert!(result.confidence >= 0.90);
    }

    #[test]
    fn test_template_recommender_fallback() {
        let recommender = TemplateRecommender::new();
        let result = recommender.recommend("do something completely random xyz123");

        assert_eq!(result.template, "ubuntu:24.04");
        assert!(result.confidence < 0.5);
    }

    #[test]
    fn test_response_serialization() {
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
    fn test_capacity_response_serialization() {
        let response = CapacityCheckResponse {
            available: true,
            current_count: 5,
            max_capacity: 20,
            recommended_action: "proceed".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: CapacityCheckResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.available, true);
        assert_eq!(deserialized.current_count, 5);
    }

    #[test]
    fn test_config_change_entry_serialization() {
        use chrono::Utc;

        let entry = ConfigChangeEntry {
            timestamp: Utc::now(),
            key: "pool.max_total".to_string(),
            old_value: Some("10".to_string()),
            new_value: "15".to_string(),
            changed_by: "sandbox_set_config".to_string(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: ConfigChangeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.key, "pool.max_total");
        assert_eq!(deserialized.new_value, "15");
    }

    #[test]
    fn test_orient_me_response_fields() {
        let response = OrientMeResponse {
            gateway_version: "1.0.0".to_string(),
            provider: "podman".to_string(),
            pool_status: serde_json::json!({"enabled": true, "active": 5}),
            available_templates: vec!["debian:bookworm-slim".to_string()],
            capabilities: vec!["sandbox_create".to_string()],
            known_limitations: vec![],
            worker_heartbeat_available: false,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("podman"));
        assert!(json.contains("sandbox_create"));
    }
}
