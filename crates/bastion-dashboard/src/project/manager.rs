//! Project manager implementation.
//!
//! Provides the `ProjectManager` struct which implements `ProjectManagerPort`
//! for managing projects on the local filesystem.

use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use bastion_domain::project::{PipelineDef, Project, ProjectId, ProjectKind, SandboxPurpose};
use bastion_domain::shared::SandboxId;

use crate::api::client::DashboardApiPort;
use crate::error::ProjectError;
use crate::project::config::{load_dashboard_state, load_pipelines, save_dashboard_state, DashboardState, ProjectConfig, ProjectMetadata};
use crate::project::types::{CreateSandboxRequest, ProjectSandbox, ProjectSummary};
use crate::api::DashboardApiClient;

/// Directories created when initializing a new project.
const BASTION_DIRS: &[&str] = &[
    "pipelines",
    "db",
    "templates",
    "providers",
    "capabilities",
    "catalog",
    "runtime",
];

/// Port trait for project management operations.
///
/// This trait defines the interface for project management,
/// allowing for different implementations (local filesystem, remote, etc.).
#[async_trait::async_trait]
pub trait ProjectManagerPort: Send + Sync {
    /// Open an existing project from a path.
    async fn open_project(&self, path: &Path) -> Result<Project, ProjectError>;

    /// Initialize a new project at the given path.
    async fn init_project(&self, path: &Path, kind: ProjectKind) -> Result<Project, ProjectError>;

    /// List all projects found under the given root directory.
    async fn list_projects(&self, root: &Path) -> Result<Vec<ProjectSummary>, ProjectError>;

    /// Get sandboxes associated with a project.
    async fn get_project_sandboxes(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectSandbox>, ProjectError>;

    /// Create a new sandbox for a project.
    async fn create_sandbox(
        &self,
        project_id: &ProjectId,
        purpose: SandboxPurpose,
    ) -> Result<ProjectSandbox, ProjectError>;

    /// Terminate a sandbox.
    async fn terminate_sandbox(&self, sandbox_id: &SandboxId) -> Result<(), ProjectError>;

    /// List all pipelines for a project.
    async fn list_pipelines(&self, project_path: &Path) -> Result<Vec<PipelineDef>, ProjectError>;

    /// Load dashboard state from .bastion/dashboard.json.
    async fn load_dashboard_state(&self, project_path: &Path) -> Result<DashboardState, ProjectError>;

    /// Save dashboard state to .bastion/dashboard.json.
    async fn save_dashboard_state(&self, project_path: &Path, state: &DashboardState) -> Result<(), ProjectError>;
}

/// Project manager for local filesystem operations.
///
/// Manages project lifecycle including creation, discovery, and sandbox management.
pub struct ProjectManager {
    api_client: DashboardApiClient,
}

impl ProjectManager {
    /// Create a new ProjectManager with the given API client.
    pub fn new(api_client: DashboardApiClient) -> Self {
        Self { api_client }
    }

    /// Create a new ProjectManager with a default API client.
    pub fn with_default_client() -> Self {
        Self {
            api_client: DashboardApiClient::default(),
        }
    }

    /// Get the path to the .bastion directory for a project.
    fn bastion_dir(path: &Path) -> PathBuf {
        path.join(".bastion")
    }

    /// Get the path to the project.toml file.
    fn project_toml_path(path: &Path) -> PathBuf {
        Self::bastion_dir(path).join("project.toml")
    }

    /// Ensure the .bastion directory structure exists.
    fn ensure_bastion_dirs(path: &Path) -> Result<PathBuf, ProjectError> {
        let bastion_dir = Self::bastion_dir(path);

        if !bastion_dir.exists() {
            std::fs::create_dir_all(&bastion_dir)
                .map_err(|e| ProjectError::ProjectInitFailed(format!("Failed to create .bastion dir: {}", e)))?;
        }

        for dir in BASTION_DIRS {
            let dir_path = bastion_dir.join(dir);
            if !dir_path.exists() {
                std::fs::create_dir_all(&dir_path)
                    .map_err(|e| ProjectError::ProjectInitFailed(format!("Failed to create {} dir: {}", dir, e)))?;
            }
        }

        Ok(bastion_dir)
    }

