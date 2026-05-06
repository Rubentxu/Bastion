//! Enrichment run record model for persisting harness telemetry.
//!
//! Stores truncated command/output summaries and fact/rule counts from each
//! enrichment pipeline run, enabling future Meta-Harness optimization.

use serde::{Deserialize, Serialize};

/// A single enrichment pipeline run record.
///
/// All string fields (`command`, `output_summary_stdout`, `output_summary_stderr`)
/// are truncated at 500 chars. Empty output is stored as `None`, not empty string.
///
/// # Privacy
///
/// Raw unbounded stdout/stderr are NEVER stored. Only truncated summaries are persisted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichmentRunRecord {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// ISO 8601 timestamp when the run was recorded.
    pub timestamp: String,
    /// Truncated command string (≤500 chars).
    pub command: String,
    /// The enricher catalog ID that matched this command.
    pub enricher_id: String,
    /// Exit code from the command execution.
    pub exit_code: i32,
    /// Duration of the command in milliseconds.
    pub duration_ms: u64,
    /// Truncated stdout summary (≤500 chars), or `None` if empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary_stdout: Option<String>,
    /// Truncated stderr summary (≤500 chars), or `None` if empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary_stderr: Option<String>,
    /// Number of facts extracted by extractors.
    pub facts_count: u32,
    /// Number of facts derived by rule evaluation.
    pub derived_facts_count: u32,
    /// Number of rules that matched (condition evaluated to true).
    pub rule_hits_count: u32,
    /// Number of facts tagged with `diagnostic`.
    pub diagnostics_count: u32,
    /// Number of artifact facts discovered.
    pub artifact_count: u32,
    /// Average confidence score across all facts (0.0–1.0).
    pub confidence_avg: f64,
    /// Verdict set by rule evaluation, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Number of recommendations produced by rule evaluation.
    pub recommendation_count: u32,
    /// Error message if the pipeline failed, or `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl EnrichmentRunRecord {
    /// Construct a record from an enrichment run.
    ///
    /// # Arguments
    ///
    /// * `id` - UUID v4 string for this record
    /// * `timestamp` - ISO 8601 timestamp
    /// * `command` - Truncated command string
    /// * `enricher_id` - The matched enricher catalog ID
    /// * `exit_code` - Command exit code
    /// * `duration_ms` - Command duration in ms
    /// * `stdout` - Truncated stdout summary (None if empty)
    /// * `stderr` - Truncated stderr summary (None if empty)
    /// * `facts_count` - Number of extracted facts
    /// * `derived_facts_count` - Number of rule-derived facts
    /// * `rule_hits_count` - Number of rules that matched
    /// * `diagnostics_count` - Number of diagnostic-tagged facts
    /// * `artifact_count` - Number of artifact facts
    /// * `confidence_avg` - Average confidence score
    /// * `verdict` - Verdict from rule evaluation, if set
    /// * `recommendation_count` - Number of recommendations
    /// * `error` - Error message if pipeline failed
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        timestamp: String,
        command: String,
        enricher_id: String,
        exit_code: i32,
        duration_ms: u64,
        output_summary_stdout: Option<String>,
        output_summary_stderr: Option<String>,
        facts_count: u32,
        derived_facts_count: u32,
        rule_hits_count: u32,
        diagnostics_count: u32,
        artifact_count: u32,
        confidence_avg: f64,
        verdict: Option<String>,
        recommendation_count: u32,
        error: Option<String>,
    ) -> Self {
        Self {
            id,
            timestamp,
            command,
            enricher_id,
            exit_code,
            duration_ms,
            output_summary_stdout,
            output_summary_stderr,
            facts_count,
            derived_facts_count,
            rule_hits_count,
            diagnostics_count,
            artifact_count,
            confidence_avg,
            verdict,
            recommendation_count,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_full_record() {
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            5,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        let json = serde_json::to_string(&record).unwrap();
        let parsed: EnrichmentRunRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(parsed.enricher_id, "maven");
        assert_eq!(parsed.facts_count, 5);
        assert_eq!(parsed.derived_facts_count, 2);
        assert_eq!(parsed.rule_hits_count, 3);
        assert_eq!(parsed.diagnostics_count, 1);
        assert_eq!(parsed.artifact_count, 2);
        assert!((parsed.confidence_avg - 0.85).abs() < f64::EPSILON);
        assert_eq!(parsed.verdict.as_deref(), Some("PASSED"));
        assert_eq!(parsed.recommendation_count, 1);
        assert!(parsed.error.is_none());
        assert_eq!(parsed.output_summary_stdout.as_deref(), Some("BUILD SUCCESS"));
        assert!(parsed.output_summary_stderr.is_none());
    }

    #[test]
    fn serde_roundtrip_record_with_error() {
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440001".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            1,
            1000,
            None,
            Some("COMPILATION ERROR".to_string()),
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            Some("extraction failed: pattern not found".to_string()),
        );

        let json = serde_json::to_string(&record).unwrap();
        let parsed: EnrichmentRunRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.exit_code, 1);
        assert!(parsed.error.is_some());
        assert_eq!(parsed.error.as_deref(), Some("extraction failed: pattern not found"));
        assert_eq!(parsed.output_summary_stderr.as_deref(), Some("COMPILATION ERROR"));
        assert!(parsed.verdict.is_none());
    }

    #[test]
    fn serde_roundtrip_record_with_all_none_options() {
        // Test that Option fields properly round-trip as null
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440002".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "echo hello".to_string(),
            "".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        let json = serde_json::to_string(&record).unwrap();
        let parsed: EnrichmentRunRecord = serde_json::from_str(&json).unwrap();

        // Options should be None (null in JSON)
        assert!(parsed.output_summary_stdout.is_none());
        assert!(parsed.output_summary_stderr.is_none());
        assert!(parsed.verdict.is_none());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn serde_skips_none_options_in_json() {
        // Verify that skip_serializing_if works correctly
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440003".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "echo hello".to_string(),
            "".to_string(),
            0,
            100,
            None,
            None,
            0,
            0,
            0,
            0,
            0,
            0.0,
            None,
            0,
            None,
        );

        let json = serde_json::to_string(&record).unwrap();

        // None options should NOT appear in JSON
        assert!(!json.contains("output_summary_stdout"));
        assert!(!json.contains("output_summary_stderr"));
        assert!(!json.contains("\"verdict\""));
        assert!(!json.contains("\"error\""));
    }
}
