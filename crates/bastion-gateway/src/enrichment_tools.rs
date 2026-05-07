//! Enrichment MCP tools: optimizer report, retention info, retention cleanup, and health.
//!
//! Exposes four MCP tools for managing and inspecting the enrichment run recorder:
//! - `enrichment_optimizer_report`: Generate an optimizer report from recorded runs
//! - `enrichment_retention_info`: Get current retention config and DB stats
//! - `enrichment_retention_cleanup`: Run retention cleanup and get deleted/remaining counts
//! - `enrichment_health`: Get operational health status of the enrichment adapter

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{schemars, tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::BastionGateway;

// ─── Tool parameter types ────────────────────────────────────────────────────

/// Optional timestamp filter for optimizer report (ISO 8601).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EnrichmentOptimizerReportParams {
    /// Only include runs after this timestamp (ISO 8601 format).
    /// Example: "2025-02-01T00:00:00Z"
    pub after: Option<String>,
}

// ─── Tool router function ───────────────────────────────────────────────────

/// Returns the enrichment tools router, combining all enrichment MCP tools.
pub fn enrichment_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((BastionGateway::enrichment_optimizer_report_tool_attr(), BastionGateway::enrichment_optimizer_report))
        .with_route((BastionGateway::enrichment_retention_info_tool_attr(), BastionGateway::enrichment_retention_info))
        .with_route((BastionGateway::enrichment_retention_cleanup_tool_attr(), BastionGateway::enrichment_retention_cleanup))
        .with_route((BastionGateway::enrichment_health_tool_attr(), BastionGateway::enrichment_health))
}

// ─── Tool implementations ────────────────────────────────────────────────────

impl BastionGateway {
    /// Generate an optimizer report from recorded enrichment runs.
    ///
    /// Returns a JSON object with:
    /// - `generated_at`: ISO 8601 timestamp of report generation
    /// - `total_runs_analyzed`: Total number of runs included
    /// - `scores`: Per-enricher scores including utility_score, artifact_yield, etc.
    /// - `recommendations`: Per-enricher optimization recommendations
    ///
    /// When recorder is not configured, returns an empty report with `total_runs_analyzed: 0`.
    #[tool(description = "Generate an optimizer report from recorded enrichment runs")]
    async fn enrichment_optimizer_report(
        &self,
        Parameters(params): Parameters<EnrichmentOptimizerReportParams>,
    ) -> String {
        // Get the enrichment adapter
        let adapter = match &*self.enrichment_adapter {
            Some(a) => a,
            None => {
                return serde_json::json!({
                    "error": "enrichment recorder not configured"
                })
                .to_string();
            }
        };

        // Get the optimizer repository from the adapter
        let optimizer_repo: &Arc<dyn enrichment_engine::optimizer::OptimizerRepository> = match adapter.optimizer_repo() {
            Some(repo) => repo,
            None => {
                return serde_json::json!({
                    "error": "enrichment optimizer repository not configured"
                })
                .to_string();
            }
        };

        // Read records and generate report
        let records_result: Result<Vec<enrichment_engine::models::EnrichmentRunRecord>, enrichment_engine::traits::EnrichmentError> = optimizer_repo.read_records(params.after.as_deref()).await;
        match records_result {
            Ok(records) => {
                use enrichment_engine::optimizer::generate_report;

                let report = generate_report(&records);
                serde_json::json!({
                    "generated_at": report.generated_at,
                    "total_runs_analyzed": report.total_runs_analyzed,
                    "scores": report.scores,
                    "recommendations": report.recommendations
                })
                .to_string()
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read optimizer records");
                serde_json::json!({
                    "error": format!("Failed to generate optimizer report: {}", e)
                })
                .to_string()
            }
        }
    }

