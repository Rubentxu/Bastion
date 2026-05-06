//! Utility metrics computed from an enrichment run record.
//!
//! Provides a pure function `from_run()` to derive utility metrics from a
//! persisted `EnrichmentRunRecord`, suitable for Meta-Harness optimization.

use serde::{Deserialize, Serialize};

use super::run_record::EnrichmentRunRecord;

/// Utility metrics derived from an enrichment run record.
///
/// These metrics are used by the Meta-Harness to score and compare
/// different enrichment strategies.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UtilityMetrics {
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
    pub aggregate_confidence: f64,
    /// Whether a verdict was set by rule evaluation.
    pub verdict_present: bool,
    /// Number of recommendations produced.
    pub recommendation_count: u32,
    /// Duration of the command in milliseconds.
    pub elapsed_ms: u64,
    /// Whether the enrichment pipeline reported an error.
    pub errored: bool,
}

impl UtilityMetrics {
    /// Compute utility metrics from an enrichment run record.
    ///
    /// This is a pure function — no side effects, no async, no external dependencies.
    ///
    /// # Arguments
    ///
    /// * `record` - Reference to the persisted enrichment run record
    ///
    /// # Returns
    ///
    /// A `UtilityMetrics` instance with all fields derived from the record.
    ///
    /// # Examples
    ///
    /// ```
    /// use enrichment_engine::models::{EnrichmentRunRecord, UtilityMetrics};
    ///
    /// let record = EnrichmentRunRecord::new(
    ///     "id".to_string(),
    ///     "2024-01-01T00:00:00Z".to_string(),
    ///     "mvn package".to_string(),
    ///     "maven".to_string(),
    ///     0, 5000,
    ///     Some("BUILD SUCCESS".to_string()),
    ///     None,
    ///     8, 2, 3, 1, 2, 0.85,
    ///     Some("PASSED".to_string()),
    ///     1,
    ///     None,
    /// );
    ///
    /// let metrics = UtilityMetrics::from_run(&record);
    /// assert_eq!(metrics.facts_count, 8);
    /// assert!(metrics.verdict_present);
    /// assert!(!metrics.errored);
    /// ```
    pub fn from_run(record: &EnrichmentRunRecord) -> Self {
        Self {
            facts_count: record.facts_count,
            derived_facts_count: record.derived_facts_count,
            rule_hits_count: record.rule_hits_count,
            diagnostics_count: record.diagnostics_count,
            artifact_count: record.artifact_count,
            aggregate_confidence: record.confidence_avg,
            verdict_present: record.verdict.is_some(),
            recommendation_count: record.recommendation_count,
            elapsed_ms: record.duration_ms,
            errored: record.error.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EnrichmentRunRecord;

    #[test]
    fn from_run_healthy_run() {
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440000".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "mvn package".to_string(),
            "maven".to_string(),
            0,
            5000,
            Some("BUILD SUCCESS".to_string()),
            None,
            8,
            2,
            3,
            1,
            2,
            0.85,
            Some("PASSED".to_string()),
            1,
            None,
        );

        let metrics = UtilityMetrics::from_run(&record);

        assert_eq!(metrics.facts_count, 8);
        assert_eq!(metrics.derived_facts_count, 2);
        assert_eq!(metrics.rule_hits_count, 3);
        assert_eq!(metrics.diagnostics_count, 1);
        assert_eq!(metrics.artifact_count, 2);
        assert!((metrics.aggregate_confidence - 0.85).abs() < f64::EPSILON);
        assert!(metrics.verdict_present);
        assert_eq!(metrics.recommendation_count, 1);
        assert_eq!(metrics.elapsed_ms, 5000);
        assert!(!metrics.errored);
    }

    #[test]
    fn from_run_errored_run() {
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

        let metrics = UtilityMetrics::from_run(&record);

        assert_eq!(metrics.facts_count, 0);
        assert!(!metrics.verdict_present);
        assert!(metrics.errored);
        assert_eq!(metrics.elapsed_ms, 1000);
        assert!((metrics.aggregate_confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn from_run_zero_facts_healthy() {
        // A run with zero facts but no error should still be healthy
        let record = EnrichmentRunRecord::new(
            "550e8400-e29b-41d4-a716-446655440002".to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "echo hello".to_string(),
            "".to_string(),
            0,
            100,
            Some("hello".to_string()),
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

        let metrics = UtilityMetrics::from_run(&record);

        assert_eq!(metrics.facts_count, 0);
        assert!(!metrics.verdict_present);
        assert!(!metrics.errored);
        assert_eq!(metrics.elapsed_ms, 100);
    }

    #[test]
    fn utility_metrics_default_is_all_zeroes_error_false() {
        let metrics = UtilityMetrics::default();

        assert_eq!(metrics.facts_count, 0);
        assert_eq!(metrics.derived_facts_count, 0);
        assert_eq!(metrics.rule_hits_count, 0);
        assert_eq!(metrics.diagnostics_count, 0);
        assert_eq!(metrics.artifact_count, 0);
        assert!((metrics.aggregate_confidence - 0.0).abs() < f64::EPSILON);
        assert!(!metrics.verdict_present);
        assert_eq!(metrics.recommendation_count, 0);
        assert_eq!(metrics.elapsed_ms, 0);
        assert!(!metrics.errored);
    }

    #[test]
    fn serde_roundtrip_utility_metrics() {
        let metrics = UtilityMetrics {
            facts_count: 10,
            derived_facts_count: 3,
            rule_hits_count: 5,
            diagnostics_count: 2,
            artifact_count: 4,
            aggregate_confidence: 0.92,
            verdict_present: true,
            recommendation_count: 2,
            elapsed_ms: 8000,
            errored: false,
        };

        let json = serde_json::to_string(&metrics).unwrap();
        let parsed: UtilityMetrics = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.facts_count, 10);
        assert_eq!(parsed.derived_facts_count, 3);
        assert_eq!(parsed.rule_hits_count, 5);
        assert_eq!(parsed.diagnostics_count, 2);
        assert_eq!(parsed.artifact_count, 4);
        assert!((parsed.aggregate_confidence - 0.92).abs() < f64::EPSILON);
        assert!(parsed.verdict_present);
        assert_eq!(parsed.recommendation_count, 2);
        assert_eq!(parsed.elapsed_ms, 8000);
        assert!(!parsed.errored);
    }
}
