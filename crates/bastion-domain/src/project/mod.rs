//! Project bounded context — project-centric dashboard domain types.
//!
//! This module contains the core domain types for project management:
//! - [`ProjectId`] — unique identifier for a project
//! - [`ProjectKind`] — type of project (Rust, NodeJs, Python, Go, Generic)
//! - [`SandboxPurpose`] — purpose of a sandbox within a project
//! - [`PipelineDef`] — pipeline definition with stages
//! - [`Project`] — the aggregate root

mod aggregate;
pub mod types;

pub use aggregate::{Project, ProjectRepository, ProjectSummary};
pub use types::{PipelineDef, PipelineStage, ProjectId, ProjectKind, SandboxPurpose};
