//! API request and response types.
//!
//! Defines the types used for communicating with the Bastion gateway API.

use serde::{Deserialize, Serialize};

use bastion_domain::project::SandboxPurpose;
use bastion_domain::sandbox::value_objects::{ResourcesSpec, SandboxStatus};

// Re-export CreateSandboxRequest from project types for API use
pub use crate::project::types::CreateSandboxRequest;

/// Response containing project details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDetailResponse {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Response containing a list of sandboxes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxListResponse {
    pub sandboxes: Vec<SandboxInfo>,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Information about a sandbox in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInfo {
    pub id: String,
    pub template_id: String,
    pub provider_id: String,
    pub status: SandboxStatus,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub purpose: Option<SandboxPurpose>,
    pub project_id: Option<String>,
}

/// Response containing sandbox details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxDetailResponse {
    pub id: String,
    pub template_id: String,
    pub provider_id: String,
    pub status: SandboxStatus,
    pub resources: ResourcesSpec,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub purpose: Option<SandboxPurpose>,
    pub project_id: Option<String>,
}

/// Response containing sandbox metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub sandbox_id: String,
    pub cpu_usage_percent: f64,
    pub memory_usage_mb: u64,
    pub disk_usage_mb: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub timestamp: String,
}

/// Response containing resource usage information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResourcesResponse {
    pub sandbox_id: String,
    pub resources: ResourcesSpec,
    pub usage: ResourceUsage,
}

/// Current resource usage for a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

/// API error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ApiErrorResponse {
    /// Create a new API error response.
    pub fn new(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            details: None,
        }
    }

    /// Create an error response with details.
    pub fn with_details(error: impl Into<String>, message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            details: Some(details.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::project::ProjectId;

    #[test]
    fn test_create_sandbox_request_serialization() {
        let request = CreateSandboxRequest {
            project_id: ProjectId::new("proj-1"),
            purpose: SandboxPurpose::E2eTest,
            template_id: Some("template-rust".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("proj-1"));
        assert!(json.contains("e2e_test"));
        assert!(json.contains("template-rust"));
    }

    #[test]
    fn test_create_sandbox_request_without_template() {
        let request = CreateSandboxRequest {
            project_id: ProjectId::new("proj-1"),
            purpose: SandboxPurpose::AdHocTest,
            template_id: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: CreateSandboxRequest = serde_json::from_str(&json).unwrap();
        assert!(parsed.template_id.is_none());
    }

    #[test]
    fn test_api_error_response_new() {
        let error = ApiErrorResponse::new("NotFound", "Project not found");
        assert_eq!(error.error, "NotFound");
        assert_eq!(error.message, "Project not found");
        assert!(error.details.is_none());
    }

    #[test]
    fn test_api_error_response_with_details() {
        let error = ApiErrorResponse::with_details(
            "ValidationError",
            "Invalid request",
            "field 'name' is required",
        );
        assert_eq!(error.error, "ValidationError");
        assert!(error.details.is_some());
        assert_eq!(error.details.unwrap(), "field 'name' is required");
    }

    #[test]
    fn test_sandbox_info_deserialization() {
        let json = r#"{
            "id": "sb-123",
            "template_id": "tmpl-1",
            "provider_id": "prov-1",
            "status": "running",
            "created_at": "2024-01-01T00:00:00Z",
            "purpose": "ad_hoc_test",
            "project_id": "proj-1"
        }"#;

        let info: SandboxInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "sb-123");
        assert_eq!(info.status, SandboxStatus::Running);
        assert_eq!(info.purpose, Some(SandboxPurpose::AdHocTest));
    }
}