    /// Write the initial project.toml file.
    fn write_project_toml(path: &Path, name: &str, kind: ProjectKind) -> Result<(), ProjectError> {
        let toml_path = Self::project_toml_path(path);

        // Convert ProjectKind to string representation
        let kind_str = match kind {
            ProjectKind::Rust => "rust",
            ProjectKind::NodeJs => "nodejs",
            ProjectKind::Python => "python",
            ProjectKind::Go => "go",
            ProjectKind::Generic => "generic",
        };

        let toml_content = format!(
            r#"# Bastion project configuration
# This file is managed by the bastion-dashboard crate

[project]
name = "{}"
kind = "{}"
"#,
            name, kind_str
        );

        std::fs::write(&toml_path, toml_content)
            .map_err(|e| ProjectError::ProjectInitFailed(format!("Failed to write project.toml: {}", e)))?;

        debug!("Wrote project.toml to: {}", toml_path.display());
        Ok(())
    }

    /// Extract project metadata from a path.
    fn extract_metadata(path: &Path) -> Result<ProjectMetadata, ProjectError> {
        let config = ProjectConfig::load_from_file(&Self::project_toml_path(path))?;

        // Generate a stable ID from the path
        let id = ProjectId::new(path.to_string_lossy().to_string());

        Ok(ProjectMetadata::new(
            id,
            config.name().to_string(),
            config.kind(),
        ))
    }
}

#[async_trait::async_trait]
impl ProjectManagerPort for ProjectManager {
    async fn open_project(&self, path: &Path) -> Result<Project, ProjectError> {
        info!("Opening project at: {}", path.display());

        let canonical_path = path.canonicalize()
            .map_err(|e| ProjectError::InvalidProjectPath(format!("Invalid path {}: {}", path.display(), e)))?;

        let bastion_dir = Self::bastion_dir(&canonical_path);
        if !bastion_dir.exists() {
            return Err(ProjectError::ProjectNotFound(format!(
                "No .bastion directory found at: {}",
                canonical_path.display()
            )));
        }

        let metadata = Self::extract_metadata(&canonical_path)?;
        debug!("Found project: {} ({})", metadata.name, metadata.id);

        Ok(Project::new(
            metadata.id,
            metadata.name,
            canonical_path,
            metadata.kind,
        ))
    }

    async fn init_project(&self, path: &Path, kind: ProjectKind) -> Result<Project, ProjectError> {
        info!("Initializing new project at: {} with kind: {:?}", path.display(), kind);

        // Create the .bastion directory structure
        Self::ensure_bastion_dirs(path)?;

        // Determine project name from directory
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string();

        // Write the project.toml file
        Self::write_project_toml(path, &name, kind)?;

        // Create the project
        let project_id = ProjectId::new(path.to_string_lossy().to_string());
        let project = Project::new(
            project_id.clone(),
            name,
            path.to_path_buf(),
            kind,
        );

        info!(
            "Project initialized successfully: {} at {}",
            project.name,
            path.display()
        );

        Ok(project)
    }

