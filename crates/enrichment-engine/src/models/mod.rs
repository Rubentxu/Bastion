//! Core domain models for the enrichment engine.
//!
//! All types are framework-free (no Bastion, no MCP) and serde-serializable.

mod enricher;

pub use enricher::{EnricherDescriptor, ExtractorConfig};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Invocation context for an operation — holds command metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationInvocation {
    /// The primary command string (e.g., "mvn package").
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Working directory for the command.
    pub working_dir: Option<String>,
    /// Environment variables.
    pub env_vars: HashMap<String, String>,
}

impl OperationInvocation {
    /// Construct from a command template string.
    pub fn from_command(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            working_dir: None,
            env_vars: HashMap::new(),
        }
    }
}

/// Result of a completed operation execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationResult {
    /// Exit code of the process.
    pub exit_code: i32,
    /// Standard output as a string.
    pub stdout: String,
    /// Standard error as a string.
    pub stderr: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the operation timed out.
    pub timed_out: bool,
}

/// A single extracted fact from an operation result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    /// The fact key (e.g., "build_status", "jar_artifact").
    pub key: String,
    /// The fact value (e.g., "BUILD SUCCESS").
    pub value: String,
    /// Optional tags for categorization.
    pub tags: Vec<String>,
    /// Name of the extractor that produced this fact.
    pub source_extractor: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f32,
}

/// Summary of test results.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestSummary {
    /// Total tests run.
    pub run: u32,
    /// Number of failures.
    pub failed: u32,
    /// Number of errors.
    pub errors: u32,
    /// Number of skipped tests.
    pub skipped: u32,
}

/// Metadata about the enrichment process itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichmentMeta {
    /// Source identifier (e.g., "enrichment-engine").
    pub source: String,
    /// ISO8601 timestamp when enrichment was computed.
    pub timestamp: String,
    /// The enricher that was matched for this command.
    pub enricher_id: String,
}

/// Agent context: aggregated facts from the enrichment pipeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentContext {
    /// All extracted facts.
    pub facts: Vec<Fact>,
    /// Parsed build status (e.g., "BUILD SUCCESS").
    pub build_status: Option<String>,
    /// Discovered artifacts.
    pub artifacts: Vec<Fact>,
    /// Parsed test summary, if available.
    pub test_summary: Option<TestSummary>,
    /// Enrichment metadata.
    pub enrichment_meta: EnrichmentMeta,
}
