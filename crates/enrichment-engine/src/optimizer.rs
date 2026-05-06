//! Meta-Harness Optimizer for enrichment runs.
//!
//! Scores enricher utility from persisted `EnrichmentRunRecord` entries.
//! Produces read-only reports — no catalog mutation.
//!
//! # Models
//!
//! - [`EnricherScore`] — Per-enricher aggregated metrics and utility score
//! - [`OptimizationRecommendation`] — Action suggestion with confidence
//! - [`OptimizerReport`] — Full analysis output with timestamps
//!
//! # Scoring Algorithm
//!
//! ```text
//! utility_score ∈ [0.0, 1.0]
//! Components (equal weights, sum = 1.0):
//!   artifact_score   = artifact_yield                          (0–1)
//!   diagnostic_score = diagnostic_hit_rate                     (0–1)
//!   latency_score    = 1.0 - min(avg_latency_ms / 5000.0, 1.0) (0–1, lower is better)
//!   accuracy_score   = 1.0 - false_positive_rate               (0–1)
//!
//! utility_score = (artifact_score + diagnostic_score + latency_score + accuracy_score) / 4.0
//!
//! Confidence: runs < 10 → confidence = runs as f64 / 10.0, else confidence = 1.0
//! ```

use serde::{Deserialize, Serialize};

use crate::models::EnrichmentRunRecord;
use crate::traits::EnrichmentError;

/// Aggregated statistics for an enricher over a set of runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AggregateStats {
    /// The enricher catalog ID.
    pub enricher_id: String,
    /// Total number of runs for this enricher.
    pub total_runs: u64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Fraction of runs that produced at least one artifact.
    pub artifact_yield: f64,
    /// Fraction of runs that produced at least one diagnostic fact.
    pub diagnostic_hit_rate: f64,
    /// Fraction of runs that produced zero artifacts.
    pub false_positive_rate: f64,
    /// Number of runs where no rules fired.
    pub never_fired_rules: u64,
    /// Fraction of runs that encountered an error.
    pub error_rate: f64,
}

/// Score for a single enricher.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnricherScore {
    /// The enricher catalog ID.
    pub enricher_id: String,
    /// Total number of runs analyzed.
    pub total_runs: u64,
    /// Fraction of runs producing artifacts (0.0–1.0).
    pub artifact_yield: f64,
    /// Fraction of runs producing diagnostics (0.0–1.0).
    pub diagnostic_hit_rate: f64,
    /// Fraction of runs producing no artifacts (0.0–1.0).
    pub false_positive_rate: f64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Number of runs where no rules fired.
    pub never_fired_rules: u64,
    /// Computed utility score (0.0–1.0).
    pub utility_score: f64,
}

/// Recommended action for an enricher.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RecAction {
    /// Enricher is performing well — continue using as-is.
    Keep,
    /// Enricher shows moderate utility — review configuration.
    Review,
    /// Enricher has low utility — consider deprioritizing in catalog ordering.
    Deprioritize,
    /// Enricher has very low utility — recommend removal from catalog.
    Remove,
}

impl std::fmt::Display for RecAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecAction::Keep => write!(f, "Keep"),
            RecAction::Review => write!(f, "Review"),
            RecAction::Deprioritize => write!(f, "Deprioritize"),
            RecAction::Remove => write!(f, "Remove"),
        }
    }
}

/// Optimization recommendation for a single enricher.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OptimizationRecommendation {
    /// The enricher catalog ID.
    pub enricher_id: String,
    /// Recommended action.
    pub action: RecAction,
    /// Human-readable explanation for the recommendation.
    pub reason: String,
    /// Confidence score (0.0–1.0). Below 1.0 when runs < 10.
    pub confidence: f64,
}

