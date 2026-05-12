//! Project configuration parsing.
//!
//! Handles reading and parsing of `.bastion/project.toml` files and `.bastion/pipelines/*.toml` files.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::error::ProjectError;
use bastion_domain::project::{PipelineDef, PipelineStage, ProjectId, ProjectKind};

/// Raw project.toml structure as loaded from disk.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectConfigData,
}

/// The `[project]` section of project.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfigData {
    pub name: String,
    #[serde(default)]
    pub kind: Option<ProjectKind>,
}

/// Pipeline TOML structure as loaded from disk.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub stages: Vec<PipelineStageConfig>,
}

/// A single stage in a pipeline TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineStageConfig {
    pub name: String,
    pub image: String,
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    300000 // 5 minutes default
}

impl ProjectConfig {
    /// Load and parse a project.toml file from the given path.
    pub fn load_from_file(path: &Path) -> Result<Self, ProjectError> {
        debug!("Loading project config from: {}", path.display());

        let content = std::fs::read_to_string(path)
            .map_err(|e| ProjectError::ConfigParseError(format!("Failed to read {}: {}", path.display(), e)))?;

        Self::parse(&content)
    }

    /// Parse a project.toml string into a ProjectConfig.
    pub fn parse(content: &str) -> Result<Self, ProjectError> {
        toml::from_str(content)
            .map_err(|e| ProjectError::ConfigParseError(format!("Invalid TOML: {}", e)))
    }

    /// Extract the project name.
    pub fn name(&self) -> &str {
        &self.project.name
    }

    /// Extract the project kind, defaulting to Generic if not specified.
    pub fn kind(&self) -> ProjectKind {
        self.project.kind.unwrap_or_default()
    }
}

impl PipelineConfig {
    /// Load and parse a pipeline TOML file.
    pub fn load_from_file(path: &Path) -> Result<Self, ProjectError> {
        debug!("Loading pipeline config from: {}", path.display());

        let content = std::fs::read_to_string(path)
            .map_err(|e| ProjectError::ConfigParseError(format!("Failed to read {}: {}", path.display(), e)))?;

        Self::parse(&content)
    }

    /// Parse a pipeline TOML string into a PipelineConfig.
    pub fn parse(content: &str) -> Result<Self, ProjectError> {
        toml::from_str(content)
            .map_err(|e| ProjectError::ConfigParseError(format!("Invalid TOML: {}", e)))
    }

    /// Convert to domain PipelineDef.
    pub fn to_pipeline_def(&self) -> PipelineDef {
        PipelineDef {
            name: self.name.clone(),
            description: self.description.clone(),
            stages: self.stages.iter().map(|s| s.to_pipeline_stage()).collect(),
        }
    }
}

impl PipelineStageConfig {
    /// Convert to domain PipelineStage.
    pub fn to_pipeline_stage(&self) -> PipelineStage {
        PipelineStage {
            name: self.name.clone(),
            image: self.image.clone(),
            command: self.command.clone(),
            timeout_ms: self.timeout_ms,
        }
    }
}

/// Get the path to the pipelines directory for a project.
pub fn pipelines_dir(project_path: &Path) -> PathBuf {
    project_path.join(".bastion").join("pipelines")
}

/// List all pipeline TOML files in a project's pipelines directory.
pub fn list_pipeline_files(project_path: &Path) -> Result<Vec<PathBuf>, ProjectError> {
    let pipelines_path = pipelines_dir(project_path);

    if !pipelines_path.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&pipelines_path)
        .map_err(|e| ProjectError::ConfigParseError(format!("Failed to read pipelines dir: {}", e)))?;

    let mut pipeline_files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            pipeline_files.push(path);
        }
    }

    Ok(pipeline_files)
}

