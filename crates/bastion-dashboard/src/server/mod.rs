//! HTTP server module for the bastion-dashboard.
//!
//! This module provides the HTTP server implementation for the dashboard,
//! including routing, handlers, and application setup.
//!
//! ## Server Architecture
//!
//! The server is built using Axum and provides a REST API for managing
//! projects and sandboxes, as well as Server-Sent Events (SSE) for
//! real-time updates.
//!
//! ### Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | /api/v1/health | Health check |
//! | GET | /api/v1/projects | List all projects |
//! | GET | /api/v1/projects/:id | Get project details |
//! | POST | /api/v1/projects | Create a new project |
//! | GET | /api/v1/projects/:id/sandboxes | List project sandboxes |
//! | POST | /api/v1/projects/:id/sandboxes | Create project sandbox |
//! | GET | /api/v1/sandboxes | List all sandboxes |
//! | GET | /api/v1/sandboxes/:id | Get sandbox details |
//! | DELETE | /api/v1/sandboxes/:id | Terminate sandbox |
//! | GET | /api/v1/projects/:id/metrics | Get project metrics |
//! | GET | /api/v1/metrics | Get global metrics |
//! | GET | /api/v1/events | SSE event stream |
//!
//! ## Example
//!
//! ```ignore
//! use bastion_dashboard::{DashboardApp, DashboardApiClient, ProjectManager};
//!
//! #[tokio::main]
//! async fn main() {
//!     let api_client = DashboardApiClient::new("http://localhost:8080/api/v1");
//!     let manager = ProjectManager::new(api_client.clone());
//!
//!     let app = DashboardApp::with_default_port(
//!         std::sync::Arc::new(manager),
//!         std::sync::Arc::new(api_client),
//!         "http://localhost:8080".to_string(),
//!     );
//!
//!     app.serve().await;
//! }
//! ```

pub mod app;
pub mod handlers;
pub mod router;
pub mod state;

// Re-exports for convenience
pub use app::{serve, DashboardApp, ServerHandle, ServerError, DEFAULT_SERVER_PORT};
pub use state::DashboardServerState;
