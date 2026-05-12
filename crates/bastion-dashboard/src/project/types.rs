//! Dashboard-specific project types.
//!
//! This module contains types that are specific to the dashboard crate
//! and are not part of the domain layer.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use bastion_domain::project::{ProjectId, ProjectKind, SandboxPurpose};

/// Summary information about a project for display in the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectKind,
    pub sandbox_count: usize,
}

impl ProjectSummary {
    /// Create a new project summary.
    pub fn new(
        id: ProjectId,
        name: String,
        path: PathBuf,
        kind: ProjectKind,
        sandbox_count: usize,
    ) -> Self {
        Self {
            id,
            name,
            path,
            kind,
            sandbox_count,
        }
    }
}

/// A sandbox associated with a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSandbox {
    pub id: String,
    pub purpose: SandboxPurpose,
    pub status: String,
    pub created_at: String,
}

/// Request to create a new sandbox within a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSandboxRequest {
    pub project_id: ProjectId,
    pub purpose: SandboxPurpose,
    pub template_id: Option<String>,
}

impl CreateSandboxRequest {
    /// Create a new sandbox creation request.
    pub fn new(project_id: ProjectId, purpose: SandboxPurpose) -> Self {
        Self {
            project_id,
            purpose,
            template_id: None,
        }
    }

    /// Create a request with a specific template.
    pub fn with_template(project_id: ProjectId, purpose: SandboxPurpose, template_id: String) -> Self {
        Self {
            project_id,
            purpose,
            template_id: Some(template_id),
        }
    }
}
