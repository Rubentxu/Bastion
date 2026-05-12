//! Project management module.
//!
//! This module provides project management functionality including:
//! - Opening and initializing projects
//! - Listing projects
//! - Managing project-scoped sandboxes

pub mod config;
pub mod manager;
pub mod types;

pub use config::{DashboardState, ProjectConfig, ProjectMetadata};
pub use manager::{ProjectManager, ProjectManagerPort};
pub use types::{CreateSandboxRequest, ProjectSandbox, ProjectSummary};
