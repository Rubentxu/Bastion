//! Pipeline and dashboard state handlers.

use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use tracing::debug;

use crate::error::DashboardError;
use crate::project::config::DashboardState;
use crate::server::state::DashboardServerState;

/// List pipelines for a project.
///
/// GET /api/v1/projects/:id/pipelines
pub async fn list_pipelines(
    State(state): State<DashboardServerState>,
    Path(project_id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), DashboardError> {
    debug!("Listing pipelines for project: {}", project_id);

    // For now, we need the project path to load pipelines
    // The project_id is actually the path in our implementation
    let project_path = std::path::Path::new(&project_id);

    let pipelines = state.project_manager.list_pipelines(project_path).await
        .map_err(|e| {
            tracing::warn!("Failed to list pipelines: {}", e);
            DashboardError::InternalError(format!("Failed to list pipelines: {}", e))
        })?;

    let total = pipelines.len();
    Ok((StatusCode::OK, Json(serde_json::json!({
        "pipelines": pipelines,
        "total": total
    }))))
}

/// Get dashboard state.
///
/// GET /api/v1/dashboard/state
pub async fn get_dashboard_state(
    State(state): State<DashboardServerState>,
) -> impl IntoResponse {
    debug!("Getting dashboard state");

    // For now, use a default project path (current directory)
    // In a real implementation, this would be tied to the authenticated user or a specific project
    let project_path = std::path::Path::new(".");

    match state.project_manager.load_dashboard_state(project_path).await {
        Ok(state) => (StatusCode::OK, Json(state)),
        Err(e) => {
            tracing::warn!("Failed to load dashboard state: {}", e);
            // Return default state if loading fails
            (StatusCode::OK, Json(DashboardState::default()))
        }
    }
}

/// Update dashboard state.
///
/// PUT /api/v1/dashboard/state
pub async fn put_dashboard_state(
    State(state): State<DashboardServerState>,
    Json(new_state): Json<DashboardState>,
) -> Result<(StatusCode, Json<DashboardState>), DashboardError> {
    debug!("Saving dashboard state");

    // For now, use a default project path (current directory)
    let project_path = std::path::Path::new(".");

    state.project_manager.save_dashboard_state(project_path, &new_state).await
        .map_err(|e| {
            tracing::warn!("Failed to save dashboard state: {}", e);
            DashboardError::InternalError(format!("Failed to save dashboard state: {}", e))
        })?;

    Ok((StatusCode::OK, Json(new_state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_state_default() {
        let state = DashboardState::default();
        assert!(state.selected_project.is_none());
        assert!(state.collapsed_sections.is_empty());
        assert!(state.column_order.is_empty());
        // Note: derived Default gives empty string, not "system"
        // The "system" default is only applied during JSON deserialization
        assert_eq!(state.theme, "");
    }

    #[test]
    fn test_dashboard_state_with_theme() {
        let json = r#"{"theme":"dark"}"#;
        let state: DashboardState = serde_json::from_str(json).unwrap();
        assert_eq!(state.theme, "dark");
    }
}
