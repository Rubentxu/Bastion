//! Project domain types.
//!
//! Core types for project-centric dashboard: ProjectId, ProjectKind, SandboxPurpose, PipelineDef.

use serde::{Deserialize, Serialize};
use std::fmt::Display;

/// Unique identifier for a project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ProjectId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ProjectId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Kind of project (determines default sandbox templates, pipeline stages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Rust,
    NodeJs,
    Python,
    Go,
    #[default]
    Generic,
}

/// Purpose of a sandbox — determines lifecycle, billing, and isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPurpose {
    /// Ad-hoc testing from dashboard/CLI
    AdHocTest,
    /// Proof-of-concept exploration
    ProofOfConcept,
    /// End-to-end integration tests
    E2eTest,
    /// Real user-facing tests (staging, canary)
    RealTest,
    /// Pipeline stage execution (PipelineStage)
    PipelineStage,
    /// One-off job execution (compile, deploy, etc.)
    Job,
}

impl SandboxPurpose {
    pub fn billing_tag(&self) -> &'static str {
        match self {
            Self::AdHocTest => "adhoc",
            Self::ProofOfConcept => "poc",
            Self::E2eTest => "e2e",
            Self::RealTest => "realtest",
            Self::PipelineStage => "pipeline",
            Self::Job => "job",
        }
    }
}

/// Pipeline definition loaded from .bastion/pipelines/*.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDef {
    pub name: String,
    pub description: String,
    pub stages: Vec<PipelineStage>,
}

/// A single stage in a pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub name: String,
    pub image: String,
    pub command: String,
    pub timeout_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_id_new() {
        let id = ProjectId::new("test-id");
        assert_eq!(id.as_str(), "test-id");
    }

    #[test]
    fn test_project_id_generate() {
        let id1 = ProjectId::generate();
        let id2 = ProjectId::generate();
        assert_ne!(id1, id2);
        assert!(!id1.as_str().is_empty());
        assert!(!id2.as_str().is_empty());
    }

    #[test]
    fn test_project_id_display() {
        let id = ProjectId::new("my-project");
        let displayed = format!("{}", id);
        assert_eq!(displayed, "my-project");
    }

    #[test]
    fn test_project_id_from_string() {
        let id: ProjectId = String::from("from-string").into();
        assert_eq!(id.as_str(), "from-string");
    }

    #[test]
    fn test_project_kind_default() {
        let kind = ProjectKind::default();
        assert_eq!(kind, ProjectKind::Generic);
    }

    #[test]
    fn test_project_kind_serialize() {
        let kind = ProjectKind::Rust;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"rust\"");
    }

    #[test]
    fn test_project_kind_deserialize() {
        let kind: ProjectKind = serde_json::from_str("\"node_js\"").unwrap();
        assert_eq!(kind, ProjectKind::NodeJs);
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_adhoc() {
        assert_eq!(SandboxPurpose::AdHocTest.billing_tag(), "adhoc");
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_poc() {
        assert_eq!(SandboxPurpose::ProofOfConcept.billing_tag(), "poc");
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_e2e() {
        assert_eq!(SandboxPurpose::E2eTest.billing_tag(), "e2e");
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_realtest() {
        assert_eq!(SandboxPurpose::RealTest.billing_tag(), "realtest");
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_pipeline() {
        assert_eq!(SandboxPurpose::PipelineStage.billing_tag(), "pipeline");
    }

    #[test]
    fn test_sandbox_purpose_billing_tag_job() {
        assert_eq!(SandboxPurpose::Job.billing_tag(), "job");
    }

    #[test]
    fn test_sandbox_purpose_serialize() {
        let purpose = SandboxPurpose::E2eTest;
        let json = serde_json::to_string(&purpose).unwrap();
        assert_eq!(json, "\"e2e_test\"");
    }

    #[test]
    fn test_sandbox_purpose_deserialize() {
        let purpose: SandboxPurpose = serde_json::from_str("\"proof_of_concept\"").unwrap();
        assert_eq!(purpose, SandboxPurpose::ProofOfConcept);
    }

    #[test]
    fn test_pipeline_def_structure() {
        let pipeline = PipelineDef {
            name: "Test Pipeline".to_string(),
            description: "A test pipeline".to_string(),
            stages: vec![PipelineStage {
                name: "build".to_string(),
                image: "rust:1.70".to_string(),
                command: "cargo build".to_string(),
                timeout_ms: 300000,
            }],
        };
        assert_eq!(pipeline.name, "Test Pipeline");
        assert_eq!(pipeline.stages.len(), 1);
        assert_eq!(pipeline.stages[0].name, "build");
    }

    #[test]
    fn test_pipeline_stage_serialize() {
        let stage = PipelineStage {
            name: "test".to_string(),
            image: "debian:bookworm".to_string(),
            command: "cargo test".to_string(),
            timeout_ms: 120000,
        };
        let json = serde_json::to_string(&stage).unwrap();
        let parsed: PipelineStage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.timeout_ms, 120000);
    }
}