/// Load all pipelines for a project.
pub fn load_pipelines(project_path: &Path) -> Result<Vec<PipelineDef>, ProjectError> {
    let pipeline_files = list_pipeline_files(project_path)?;

    let mut pipelines = Vec::new();
    for file_path in pipeline_files {
        match PipelineConfig::load_from_file(&file_path) {
            Ok(config) => {
                pipelines.push(config.to_pipeline_def());
            }
            Err(e) => {
                debug!("Failed to load pipeline {}: {}", file_path.display(), e);
            }
        }
    }

    Ok(pipelines)
}

/// Dashboard state for persisting UI preferences.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct DashboardState {
    /// Currently selected project ID.
    #[serde(default)]
    pub selected_project: Option<String>,
    /// Collapsed section IDs.
    #[serde(default)]
    pub collapsed_sections: Vec<String>,
    /// Order of columns.
    #[serde(default)]
    pub column_order: Vec<String>,
    /// UI theme preference.
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "system".to_string()
}

/// Get the path to the dashboard state file.
pub fn dashboard_state_path(project_path: &Path) -> PathBuf {
    project_path.join(".bastion").join("dashboard.json")
}

/// Load dashboard state from .bastion/dashboard.json.
pub fn load_dashboard_state(project_path: &Path) -> Result<DashboardState, ProjectError> {
    let state_path = dashboard_state_path(project_path);

    if !state_path.exists() {
        return Ok(DashboardState::default());
    }

    let content = std::fs::read_to_string(&state_path)
        .map_err(|e| ProjectError::ConfigParseError(format!("Failed to read {}: {}", state_path.display(), e)))?;

    serde_json::from_str(&content)
        .map_err(|e| ProjectError::ConfigParseError(format!("Invalid JSON: {}", e)))
}

/// Save dashboard state to .bastion/dashboard.json.
pub fn save_dashboard_state(project_path: &Path, state: &DashboardState) -> Result<(), ProjectError> {
    let state_path = dashboard_state_path(project_path);

    // Ensure the .bastion directory exists
    let bastion_dir = state_path.parent().unwrap();
    if !bastion_dir.exists() {
        std::fs::create_dir_all(bastion_dir)
            .map_err(|e| ProjectError::ConfigParseError(format!("Failed to create dir: {}", e)))?;
    }

    let content = serde_json::to_string_pretty(state)
        .map_err(|e| ProjectError::ConfigParseError(format!("Failed to serialize: {}", e)))?;

    std::fs::write(&state_path, content)
        .map_err(|e| ProjectError::ConfigParseError(format!("Failed to write {}: {}", state_path.display(), e)))
}

/// Project metadata extracted from configuration.
#[derive(Debug, Clone)]
pub struct ProjectMetadata {
    pub id: ProjectId,
    pub name: String,
    pub kind: ProjectKind,
}

impl ProjectMetadata {
    /// Create new project metadata.
    pub fn new(id: ProjectId, name: String, kind: ProjectKind) -> Self {
        Self { id, name, kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_config() {
        let content = r#"
[project]
name = "my-project"
kind = "rust"
"#;
        let config = ProjectConfig::parse(content).unwrap();
        assert_eq!(config.name(), "my-project");
        assert_eq!(config.kind(), ProjectKind::Rust);
    }

    #[test]
    fn test_parse_config_without_kind() {
        let content = r#"
[project]
name = "my-project"
"#;
        let config = ProjectConfig::parse(content).unwrap();
        assert_eq!(config.name(), "my-project");
        assert_eq!(config.kind(), ProjectKind::Generic);
    }

    #[test]
    fn test_parse_invalid_toml() {
        let content = r#"
[project
name = "my-project"
"#;
        let result = ProjectConfig::parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_project_metadata_new() {
        let id = ProjectId::new("test-id");
        let meta = ProjectMetadata::new(id.clone(), "Test".to_string(), ProjectKind::Go);
        assert_eq!(meta.id, id);
        assert_eq!(meta.name, "Test");
        assert_eq!(meta.kind, ProjectKind::Go);
    }
}
