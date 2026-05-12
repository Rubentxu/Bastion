//! HTTP request handlers.
//!
//! This module exports all HTTP request handlers used by the dashboard server.

pub mod events;
pub mod metrics;
pub mod pipelines;
pub mod projects;
pub mod sandboxes;

// Re-export handler functions for use in router
pub use events::sse_events;
pub use metrics::{get_global_metrics, get_project_metrics};
pub use pipelines::{get_dashboard_state, list_pipelines, put_dashboard_state};
pub use projects::{create_project, get_project, list_projects};
pub use sandboxes::{
    create_project_sandbox, get_sandbox, list_all_sandboxes, list_project_sandboxes,
    terminate_sandbox,
};
