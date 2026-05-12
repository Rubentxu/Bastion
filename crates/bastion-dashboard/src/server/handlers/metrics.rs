//! Metrics-related HTTP handlers.
//!
//! Handles requests for sandbox and project metrics.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tracing::debug;

use crate::error::ApiError;
use crate::server::state::DashboardServerState;

/// Handler for getting metrics for all sandboxes.
///
/// GET /api/v1/metrics
///
/// Returns aggregated metrics across all sandboxes.
pub async fn get_global_metrics(
    State(state): State<DashboardServerState>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Getting global metrics");

    // Get all sandboxes first
    let sandboxes = state
        .api_client
        .list_sandboxes()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to list sandboxes: {}", e)))?;

    let mut all_metrics = Vec::new();

    // Fetch metrics for each sandbox
    for sandbox in sandboxes {
        match state.api_client.get_metrics(&sandbox.id).await {
            Ok(metrics) => all_metrics.push(metrics),
            Err(e) => {
                debug!("Failed to get metrics for sandbox {}: {}", sandbox.id, e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "metrics": all_metrics,
        "total": all_metrics.len()
    })))
}

/// Handler for getting metrics for a specific project.
///
/// GET /api/v1/projects/:id/metrics
///
/// Returns metrics for all sandboxes associated with the specified project.
pub async fn get_project_metrics(
    State(state): State<DashboardServerState>,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Getting metrics for project: {}", project_id);

    // Get sandboxes for the project
    let project_sandboxes = state
        .project_manager
        .get_project_sandboxes(&bastion_domain::project::ProjectId::new(&project_id))
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to get project sandboxes: {}", e)))?;

    let mut all_metrics = Vec::new();

    // Fetch metrics for each sandbox
    for sandbox in project_sandboxes {
        match state.api_client.get_metrics(&sandbox.id).await {
            Ok(metrics) => all_metrics.push(metrics),
            Err(e) => {
                debug!("Failed to get metrics for sandbox {}: {}", sandbox.id, e);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "project_id": project_id,
        "metrics": all_metrics,
        "total": all_metrics.len()
    })))
}

#[cfg(test)]
mod tests {
    use crate::api::types::MetricsResponse;

    #[test]
    fn test_metrics_response_structure() {
        let json = r#"{
            "sandbox_id": "sb-123",
            "cpu_usage_percent": 45.5,
            "memory_usage_mb": 256,
            "disk_usage_mb": 1024,
            "network_rx_bytes": 1024,
            "network_tx_bytes": 512,
            "timestamp": "2024-01-01T00:00:00Z"
        }"#;

        let metrics: MetricsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(metrics.sandbox_id, "sb-123");
        assert!((metrics.cpu_usage_percent - 45.5).abs() < 0.01);
        assert_eq!(metrics.memory_usage_mb, 256);
    }
}
