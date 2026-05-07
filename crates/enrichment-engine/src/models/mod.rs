//! Core domain models for the enrichment engine.
//!
//! All types are framework-free (no Bastion, no MCP) and serde-serializable.

mod enricher;
mod rule;
mod run_record;
mod utility_metrics;

pub use enricher::{CommandExtractorPolicy, EnricherDescriptor, ExtractorConfig};
pub use rule::{RuleAction, RuleConfig, RuleOutput};
pub use run_record::EnrichmentRunRecord;
pub use utility_metrics::UtilityMetrics;

use regex::Regex;
use std::sync::Arc;

/// A pre-compiled regex pattern with its metadata.
/// Created once at catalog load time and reused across all pipeline requests.
#[derive(Clone)]
pub struct ValidatedPattern {
    /// The pre-compiled regex.
    pub regex: Arc<Regex>,
    /// The original pattern string (for debugging/logging).
    pub pattern_str: String,
    /// The fact key to emit.
    pub fact_key: String,
    /// The extractor ID.
    pub extractor_id: String,
    /// Merge mode: "single" or "multi".
    pub merge_mode: String,
}

impl ValidatedPattern {
    /// Create a new ValidatedPattern from a pattern string.
    /// Returns Err with message if the pattern is invalid.
    pub fn new(extractor_id: &str, pattern: &str, fact_key: &str, merge_mode: &str) -> Result<Self, String> {
        let regex = Regex::new(pattern).map_err(|e| e.to_string())?;
        Ok(Self {
            regex: Arc::new(regex),
            pattern_str: pattern.to_string(),
            fact_key: fact_key.to_string(),
            extractor_id: extractor_id.to_string(),
            merge_mode: merge_mode.to_string(),
        })
    }
}

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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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
    /// Verdict set by rule evaluation (last-wins across rules).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Recommendations from rule evaluation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendations: Option<Vec<String>>,
}

/// Retention policy configuration for enrichment run records.
///
/// Controls time-based and row-count-based cleanup of the run recorder database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Maximum age of records in days. Records older than this are deleted.
    pub max_age_days: u32,
    /// Maximum number of rows to retain. Oldest rows are deleted when exceeded.
    pub max_rows: u64,
    /// Whether cleanup is enabled. When false, cleanup() returns immediately.
    pub enabled: bool,
    /// Whether sanitization is enabled. When true, commands are sanitized before persistence.
    pub sanitize: bool,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            max_age_days: 90,
            max_rows: 100_000,
            enabled: true,
            sanitize: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verdict and recommendations are omitted from JSON when None (backward-compatible).
    #[test]
    fn agent_context_backward_compatible_serialization() {
        let ctx = AgentContext {
            facts: vec![],
            build_status: Some("BUILD SUCCESS".to_string()),
            artifacts: vec![],
            test_summary: None,
            enrichment_meta: EnrichmentMeta {
                source: "test".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                enricher_id: "maven".to_string(),
            },
            verdict: None,
            recommendations: None,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        // Must NOT contain "verdict" or "recommendations" keys
        assert!(!json.contains("\"verdict\""));
        assert!(!json.contains("\"recommendations\""));
    }

    /// Verdict and recommendations ARE present when set.
    #[test]
    fn agent_context_with_verdict_and_recommendations() {
        let ctx = AgentContext {
            facts: vec![],
            build_status: Some("BUILD SUCCESS".to_string()),
            artifacts: vec![],
            test_summary: None,
            enrichment_meta: EnrichmentMeta {
                source: "test".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                enricher_id: "maven".to_string(),
            },
            verdict: Some("PASSED".to_string()),
            recommendations: Some(vec!["Review tests".to_string()]),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"verdict\":\"PASSED\""));
        assert!(json.contains("\"recommendations\""));
    }
}