/// Full optimizer report with all scores and recommendations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OptimizerReport {
    /// ISO 8601 timestamp when the report was generated.
    pub generated_at: String,
    /// Per-enricher scores.
    pub scores: Vec<EnricherScore>,
    /// Per-enricher recommendations.
    pub recommendations: Vec<OptimizationRecommendation>,
    /// Total number of runs analyzed across all enrichers.
    pub total_runs_analyzed: u64,
}

impl OptimizerReport {
    /// Serialize the report to JSON with at most 4 decimal places for floats.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Trait for reading enrichment run records for optimization analysis.
///
/// Implementations are responsible for actual data storage (SQLite, etc.).
#[async_trait::async_trait]
pub trait OptimizerRepository: Send + Sync {
    /// Read all run records, optionally filtered to those after a given timestamp.
    async fn read_records(&self, after: Option<&str>) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError>;

    /// Read all records for a specific enricher.
    async fn read_records_by_enricher(
        &self,
        enricher_id: &str,
    ) -> Result<Vec<EnrichmentRunRecord>, EnrichmentError>;

    /// Compute aggregate statistics per enricher.
    async fn compute_statistics(&self) -> Result<Vec<AggregateStats>, EnrichmentError>;
}

// ─── Scoring Functions ─────────────────────────────────────────────────────────

/// Minimum runs before full confidence is assigned.
const MIN_RUNS_FOR_CONFIDENCE: u64 = 10;

/// Maximum latency (ms) for latency score calculation.
const MAX_LATENCY_MS: f64 = 5000.0;

/// Compute `EnricherScore` for a single enricher from its aggregate stats.
pub fn compute_enricher_score(stats: &AggregateStats) -> EnricherScore {
    let artifact_score = stats.artifact_yield;
    let diagnostic_score = stats.diagnostic_hit_rate;
    let latency_score = 1.0 - (stats.avg_latency_ms / MAX_LATENCY_MS).min(1.0);
    let accuracy_score = 1.0 - stats.false_positive_rate;

    // If no runs have been executed, utility is 0 (can't assess quality without data)
    let utility_score = if stats.total_runs == 0 {
        0.0
    } else {
        (artifact_score + diagnostic_score + latency_score + accuracy_score) / 4.0
    };

    EnricherScore {
        enricher_id: stats.enricher_id.clone(),
        total_runs: stats.total_runs,
        artifact_yield: round4(stats.artifact_yield),
        diagnostic_hit_rate: round4(stats.diagnostic_hit_rate),
        false_positive_rate: round4(stats.false_positive_rate),
        avg_latency_ms: round4(stats.avg_latency_ms),
        never_fired_rules: stats.never_fired_rules,
        utility_score: round4(utility_score.clamp(0.0, 1.0)),
    }
}

/// Compute confidence score based on number of runs.
pub fn compute_confidence(total_runs: u64) -> f64 {
    if total_runs >= MIN_RUNS_FOR_CONFIDENCE {
        1.0
    } else {
        round4(total_runs as f64 / MIN_RUNS_FOR_CONFIDENCE as f64)
    }
}

/// Generate an `OptimizationRecommendation` from an `EnricherScore`.
pub fn generate_recommendation(score: &EnricherScore) -> OptimizationRecommendation {
    let confidence = compute_confidence(score.total_runs);
    let (action, reason) = determine_action_and_reason(score, confidence);

    OptimizationRecommendation {
        enricher_id: score.enricher_id.clone(),
        action,
        reason,
        confidence,
    }
}

/// Determine the recommended action and reason based on score and confidence.
fn determine_action_and_reason(score: &EnricherScore, confidence: f64) -> (RecAction, String) {
    let utility = score.utility_score;
    let runs = score.total_runs;

    // Low confidence — always review
    if confidence < 0.8 {
        let reason = if runs == 0 {
            "No runs recorded for this enricher.".to_string()
        } else {
            format!(
                "Insufficient data ({} runs). Need {} for full confidence.",
                runs, MIN_RUNS_FOR_CONFIDENCE
            )
        };
        return (RecAction::Review, reason);
    }

    // High confidence thresholds
    if utility < 0.2 {
        (RecAction::Remove, format!("Very low utility score ({:.2}). Recommend removal.", utility))
    } else if utility < 0.4 {
        (
            RecAction::Deprioritize,
            format!("Low utility score ({:.2}). Consider deprioritizing in catalog.", utility),
        )
    } else if utility > 0.7 {
        (RecAction::Keep, format!("High utility score ({:.2}). Keep in catalog.", utility))
    } else {
        (
            RecAction::Review,
            format!("Moderate utility score ({:.2}). Review configuration.", utility),
        )
    }
}

/// Compute aggregate statistics from a collection of run records for one enricher.
pub fn compute_aggregate_stats(enricher_id: &str, records: &[EnrichmentRunRecord]) -> AggregateStats {
    if records.is_empty() {
        return AggregateStats {
            enricher_id: enricher_id.to_string(),
            total_runs: 0,
            avg_latency_ms: 0.0,
            artifact_yield: 0.0,
            diagnostic_hit_rate: 0.0,
            false_positive_rate: 0.0,
            never_fired_rules: 0,
            error_rate: 0.0,
        };
    }

    let total_runs = records.len() as u64;

    // Average latency
    let total_latency: u64 = records.iter().map(|r| r.duration_ms).sum();
    let avg_latency_ms = total_latency as f64 / total_runs as f64;

    // Artifact yield (runs with artifact_count > 0)
    let runs_with_artifacts = records.iter().filter(|r| r.artifact_count > 0).count() as u64;
    let artifact_yield = runs_with_artifacts as f64 / total_runs as f64;

    // Diagnostic hit rate (runs with diagnostics_count > 0)
    let runs_with_diagnostics = records.iter().filter(|r| r.diagnostics_count > 0).count() as u64;
    let diagnostic_hit_rate = runs_with_diagnostics as f64 / total_runs as f64;

    // False positive rate (runs with no artifacts)
    let runs_with_no_artifacts = records.iter().filter(|r| r.artifact_count == 0).count() as u64;
    let false_positive_rate = runs_with_no_artifacts as f64 / total_runs as f64;

    // Never fired rules (runs with rule_hits_count == 0)
    let never_fired_rules = records.iter().filter(|r| r.rule_hits_count == 0).count() as u64;

    // Error rate (runs with error != None)
    let runs_with_error = records.iter().filter(|r| r.error.is_some()).count() as u64;
    let error_rate = runs_with_error as f64 / total_runs as f64;

    AggregateStats {
        enricher_id: enricher_id.to_string(),
        total_runs,
        avg_latency_ms: round4(avg_latency_ms),
        artifact_yield: round4(artifact_yield),
        diagnostic_hit_rate: round4(diagnostic_hit_rate),
        false_positive_rate: round4(false_positive_rate),
        never_fired_rules,
        error_rate: round4(error_rate),
    }
}

/// Group records by enricher_id and compute aggregate stats.
pub fn compute_all_stats(
    records: &[EnrichmentRunRecord],
) -> Vec<AggregateStats> {
    use std::collections::HashMap;

    let mut by_enricher: HashMap<String, Vec<EnrichmentRunRecord>> = HashMap::new();
    for record in records {
        by_enricher
            .entry(record.enricher_id.clone())
            .or_default()
            .push(record.clone());
    }

    by_enricher
        .into_iter()
        .map(|(enricher_id, recs)| compute_aggregate_stats(&enricher_id, &recs))
        .collect()
}

/// Compute all enricher scores from aggregate stats.
pub fn compute_scores(stats: &[AggregateStats]) -> Vec<EnricherScore> {
    stats.iter().map(compute_enricher_score).collect()
}

/// Generate all recommendations from scores.
pub fn generate_recommendations(scores: &[EnricherScore]) -> Vec<OptimizationRecommendation> {
    scores.iter().map(generate_recommendation).collect()
}

/// Generate a full optimizer report from run records.
pub fn generate_report(records: &[EnrichmentRunRecord]) -> OptimizerReport {
    let stats = compute_all_stats(records);
    let scores = compute_scores(&stats);
    let recommendations = generate_recommendations(&scores);
    let total_runs = records.len() as u64;

    OptimizerReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        scores,
        recommendations,
        total_runs_analyzed: total_runs,
    }
}

