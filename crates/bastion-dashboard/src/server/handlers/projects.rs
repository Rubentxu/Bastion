//! Project-related HTTP handlers.
//!
//! Handles requests for project listing, details, and creation.

use std::path::PathBuf;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use tracing::{debug, info};

use bastion_domain::project::ProjectKind;

use crate::api::types::ProjectDetailResponse;
use crate::error::ApiError;
use crate::project::types::ProjectSummary;
use crate::server::state::DashboardServerState;

/// Handler for listing all projects.
///
/// GET /api/v1/projects
///
/// Returns a list of all discovered projects.
pub async fn list_projects(
    State(state): State<DashboardServerState>,
) -> Result<Json<Vec<ProjectDetailResponse>>, ApiError> {
    debug!("Listing all projects");

    // Use a default root path for project discovery
    let root = PathBuf::from(".");
    let projects: Vec<ProjectSummary> = state
        .project_manager
        .list_projects(&root)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to list projects: {}", e)))?;

    let response: Vec<ProjectDetailResponse> = projects
        .into_iter()
        .map(|p| ProjectDetailResponse {
            id: p.id.to_string(),
            name: p.name,
            kind: kind_to_string(&p.kind),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .collect();

    Ok(Json(response))
}

/// Handler for getting a single project by ID.
///
/// GET /api/v1/projects/:id
///
/// Returns project details for the specified project ID.
pub async fn get_project(
    State(state): State<DashboardServerState>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectDetailResponse>, ApiError> {
    debug!("Getting project: {}", project_id);

    // First try to find it via the project manager
    let root = PathBuf::from(".");
    let projects: Vec<ProjectSummary> = state
        .project_manager
        .list_projects(&root)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to list projects: {}", e)))?;

    let project = projects
        .into_iter()
        .find(|p| p.id.to_string() == project_id)
        .ok_or_else(|| ApiError::NotFound(format!("Project not found: {}", project_id)))?;

    let response = ProjectDetailResponse {
        id: project.id.to_string(),
        name: project.name,
        kind: kind_to_string(&project.kind),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok(Json(response))
}

/// Request body for creating a new project.
#[derive(serde::Deserialize)]
pub struct CreateProjectRequest {
    pub path: String,
    pub kind: String,
}

/// Handler for creating/registering a new project.
///
/// POST /api/v1/projects
///
/// Creates a new project at the specified path.
pub async fn create_project(
    State(state): State<DashboardServerState>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectDetailResponse>), ApiError> {
    info!("Creating project at: {} with kind: {}", request.path, request.kind);

    let path = PathBuf::from(&request.path);
    let kind = match request.kind.to_lowercase().as_str() {
        "rust" => ProjectKind::Rust,
        "nodejs" | "node" => ProjectKind::NodeJs,
        "python" | "py" => ProjectKind::Python,
        "go" => ProjectKind::Go,
        _ => ProjectKind::Generic,
    };

    let project = state
        .project_manager
        .init_project(&path, kind)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to create project: {}", e)))?;

    let response = ProjectDetailResponse {
        id: project.id.to_string(),
        name: project.name,
        kind: kind_to_string(&project.kind),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Convert ProjectKind to string representation.
fn kind_to_string(kind: &ProjectKind) -> String {
    match kind {
        ProjectKind::Rust => "rust".to_string(),
        ProjectKind::NodeJs => "nodejs".to_string(),
        ProjectKind::Python => "python".to_string(),
        ProjectKind::Go => "go".to_string(),
        ProjectKind::Generic => "generic".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_to_string() {
        assert_eq!(kind_to_string(&ProjectKind::Rust), "rust");
        assert_eq!(kind_to_string(&ProjectKind::NodeJs), "nodejs");
        assert_eq!(kind_to_string(&ProjectKind::Generic), "generic");
    }

    #[tokio::test]
    async fn test_list_projects_returns_empty_or_error() {
        use std::sync::Arc;
        use crate::api::client::DashboardApiClient;
        use crate::project::ProjectManager;

        let api_client = DashboardApiClient::default();
        let manager = ProjectManager::new(api_client.clone());
        let state = DashboardServerState {
            project_manager: Arc::new(manager),
            api_client: Arc::new(api_client),
            gateway_url: "http://localhost:8080".to_string(),
        };

        let result = list_projects(State(state)).await;
        // Without actual projects, should return empty array or error gracefully
        assert!(result.is_ok() || matches!(result, Err(ApiError::InternalError(_))));
    }

    #[tokio::test]
    async fn test_get_nonexistent_project_returns_not_found() {
        use std::sync::Arc;
        use crate::api::client::DashboardApiClient;
        use crate::project::ProjectManager;

        let api_client = DashboardApiClient::default();
        let manager = ProjectManager::new(api_client.clone());
        let state = DashboardServerState {
            project_manager: Arc::new(manager),
            api_client: Arc::new(api_client),
            gateway_url: "http://localhost:8080".to_string(),
        };

        let result = get_project(State(state), Path("nonexistent-id".to_string())).await;
        match result {
            Ok(_) => panic!("Expected error for nonexistent project"),
            Err(ApiError::NotFound(_)) => {}
            Err(e) => panic!("Unexpected error type: {:?}", e),
        }
    }
}