    async fn list_projects(&self, root: &Path) -> Result<Vec<ProjectSummary>, ProjectError> {
        debug!("Scanning for projects under: {}", root.display());

        let mut projects = Vec::new();

        // Walk the directory tree looking for .bastion directories
        let entries = std::fs::read_dir(root)
            .map_err(|e| ProjectError::InvalidProjectPath(format!("Cannot read root {}: {}", root.display(), e)))?;

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                let bastion_dir = Self::bastion_dir(&entry_path);
                if bastion_dir.exists() {
                    match Self::extract_metadata(&entry_path) {
                        Ok(metadata) => {
                            let summary = ProjectSummary::new(
                                metadata.id,
                                metadata.name.clone(),
                                entry_path.clone(),
                                metadata.kind,
                                0, // Sandbox count unknown without API call
                            );
                            projects.push(summary);
                            debug!("Found project: {}", metadata.name);
                        }
                        Err(e) => {
                            warn!("Failed to read project at {}: {}", entry_path.display(), e);
                        }
                    }
                }

                // Recurse into subdirectories (one level deep for workspaces)
                if let Ok(sub_entries) = std::fs::read_dir(&entry_path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if sub_path.is_dir() {
                            let sub_bastion_dir = Self::bastion_dir(&sub_path);
                            if sub_bastion_dir.exists()
                                && let Ok(metadata) = Self::extract_metadata(&sub_path)
                            {
                                let summary = ProjectSummary::new(
                                    metadata.id,
                                    metadata.name.clone(),
                                    sub_path,
                                    metadata.kind,
                                    0,
                                );
                                projects.push(summary);
                            }
                        }
                    }
                }
            }
        }

        debug!("Found {} projects", projects.len());
        Ok(projects)
    }

    async fn get_project_sandboxes(
        &self,
        project_id: &ProjectId,
    ) -> Result<Vec<ProjectSandbox>, ProjectError> {
        debug!("Getting sandboxes for project: {}", project_id);

        // Delegate to API client
        self.api_client
            .list_sandboxes_for_project(project_id.as_str())
            .await
            .map_err(|e| ProjectError::ProjectNotFound(format!("API error: {}", e)))
    }

    async fn create_sandbox(
        &self,
        project_id: &ProjectId,
        purpose: SandboxPurpose,
    ) -> Result<ProjectSandbox, ProjectError> {
        info!("Creating sandbox for project: {} with purpose: {:?}", project_id, purpose);

        let request = CreateSandboxRequest::new(project_id.clone(), purpose);

        self.api_client
            .create_sandbox(request)
            .await
            .map_err(|e| ProjectError::ProjectInitFailed(format!("API error: {}", e)))
    }

    async fn terminate_sandbox(&self, sandbox_id: &SandboxId) -> Result<(), ProjectError> {
        info!("Terminating sandbox: {}", sandbox_id);

        self.api_client
            .terminate_sandbox(sandbox_id.as_str())
            .await
            .map_err(|e| ProjectError::IoError(std::io::Error::other(format!("API error: {}", e))))
    }

    async fn list_pipelines(&self, project_path: &Path) -> Result<Vec<PipelineDef>, ProjectError> {
        debug!("Listing pipelines for project: {}", project_path.display());
        load_pipelines(project_path)
    }

    async fn load_dashboard_state(&self, project_path: &Path) -> Result<DashboardState, ProjectError> {
        debug!("Loading dashboard state for project: {}", project_path.display());
        load_dashboard_state(project_path)
    }

    async fn save_dashboard_state(&self, project_path: &Path, state: &DashboardState) -> Result<(), ProjectError> {
        debug!("Saving dashboard state for project: {}", project_path.display());
        save_dashboard_state(project_path, state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_project() -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_path = temp_dir.path().join("test-project");
        fs::create_dir_all(&project_path).unwrap();
        (temp_dir, project_path)
    }

    #[tokio::test]
    async fn test_init_project() {
        let (_temp_dir, project_path) = create_temp_project();

        let manager = ProjectManager::with_default_client();
        let _result = manager.init_project(&project_path, ProjectKind::Rust).await;

        // This will fail because API client is not connected, but we can test the filesystem parts
        // In a real test, we'd mock the API client

        // Verify .bastion directory was created
        let bastion_dir = project_path.join(".bastion");
        assert!(bastion_dir.exists());

        // Verify subdirectories
        for dir in BASTION_DIRS {
            assert!(bastion_dir.join(dir).exists());
        }

        // Verify project.toml was created
        let project_toml = bastion_dir.join("project.toml");
        assert!(project_toml.exists());
    }

    #[tokio::test]
    async fn test_open_project_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = ProjectManager::with_default_client();

        let result = manager.open_project(temp_dir.path()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_open_invalid_path() {
        let manager = ProjectManager::with_default_client();
        let result = manager.open_project(Path::new("/nonexistent/path")).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_bastion_dir_construction() {
        let path = Path::new("/home/user/project");
        let bastion = ProjectManager::bastion_dir(path);
        assert_eq!(bastion, PathBuf::from("/home/user/project/.bastion"));
    }

    #[test]
    fn test_project_toml_path() {
        let path = Path::new("/home/user/project");
        let toml_path = ProjectManager::project_toml_path(path);
        assert_eq!(toml_path, PathBuf::from("/home/user/project/.bastion/project.toml"));
    }
}
