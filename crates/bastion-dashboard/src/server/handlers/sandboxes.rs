//! Sandbox-related HTTP handlers.
//!
//! Handles requests for sandbox listing, creation, and termination.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::{debug, info};

use bastion_domain::project::SandboxPurpose;

use crate::api::types::{SandboxDetailResponse, SandboxInfo};
use crate::error::ApiError;
use crate::project::ProjectSandbox;
use crate::server::state::DashboardServerState;

/// Handler for listing sandboxes in a specific project.
///
/// GET /api/v1/projects/:id/sandboxes
///
/// Returns all sandboxes associated with the specified project.
pub async fn list_project_sandboxes(
    State(state): State<DashboardServerState>,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Listing sandboxes for project: {}", project_id);

    let sandboxes: Vec<ProjectSandbox> = state
        .project_manager
        .get_project_sandboxes(&bastion_domain::project::ProjectId::new(&project_id))
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to list project sandboxes: {}", e)))?;

    let project_id_clone = project_id.clone();
    let response: Vec<SandboxInfo> = sandboxes
        .into_iter()
        .map(|s| SandboxInfo {
            id: s.id,
            template_id: String::new(),
            provider_id: String::new(),
            status: bastion_domain::sandbox::value_objects::SandboxStatus::Running,
            created_at: s.created_at,
            expires_at: None,
            purpose: Some(s.purpose),
            project_id: Some(project_id_clone.clone()),
        })
        .collect();

    Ok(Json(serde_json::json!({
        "sandboxes": response,
        "total": response.len()
    })))
}

/// Request body for creating a sandbox.
#[derive(Debug, serde::Deserialize)]
pub struct CreateSandboxBody {
    pub purpose: String,
    #[serde(default)]
    pub template_id: Option<String>,
}

/// Handler for creating a sandbox in a project.
///
/// POST /api/v1/projects/:id/sandboxes
///
/// Creates a new sandbox within the specified project.
pub async fn create_project_sandbox(
    State(state): State<DashboardServerState>,
    Path(project_id): Path<String>,
    Json(body): Json<CreateSandboxBody>,
) -> Result<(StatusCode, Json<SandboxInfo>), ApiError> {
    info!(
        "Creating sandbox for project: {} with purpose: {}",
        project_id, body.purpose
    );

    let purpose = match body.purpose.to_lowercase().as_str() {
        "ad_hoc_test" | "adhoc" | "adhoc_test" => SandboxPurpose::AdHocTest,
        "e2e_test" | "e2e" => SandboxPurpose::E2eTest,
        "proof_of_concept" | "poc" => SandboxPurpose::ProofOfConcept,
        "real_test" | "realtest" => SandboxPurpose::RealTest,
        "pipeline_stage" | "pipeline" => SandboxPurpose::PipelineStage,
        "job" => SandboxPurpose::Job,
        _ => SandboxPurpose::AdHocTest,
    };

    let sandbox: ProjectSandbox = state
        .project_manager
        .create_sandbox(&bastion_domain::project::ProjectId::new(&project_id), purpose)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to create sandbox: {}", e)))?;

    let response = SandboxInfo {
        id: sandbox.id,
        template_id: body.template_id.unwrap_or_default(),
        provider_id: String::new(),
        status: bastion_domain::sandbox::value_objects::SandboxStatus::Pending,
        created_at: sandbox.created_at,
        expires_at: None,
        purpose: Some(sandbox.purpose),
        project_id: Some(project_id),
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Handler for listing all sandboxes globally.
///
/// GET /api/v1/sandboxes
///
/// Returns all sandboxes across all projects.
pub async fn list_all_sandboxes(
    State(state): State<DashboardServerState>,
) -> Result<impl IntoResponse, ApiError> {
    debug!("Listing all sandboxes");

    let sandboxes: Vec<SandboxInfo> = state
        .api_client
        .list_sandboxes()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to list sandboxes: {}", e)))?;

    Ok(Json(serde_json::json!({
        "sandboxes": sandboxes,
        "total": sandboxes.len()
    })))
}

/// Handler for getting a specific sandbox by ID.
///
/// GET /api/v1/sandboxes/:id
///
/// Returns details of the specified sandbox.
pub async fn get_sandbox(
    State(state): State<DashboardServerState>,
    Path(sandbox_id): Path<String>,
) -> Result<Json<SandboxDetailResponse>, ApiError> {
    debug!("Getting sandbox: {}", sandbox_id);

    let sandbox: SandboxDetailResponse = state
        .api_client
        .get_sandbox(&sandbox_id)
        .await
        .map_err(|e| match e {
            crate::error::ApiError::NotFound(_) => {
                ApiError::NotFound(format!("Sandbox not found: {}", sandbox_id))
            }
            _ => ApiError::InternalError(format!("Failed to get sandbox: {}", e)),
        })?;

    Ok(Json(sandbox))
}

/// Handler for terminating a sandbox.
///
/// DELETE /api/v1/sandboxes/:id
///
/// Terminates and cleans up the specified sandbox.
pub async fn terminate_sandbox(
    State(state): State<DashboardServerState>,
    Path(sandbox_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    info!("Terminating sandbox: {}", sandbox_id);

    state
        .api_client
        .terminate_sandbox(&sandbox_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to terminate sandbox: {}", e)))?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_sandbox_body_deserialization() {
        let json = r#"{"purpose": "e2e_test", "template_id": "rust-template"}"#;
        let body: CreateSandboxBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.purpose, "e2e_test");
        assert_eq!(body.template_id, Some("rust-template".to_string()));
    }

    #[test]
    fn test_create_sandbox_body_without_template() {
        let json = r#"{"purpose": "ad_hoc_test"}"#;
        let body: CreateSandboxBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.purpose, "ad_hoc_test");
        assert!(body.template_id.is_none());
    }
}
