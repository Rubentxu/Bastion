//! Dashboard API client.
//!
//! Provides the `DashboardApiPort` trait and `DashboardApiClient` implementation
//! for communicating with the Bastion gateway via HTTP.

use reqwest::Client;
use tracing::debug;

use bastion_domain::project::SandboxPurpose;

use crate::api::events::{parse_sse_stream, SseEvent};
use crate::api::types::{
    CreateSandboxRequest, MetricsResponse, SandboxDetailResponse,
    SandboxInfo, SandboxListResponse, SandboxResourcesResponse,
};
use crate::error::ApiError;
use crate::project::types::ProjectSandbox;

/// Default base URL for the dashboard API.
const DEFAULT_BASE_URL: &str = "http://localhost:8080/api/v1";

/// Port trait for dashboard API operations.
///
/// Defines the interface for interacting with the Bastion gateway API.
#[async_trait::async_trait]
pub trait DashboardApiPort: Send + Sync {
    /// List all sandboxes.
    async fn list_sandboxes(&self) -> Result<Vec<SandboxInfo>, ApiError>;

    /// List sandboxes for a specific project.
    async fn list_sandboxes_for_project(&self, project_id: &str) -> Result<Vec<ProjectSandbox>, ApiError>;

    /// Get details of a specific sandbox.
    async fn get_sandbox(&self, sandbox_id: &str) -> Result<SandboxDetailResponse, ApiError>;

    /// Get metrics for a sandbox.
    async fn get_metrics(&self, sandbox_id: &str) -> Result<MetricsResponse, ApiError>;

    /// Get resource usage for a sandbox.
    async fn get_sandbox_resources(&self, sandbox_id: &str) -> Result<SandboxResourcesResponse, ApiError>;

    /// Subscribe to SSE events.
    async fn subscribe_events(&self) -> Result<Vec<SseEvent>, ApiError>;

    /// Create a new sandbox.
    async fn create_sandbox(&self, request: CreateSandboxRequest) -> Result<ProjectSandbox, ApiError>;

    /// Terminate a sandbox.
    async fn terminate_sandbox(&self, sandbox_id: &str) -> Result<(), ApiError>;
}

/// HTTP client for the dashboard API.
///
/// Communicates with the Bastion gateway via REST endpoints.
#[derive(Clone)]
pub struct DashboardApiClient {
    client: Client,
    base_url: String,
}

impl Default for DashboardApiClient {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl DashboardApiClient {
    /// Create a new API client with the given base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Create a new API client with a custom client and base URL.
    pub fn with_client(client: Client, base_url: impl Into<String>) -> Self {
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    /// Make a GET request to the API.
    async fn get<T: for<'de> serde::de::Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        debug!("GET: {}", url);

        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(ApiError::NetworkError)?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ApiError::NotFound(format!("Resource not found: {}", path)));
            }
            return Err(ApiError::InternalError(format!(
                "Request failed with status: {}",
                status
            )));
        }

        response
            .json()
            .await
            .map_err(|e| ApiError::InvalidResponse(format!("JSON parse error: {}", e)))
    }

    /// Make a POST request to the API.
    async fn post<T: for<'de> serde::de::Deserialize<'de>>(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        debug!("POST: {}", url);

        let response = self.client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(ApiError::NetworkError)?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 400 {
                return Err(ApiError::BadRequest(format!("Bad request: {}", path)));
            }
            if status.as_u16() == 404 {
                return Err(ApiError::NotFound(format!("Resource not found: {}", path)));
            }
            return Err(ApiError::InternalError(format!(
                "Request failed with status: {}",
                status
            )));
        }

        response
            .json()
            .await
            .map_err(|e| ApiError::InvalidResponse(format!("JSON parse error: {}", e)))
    }

    /// Make a DELETE request to the API.
    async fn delete(&self, path: &str) -> Result<(), ApiError> {
        let url = format!("{}{}", self.base_url, path);
        debug!("DELETE: {}", url);

        let response = self.client
            .delete(&url)
            .send()
            .await
            .map_err(ApiError::NetworkError)?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ApiError::NotFound(format!("Resource not found: {}", path)));
            }
            return Err(ApiError::InternalError(format!(
                "Request failed with status: {}",
                status
            )));
        }

        Ok(())
    }

    /// Subscribe to SSE events from the API.
    async fn subscribe_sse(&self, path: &str) -> Result<Vec<SseEvent>, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        debug!("SSE Subscribe: {}", url);

        let response = self.client
            .get(&url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(ApiError::NetworkError)?;

        if !response.status().is_success() {
            return Err(ApiError::InternalError(format!(
                "SSE request failed with status: {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(ApiError::NetworkError)?;

        parse_sse_stream(&bytes)
            .map_err(|e| ApiError::InvalidResponse(format!("SSE parse error: {}", e)))
    }
}

#[async_trait::async_trait]
impl DashboardApiPort for DashboardApiClient {
    async fn list_sandboxes(&self) -> Result<Vec<SandboxInfo>, ApiError> {
        let response: SandboxListResponse = self.get("/sandboxes").await?;
        Ok(response.sandboxes)
    }

    async fn list_sandboxes_for_project(&self, project_id: &str) -> Result<Vec<ProjectSandbox>, ApiError> {
        let response: SandboxListResponse = self.get(&format!("/projects/{}/sandboxes", project_id)).await?;

        let sandboxes = response.sandboxes.into_iter().map(|info| {
            ProjectSandbox {
                id: info.id,
                purpose: info.purpose.unwrap_or(SandboxPurpose::AdHocTest),
                status: info.status.to_string(),
                created_at: info.created_at,
            }
        }).collect();

        Ok(sandboxes)
    }

    async fn get_sandbox(&self, sandbox_id: &str) -> Result<SandboxDetailResponse, ApiError> {
        self.get(&format!("/sandboxes/{}", sandbox_id)).await
    }

    async fn get_metrics(&self, sandbox_id: &str) -> Result<MetricsResponse, ApiError> {
        self.get(&format!("/sandboxes/{}/metrics", sandbox_id)).await
    }

    async fn get_sandbox_resources(&self, sandbox_id: &str) -> Result<SandboxResourcesResponse, ApiError> {
        self.get(&format!("/sandboxes/{}/resources", sandbox_id)).await
    }

    async fn subscribe_events(&self) -> Result<Vec<SseEvent>, ApiError> {
        self.subscribe_sse("/events").await
    }

    async fn create_sandbox(&self, request: CreateSandboxRequest) -> Result<ProjectSandbox, ApiError> {
        #[derive(serde::Deserialize)]
        struct CreateResponse {
            id: String,
            purpose: SandboxPurpose,
            status: String,
            created_at: String,
        }

        let response: CreateResponse = self.post("/sandboxes", &request).await?;

        Ok(ProjectSandbox {
            id: response.id,
            purpose: response.purpose,
            status: response.status,
            created_at: response.created_at,
        })
    }

    async fn terminate_sandbox(&self, sandbox_id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/sandboxes/{}", sandbox_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_api_client_default() {
        let client = DashboardApiClient::default();
        assert_eq!(client.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn test_dashboard_api_client_with_url() {
        let client = DashboardApiClient::new("http://localhost:9000/api/v1");
        assert_eq!(client.base_url, "http://localhost:9000/api/v1");
    }

    #[test]
    fn test_dashboard_api_client_with_client() {
        let client = Client::new();
        let api = DashboardApiClient::with_client(client, "http://custom:1234/api");
        assert_eq!(api.base_url, "http://custom:1234/api");
    }
}
