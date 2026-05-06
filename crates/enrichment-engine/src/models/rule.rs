//! Rule models for the enrichment engine rule engine.
//!
//! Defines `RuleConfig`, `RuleAction`, and `RuleOutput` with serde serialization.

use serde::{Deserialize, Serialize};

use super::Fact;

/// Configuration for a single rule.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuleConfig {
    /// Unique rule identifier within the enricher.
    pub id: String,
    /// The enricher this rule belongs to.
    pub enricher_id: String,
    /// CEL-lite expression condition.
    pub condition: String,
    /// Lower = higher priority; evaluated ascending.
    pub priority: i32,
    /// Whether the rule is active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Actions to execute when condition is true.
    #[serde(default)]
    pub actions: Vec<RuleAction>,
}

fn default_enabled() -> bool {
    true
}

/// Actions produced by a rule when its condition matches.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "params")]
pub enum RuleAction {
    /// Derive a new fact with confidence.
    DeriveFact {
        key: String,
        value: String,
        confidence: f32,
    },
    /// Set the final verdict (last one wins).
    SetVerdict(String),
    /// Add a recommendation.
    Recommend(String),
}

/// Output from rule evaluation: derived facts, verdict, and recommendations.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RuleOutput {
    /// Facts derived by matched rules.
    #[serde(default)]
    pub derived_facts: Vec<Fact>,
    /// Final verdict if any rule set one.
    pub verdict: Option<String>,
    /// Recommendations accumulated across all matched rules.
    #[serde(default)]
    pub recommendations: Vec<String>,
}

impl RuleOutput {
    /// Create an empty output (no rules matched).
    pub fn empty() -> Self {
        Self {
            derived_facts: Vec::new(),
            verdict: None,
            recommendations: Vec::new(),
        }
    }

    /// Merge another output into this one.
    /// - derived_facts are appended.
    /// - verdict is replaced if the other has one (last-wins).
    /// - recommendations are extended.
    pub fn merge(&mut self, other: RuleOutput) {
        self.derived_facts.extend(other.derived_facts);
        if other.verdict.is_some() {
            self.verdict = other.verdict;
        }
        self.recommendations.extend(other.recommendations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── RuleAction serialization round-trips ─────────────────────────────────

    #[test]
    fn rule_action_derive_fact_roundtrip() {
        let action = RuleAction::DeriveFact {
            key: "build_ok".to_string(),
            value: "true".to_string(),
            confidence: 0.9,
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: RuleAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RuleAction::DeriveFact { key, value, confidence }
            if key == "build_ok" && value == "true" && (confidence - 0.9).abs() < f32::EPSILON));
    }

    #[test]
    fn rule_action_set_verdict_roundtrip() {
        let action = RuleAction::SetVerdict("PASSED".to_string());
        let json = serde_json::to_string(&action).unwrap();
        let parsed: RuleAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RuleAction::SetVerdict(v) if v == "PASSED"));
    }

    #[test]
    fn rule_action_recommend_roundtrip() {
        let action = RuleAction::Recommend("Review failing tests".to_string());
        let json = serde_json::to_string(&action).unwrap();
        let parsed: RuleAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RuleAction::Recommend(v) if v == "Review failing tests"));
    }

    // ─── RuleConfig serialization round-trips ─────────────────────────────────

    #[test]
    fn rule_config_full_roundtrip() {
        let config = RuleConfig {
            id: "build_verdict".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code == 0".to_string(),
            priority: 0,
            enabled: true,
            actions: vec![
                RuleAction::SetVerdict("PASSED".to_string()),
                RuleAction::Recommend("Looks good".to_string()),
            ],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RuleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "build_verdict");
        assert_eq!(parsed.enricher_id, "maven");
        assert_eq!(parsed.condition, "exit_code == 0");
        assert_eq!(parsed.priority, 0);
        assert!(parsed.enabled);
        assert!(matches!(
            parsed.actions.as_slice(),
            [RuleAction::SetVerdict(v), RuleAction::Recommend(r)]
            if v == "PASSED" && r == "Looks good"
        ));
    }

    #[test]
    fn rule_config_minimal_roundtrip() {
        let config = RuleConfig {
            id: "minimal".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code != 0".to_string(),
            priority: 10,
            enabled: false,
            actions: vec![],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RuleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.priority, 10);
        assert!(!parsed.enabled);
        assert!(parsed.actions.is_empty());
    }

    // ─── RuleOutput serialization round-trips ────────────────────────────────

    #[test]
    fn rule_output_full_roundtrip() {
        let output = RuleOutput {
            derived_facts: vec![
                Fact {
                    key: "build_ok".to_string(),
                    value: "true".to_string(),
                    tags: vec![],
                    source_extractor: "rule".to_string(),
                    confidence: 0.9,
                },
            ],
            verdict: Some("PASSED".to_string()),
            recommendations: vec!["Review tests".to_string()],
        };
        let json = serde_json::to_string(&output).unwrap();
        let parsed: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.derived_facts.len(), 1);
        assert_eq!(parsed.derived_facts[0].key, "build_ok");
        assert_eq!(parsed.verdict.as_deref(), Some("PASSED"));
        assert_eq!(parsed.recommendations.as_slice(), ["Review tests"]);
    }

    #[test]
    fn rule_output_empty_roundtrip() {
        let output = RuleOutput {
            derived_facts: vec![],
            verdict: None,
            recommendations: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        let parsed: RuleOutput = serde_json::from_str(&json).unwrap();
        assert!(parsed.derived_facts.is_empty());
        assert!(parsed.verdict.is_none());
        assert!(parsed.recommendations.is_empty());
    }

    // ─── RuleOutput merge ─────────────────────────────────────────────────────

    #[test]
    fn rule_output_merge_last_verdict_wins() {
        let mut out = RuleOutput::empty();
        out.verdict = Some("FIRST".to_string());

        let other = RuleOutput {
            derived_facts: vec![],
            verdict: Some("SECOND".to_string()),
            recommendations: vec![],
        };
        out.merge(other);
        assert_eq!(out.verdict.as_deref(), Some("SECOND"));
    }

    #[test]
    fn rule_output_merge_appends_facts_and_recs() {
        let mut out = RuleOutput {
            derived_facts: vec![Fact {
                key: "a".to_string(),
                value: "1".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 1.0,
            }],
            verdict: None,
            recommendations: vec!["rec1".to_string()],
        };

        let other = RuleOutput {
            derived_facts: vec![Fact {
                key: "b".to_string(),
                value: "2".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 1.0,
            }],
            verdict: None,
            recommendations: vec!["rec2".to_string()],
        };
        out.merge(other);

        assert_eq!(out.derived_facts.len(), 2);
        assert_eq!(out.derived_facts[1].key, "b");
        assert_eq!(out.recommendations.as_slice(), ["rec1", "rec2"]);
    }
}
