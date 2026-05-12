//! Project aggregate and repository.
//!
//! The Project aggregate root encapsulates project-scoped sandbox management.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::{ProjectId, ProjectKind};
use crate::shared::id::SandboxId;

/// Project aggregate root.
///
/// Represents a git repo + .bastion/ directory with isolated config, DB, and pipelines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectKind,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sandbox_ids: Vec<SandboxId>,
}

impl Project {
    pub fn new(id: ProjectId, name: String, path: PathBuf, kind: ProjectKind) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            path,
            kind,
            created_at: now,
            updated_at: now,
            sandbox_ids: Vec::new(),
        }
    }

    pub fn add_sandbox(&mut self, sandbox_id: SandboxId) {
        if !self.sandbox_ids.contains(&sandbox_id) {
            self.sandbox_ids.push(sandbox_id);
            self.updated_at = Utc::now();
        }
    }

    pub fn remove_sandbox(&mut self, sandbox_id: &SandboxId) {
        self.sandbox_ids.retain(|id| id != sandbox_id);
        self.updated_at = Utc::now();
    }

    pub fn update(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// Summary of a project for REST API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    pub kind: ProjectKind,
    pub sandbox_count: usize,
    pub updated_at: DateTime<Utc>,
}

impl From<&Project> for ProjectSummary {
    fn from(project: &Project) -> Self {
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            kind: project.kind,
            sandbox_count: project.sandbox_ids.len(),
            updated_at: project.updated_at,
        }
    }
}

/// Repository port for Project aggregate.
///
/// Defines the interface for persisting and retrieving projects.
/// Implementations live in infrastructure layer.
#[async_trait::async_trait]
pub trait ProjectRepository: Send + Sync {
    async fn save(&self, project: &Project) -> Result<(), crate::shared::DomainError>;
    async fn find_by_id(
        &self,
        id: &ProjectId,
    ) -> Result<Option<Project>, crate::shared::DomainError>;
    async fn find_all(&self) -> Result<Vec<Project>, crate::shared::DomainError>;
    async fn delete(&self, id: &ProjectId) -> Result<(), crate::shared::DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_new() {
        let id = ProjectId::new("proj-1");
        let path = PathBuf::from("/tmp/test-project");
        let project = Project::new(
            id.clone(),
            "Test Project".to_string(),
            path.clone(),
            ProjectKind::Rust,
        );

        assert_eq!(project.id, id);
        assert_eq!(project.name, "Test Project");
        assert_eq!(project.path, path);
        assert_eq!(project.kind, ProjectKind::Rust);
        assert!(project.sandbox_ids.is_empty());
    }

    #[test]
    fn test_project_add_sandbox() {
        let mut project = Project::new(
            ProjectId::new("proj-1"),
            "Test".to_string(),
            PathBuf::from("/tmp"),
            ProjectKind::Generic,
        );

        let sandbox_id = SandboxId::new("sb-1");
        project.add_sandbox(sandbox_id.clone());

        assert_eq!(project.sandbox_ids.len(), 1);
        assert!(project.sandbox_ids.contains(&sandbox_id));
    }

    #[test]
    fn test_project_add_sandbox_no_duplicate() {
        let mut project = Project::new(
            ProjectId::new("proj-1"),
            "Test".to_string(),
            PathBuf::from("/tmp"),
            ProjectKind::Generic,
        );

        let sandbox_id = SandboxId::new("sb-1");
        project.add_sandbox(sandbox_id.clone());
        project.add_sandbox(sandbox_id.clone());

        assert_eq!(project.sandbox_ids.len(), 1);
    }

    #[test]
    fn test_project_remove_sandbox() {
        let mut project = Project::new(
            ProjectId::new("proj-1"),
            "Test".to_string(),
            PathBuf::from("/tmp"),
            ProjectKind::Generic,
        );

        let sandbox_id = SandboxId::new("sb-1");
        project.add_sandbox(sandbox_id.clone());
        project.remove_sandbox(&sandbox_id);

        assert!(project.sandbox_ids.is_empty());
    }

    #[test]
    fn test_project_summary_from_project() {
        let mut project = Project::new(
            ProjectId::new("proj-1"),
            "Test Project".to_string(),
            PathBuf::from("/tmp"),
            ProjectKind::Go,
        );

        let sandbox_id = SandboxId::new("sb-1");
        project.add_sandbox(sandbox_id);

        let summary = ProjectSummary::from(&project);

        assert_eq!(summary.id.as_str(), "proj-1");
        assert_eq!(summary.name, "Test Project");
        assert_eq!(summary.kind, ProjectKind::Go);
        assert_eq!(summary.sandbox_count, 1);
    }

    #[test]
    fn test_project_update_timestamp() {
        let mut project = Project::new(
            ProjectId::new("proj-1"),
            "Test".to_string(),
            PathBuf::from("/tmp"),
            ProjectKind::Generic,
        );

        let original_updated = project.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        project.update();

        assert!(project.updated_at > original_updated);
    }
}
