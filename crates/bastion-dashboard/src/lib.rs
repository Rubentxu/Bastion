//! # Bastion Dashboard
//!
//! Library for the Bastion dashboard application. Provides project management
//! and API client functionality for interacting with the Bastion sandbox gateway.
//!
//! ## Modules
//!
//! - [`api`] - REST API client for gateway communication
//! - [`error`] - Error types for dashboard operations
//! - [`project`] - Project management functionality
//!
//! ## Example
//!
//! ```ignore
//! use bastion_dashboard::{ProjectManager, DashboardApiClient};
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = DashboardApiClient::new("http://localhost:8080/api/v1");
//!     let manager = ProjectManager::new(client);
//!
//!     let projects = manager.list_projects("/path/to/projects").await;
//! }
//! ```

pub mod api;
pub mod error;
pub mod project;
pub mod server;

// UI module (WASM-based dashboard UI) - only available with csr feature
#[cfg(feature = "csr")]
pub mod ui;

// Re-exports for commonly used types
pub use api::{DashboardApiClient, DashboardApiPort, SseEvent};
pub use error::{ApiError, DashboardError, ProjectError, SseError};
pub use project::{DashboardState, ProjectManager, ProjectManagerPort, ProjectSummary};
pub use server::{DashboardApp, DashboardServerState, ServerHandle, ServerError};

// UI re-exports when csr feature is enabled
#[cfg(feature = "csr")]
pub use ui::app::App;
