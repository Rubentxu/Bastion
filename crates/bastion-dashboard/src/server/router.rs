//! HTTP router configuration.
//!
//! Sets up the Axum router with all API endpoints and middleware.

use axum::{
    routing::{delete, get, post, put},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::server::handlers;
use crate::server::state::DashboardServerState;

/// Create the Axum router with all API routes.
///
/// # Routes
///
/// - `GET /api/v1/health` - Health check
/// - `GET /api/v1/projects` - List all projects
/// - `GET /api/v1/projects/:id` - Get project details
/// - `POST /api/v1/projects` - Create a new project
/// - `GET /api/v1/projects/:id/sandboxes` - List project sandboxes
/// - `POST /api/v1/projects/:id/sandboxes` - Create project sandbox
/// - `GET /api/v1/sandboxes` - List all sandboxes
/// - `GET /api/v1/sandboxes/:id` - Get sandbox details
/// - `DELETE /api/v1/sandboxes/:id` - Terminate sandbox
/// - `GET /api/v1/projects/:id/metrics` - Get project metrics
/// - `GET /api/v1/metrics` - Get global metrics
/// - `GET /api/v1/events` - SSE event stream
pub fn create_router(state: DashboardServerState) -> Router {
    info!("Creating API router");

    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:3000".parse().unwrap(),
            "http://127.0.0.1:3000".parse().unwrap(),
        ])
        .allow_methods(Any)
        .allow_headers(Any);

    let router = Router::new()
        // Health check
        .route("/api/v1/health", get(health_check))
        // Project routes
        .route("/api/v1/projects", get(handlers::list_projects))
        .route("/api/v1/projects", post(handlers::create_project))
        .route("/api/v1/projects/{id}", get(handlers::get_project))
        // Project sandbox routes
        .route(
            "/api/v1/projects/{id}/sandboxes",
            get(handlers::list_project_sandboxes),
        )
        .route(
            "/api/v1/projects/{id}/sandboxes",
            post(handlers::create_project_sandbox),
        )
        // Global sandbox routes
        .route("/api/v1/sandboxes", get(handlers::list_all_sandboxes))
        .route("/api/v1/sandboxes/{id}", get(handlers::get_sandbox))
        .route("/api/v1/sandboxes/{id}", delete(handlers::terminate_sandbox))
        // Metrics routes
        .route(
            "/api/v1/projects/{id}/metrics",
            get(handlers::get_project_metrics),
        )
        .route("/api/v1/metrics", get(handlers::get_global_metrics))
        // SSE events
        .route("/api/v1/events", get(handlers::sse_events))
        // Pipeline routes
        .route("/api/v1/projects/{id}/pipelines", get(handlers::list_pipelines))
        // Dashboard state routes
        .route("/api/v1/dashboard/state", get(handlers::get_dashboard_state))
        .route("/api/v1/dashboard/state", put(handlers::put_dashboard_state))
        // Apply state and middleware
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    router
}

/// Health check handler.
///
/// GET /api/v1/health
///
/// Returns 200 OK if the server is healthy.
async fn health_check() -> &'static str {
    "OK"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::DashboardApiClient;
    use crate::project::ProjectManager;
    use std::sync::Arc;

    fn create_test_router() -> Router {
        let api_client = DashboardApiClient::default();
        let manager = ProjectManager::new(api_client.clone());
        let state = DashboardServerState {
            project_manager: Arc::new(manager),
            api_client: Arc::new(api_client),
            gateway_url: "http://localhost:8080".to_string(),
        };
        create_router(state)
    }

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        assert_eq!(response, "OK");
    }

    #[tokio::test]
    async fn test_router_creation() {
        // Just verify router creation succeeds without panic
        let _router = create_test_router();
        // If we got here, router creation was successful
    }
}