/// Round a float to at most 4 decimal places.
fn round4(val: f64) -> f64 {
    (val * 10_000.0).round() / 10_000.0
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EnrichmentRunRecord;

    fn make_record(
        enricher_id: &str,
        artifact_count: u32,
        diagnostics_count: u32,
        rule_hits_count: u32,
        duration_ms: u64,
        error: Option<String>,
    ) -> EnrichmentRunRecord {
        EnrichmentRunRecord::new(
            uuid::Uuid::new_v4().to_string(),
            "2024-01-01T00:00:00Z".to_string(),
            "test command".to_string(),
            enricher_id.to_string(),
            0,
            duration_ms,
            None,
            None,
            5,
            2,
            rule_hits_count,
            diagnostics_count,
            artifact_count,
            0.85,
            None,
            0,
            error,
        )
    }

    // ─── compute_aggregate_stats ─────────────────────────────────────────────────

    #[test]
    fn compute_aggregate_stats_empty() {
        let stats = compute_aggregate_stats("maven", &[]);
        assert_eq!(stats.total_runs, 0);
        assert_eq!(stats.artifact_yield, 0.0);
    }

    #[test]
    fn compute_aggregate_stats_single_run() {
        let record = make_record("maven", 2, 1, 3, 5000, None);
        let stats = compute_aggregate_stats("maven", &[record]);
        assert_eq!(stats.total_runs, 1);
        assert_eq!(stats.artifact_yield, 1.0);
        assert_eq!(stats.diagnostic_hit_rate, 1.0);
        assert!((stats.avg_latency_ms - 5000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_aggregate_stats_partial_artifact_yield() {
        // 2 out of 4 runs have artifacts
        let records = vec![
            make_record("maven", 1, 0, 0, 1000, None),
            make_record("maven", 0, 0, 0, 1000, None),
            make_record("maven", 1, 0, 0, 1000, None),
            make_record("maven", 0, 0, 0, 1000, None),
        ];
        let stats = compute_aggregate_stats("maven", &records);
        assert_eq!(stats.total_runs, 4);
        assert!((stats.artifact_yield - 0.5).abs() < f64::EPSILON);
    }

    // ─── compute_enricher_score ─────────────────────────────────────────────────

    #[test]
    fn compute_enricher_score_high_utility() {
        let stats = AggregateStats {
            enricher_id: "maven".to_string(),
            total_runs: 20,
            avg_latency_ms: 1000.0,
            artifact_yield: 0.9,
            diagnostic_hit_rate: 0.8,
            false_positive_rate: 0.1,
            never_fired_rules: 2,
            error_rate: 0.05,
        };
        let score = compute_enricher_score(&stats);

        assert!((score.utility_score - 0.85).abs() < 0.01); // (0.9 + 0.8 + 0.8 + 0.9) / 4
        assert_eq!(score.total_runs, 20);
    }

    #[test]
    fn compute_enricher_score_zero_runs() {
        let stats = AggregateStats {
            enricher_id: "empty".to_string(),
            total_runs: 0,
            avg_latency_ms: 0.0,
            artifact_yield: 0.0,
            diagnostic_hit_rate: 0.0,
            false_positive_rate: 0.0,
            never_fired_rules: 0,
            error_rate: 0.0,
        };
        let score = compute_enricher_score(&stats);
        assert_eq!(score.utility_score, 0.0);
    }

    // ─── compute_confidence ─────────────────────────────────────────────────────

    #[test]
    fn compute_confidence_zero_runs() {
        assert!((compute_confidence(0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_confidence_cold_start() {
        // 5 runs < 10 → confidence = 0.5
        assert!((compute_confidence(5) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_confidence_full() {
        // 10 runs → confidence = 1.0
        assert!((compute_confidence(10) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_confidence_beyond_full() {
        // 15 runs → confidence = 1.0
        assert!((compute_confidence(15) - 1.0).abs() < f64::EPSILON);
    }

    // ─── generate_recommendation ────────────────────────────────────────────────

    #[test]
    fn generate_recommendation_zero_runs() {
        let score = EnricherScore {
            enricher_id: "empty".to_string(),
            total_runs: 0,
            artifact_yield: 0.0,
            diagnostic_hit_rate: 0.0,
            false_positive_rate: 0.0,
            avg_latency_ms: 0.0,
            never_fired_rules: 0,
            utility_score: 0.0,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Review);
        assert!((rec.confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn generate_recommendation_low_confidence() {
        let score = EnricherScore {
            enricher_id: "cold".to_string(),
            total_runs: 5,
            artifact_yield: 0.5,
            diagnostic_hit_rate: 0.5,
            false_positive_rate: 0.5,
            avg_latency_ms: 1000.0,
            never_fired_rules: 1,
            utility_score: 0.5,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Review);
        assert!((rec.confidence - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn generate_recommendation_remove() {
        let score = EnricherScore {
            enricher_id: "bad".to_string(),
            total_runs: 20,
            artifact_yield: 0.05,
            diagnostic_hit_rate: 0.1,
            false_positive_rate: 0.95,
            avg_latency_ms: 5000.0,
            never_fired_rules: 15,
            utility_score: 0.1,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Remove);
    }

    #[test]
    fn generate_recommendation_deprioritize() {
        let score = EnricherScore {
            enricher_id: "mediocre".to_string(),
            total_runs: 20,
            artifact_yield: 0.3,
            diagnostic_hit_rate: 0.3,
            false_positive_rate: 0.7,
            avg_latency_ms: 2000.0,
            never_fired_rules: 5,
            utility_score: 0.3,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Deprioritize);
    }

    #[test]
    fn generate_recommendation_keep() {
        let score = EnricherScore {
            enricher_id: "great".to_string(),
            total_runs: 20,
            artifact_yield: 0.9,
            diagnostic_hit_rate: 0.85,
            false_positive_rate: 0.1,
            avg_latency_ms: 500.0,
            never_fired_rules: 1,
            utility_score: 0.8,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Keep);
    }

    #[test]
    fn generate_recommendation_review_moderate() {
        let score = EnricherScore {
            enricher_id: "ok".to_string(),
            total_runs: 20,
            artifact_yield: 0.5,
            diagnostic_hit_rate: 0.5,
            false_positive_rate: 0.5,
            avg_latency_ms: 2000.0,
            never_fired_rules: 3,
            utility_score: 0.5,
        };
        let rec = generate_recommendation(&score);
        assert_eq!(rec.action, RecAction::Review);
    }

    // ─── generate_report ────────────────────────────────────────────────────────

    #[test]
    fn generate_report_empty() {
        let report = generate_report(&[]);
        assert_eq!(report.total_runs_analyzed, 0);
        assert!(report.scores.is_empty());
        assert!(report.recommendations.is_empty());
    }

    #[test]
    fn generate_report_multiple_enrichers() {
        let records = vec![
            make_record("maven", 2, 1, 3, 1000, None),
            make_record("maven", 1, 0, 2, 1000, None),
            make_record("gradle", 3, 2, 4, 2000, None),
        ];
        let report = generate_report(&records);
        assert_eq!(report.total_runs_analyzed, 3);
        assert_eq!(report.scores.len(), 2); // maven and gradle
    }

    #[test]
    fn report_serialization() {
        let report = generate_report(&[]);
        let json = report.to_json().unwrap();
        assert!(json.contains("\"generated_at\""));
        assert!(json.contains("\"scores\""));
        assert!(json.contains("\"recommendations\""));
    }
}