    /// Get current retention configuration and database statistics.
    ///
    /// Returns a JSON object with:
    /// - `retention`: The retention config (max_age_days, max_rows, enabled, sanitize)
    /// - `stats`: Database stats (current_row_count, oldest_record_ts, newest_record_ts)
    ///
    /// When recorder is not configured, returns an error.
    #[tool(description = "Get current retention configuration and database statistics")]
    async fn enrichment_retention_info(&self) -> String {
        // Get the enrichment adapter
        let adapter = match &*self.enrichment_adapter {
            Some(a) => a,
            None => {
                return serde_json::json!({
                    "error": "enrichment recorder not configured"
                })
                .to_string();
            }
        };

        // Get retention info from the adapter (async to fetch stats from DB)
        let retention_stats = match adapter.retention_info().await {
            Some(stats) => stats,
            None => {
                return serde_json::json!({
                    "error": "enrichment recorder not configured"
                })
                .to_string();
            }
        };

        serde_json::json!({
            "retention": {
                "max_age_days": retention_stats.max_age_days,
                "max_rows": retention_stats.max_rows,
                "enabled": retention_stats.enabled,
                "sanitize": retention_stats.sanitize
            },
            "stats": {
                "current_row_count": retention_stats.current_row_count,
                "oldest_record_ts": retention_stats.oldest_record_ts,
                "newest_record_ts": retention_stats.newest_record_ts
            }
        })
        .to_string()
    }

    /// Run retention cleanup and return deleted/remaining row counts.
    ///
    /// This is idempotent — safe to call repeatedly.
    /// Respects the `RetentionConfig::enabled` flag (returns 0 if disabled).
    ///
    /// Returns a JSON object with:
    /// - `deleted_rows`: Number of rows deleted by this cleanup
    /// - `remaining_rows`: Number of rows remaining after cleanup
    #[tool(description = "Run retention cleanup and return deleted/remaining row counts")]
    async fn enrichment_retention_cleanup(&self) -> String {
        // Get the enrichment adapter
        let adapter = match &*self.enrichment_adapter {
            Some(a) => a,
            None => {
                return serde_json::json!({
                    "error": "enrichment recorder not configured"
                })
                .to_string();
            }
        };

        // Get the recorder from the adapter
        let recorder = match adapter.recorder() {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "enrichment recorder not configured"
                })
                .to_string();
            }
        };

        // Run cleanup
        match recorder.cleanup().await {
            Ok(deleted) => {
                // Get remaining count by querying stats after cleanup
                let remaining_rows: Option<u64> = match recorder.stats().await {
                    Ok(stats) => Some(stats.current_row_count),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to get remaining rows after cleanup");
                        None
                    }
                };
                serde_json::json!({
                    "deleted_rows": deleted,
                    "remaining_rows": remaining_rows
                })
                .to_string()
            }
            Err(e) => {
                tracing::warn!(error = %e, "Retention cleanup failed");
                serde_json::json!({
                    "error": format!("Retention cleanup failed: {}", e)
                })
                .to_string()
            }
        }
    }

    /// Get operational health status of the enrichment adapter.
    ///
    /// Returns a JSON object with:
    /// - `enabled`: Whether enrichment is enabled
    /// - `catalog_enricher_count`: Number of enrichers in the catalog
    /// - `recent_runs_5min`: Total runs (success + failure) completed in the last 5 minutes
    /// - `saturation_events`: Number of saturation drop events
    /// - `db_row_count`: Current row count in database (if recorder available)
    /// - `recorder_available`: Whether a recorder is configured
    #[tool(description = "Get operational health status of the enrichment adapter")]
    async fn enrichment_health(&self) -> String {
        // Get the enrichment adapter
        let adapter = match &*self.enrichment_adapter {
            Some(a) => a,
            None => {
                return serde_json::json!({
                    "error": "enrichment adapter not configured"
                })
                .to_string();
            }
        };

        // Get health snapshot from adapter
        let health = adapter.health().await;

        serde_json::json!({
            "enabled": health.enabled,
            "catalog_enricher_count": health.catalog_enricher_count,
            "recent_runs_5min": health.recent_runs_5min,
            "saturation_events": health.saturation_events,
            "db_row_count": health.db_row_count,
            "recorder_available": health.recorder_available
        })
        .to_string()
    }
}
