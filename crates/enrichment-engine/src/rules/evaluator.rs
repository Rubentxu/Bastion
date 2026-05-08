//! Rule evaluation engine.
//!
//! Defines the `RuleEvaluator` trait and `DefaultRuleEvaluator` implementation.

use std::sync::Arc;

use async_trait::async_trait;

use crate::models::{
    Fact, OperationInvocation, OperationResult, RuleAction, RuleConfig, RuleOutput,
};

use super::ast::{EvalContext, Expr, Parser};

/// Result of parsing a rule condition.
#[derive(Debug)]
struct ParsedRule {
    config: RuleConfig,
    expr: Expr,
}

/// Trait for rule evaluators.
///
/// The evaluator inspects an operation invocation, result, and extracted facts,
/// then produces derived facts, a verdict, and recommendations.
#[async_trait]
pub trait RuleEvaluator: Send + Sync {
    /// Evaluate all rules for the given enricher.
    async fn evaluate(
        &self,
        enricher_id: &str,
        invocation: &OperationInvocation,
        result: &OperationResult,
        facts: &[Fact],
    ) -> RuleOutput;
}

/// In-memory rule repository for testing.
pub struct InMemoryRuleRepository {
    rules: Vec<RuleConfig>,
}

impl InMemoryRuleRepository {
    pub fn new(rules: Vec<RuleConfig>) -> Self {
        Self { rules }
    }
}

#[async_trait]
impl crate::traits::RuleRepository for InMemoryRuleRepository {
    async fn find_rules(&self, enricher_id: &str) -> Vec<RuleConfig> {
        self.rules
            .iter()
            .filter(|r| r.enricher_id == enricher_id && r.enabled)
            .cloned()
            .collect()
    }

    async fn list_all_rules(&self) -> Vec<RuleConfig> {
        self.rules.clone()
    }
}

/// Default rule evaluator using a `RuleRepository`.
pub struct DefaultRuleEvaluator {
    repo: Arc<dyn crate::traits::RuleRepository>,
}

impl DefaultRuleEvaluator {
    pub fn new(repo: Arc<dyn crate::traits::RuleRepository>) -> Self {
        Self { repo }
    }

    /// Parse and cache rules for an enricher.
    async fn get_parsed_rules(&self, enricher_id: &str) -> Vec<ParsedRule> {
        let configs = self.repo.find_rules(enricher_id).await;
        configs
            .into_iter()
            .filter_map(|config| match Parser::parse(&config.condition) {
                Ok(expr) => Some(ParsedRule { config, expr }),
                Err(e) => {
                    tracing::warn!(
                        rule_id = %config.id,
                        condition = %config.condition,
                        error = %e,
                        "Skipping rule with invalid condition"
                    );
                    None
                }
            })
            .collect()
    }

    fn evaluate_rules(&self, parsed_rules: Vec<ParsedRule>, ctx: &EvalContext<'_>) -> RuleOutput {
        let mut output = RuleOutput::empty();

        // Sort by priority ascending (0 = highest)
        let mut sorted: Vec<_> = parsed_rules;
        sorted.sort_by_key(|r| r.config.priority);

        for pr in sorted {
            if ctx.evaluate(&pr.expr) {
                output.hit_count += 1;
                for action in &pr.config.actions {
                    self.execute_action(action, &mut output);
                }
            }
        }

        output
    }

    fn execute_action(&self, action: &RuleAction, output: &mut RuleOutput) {
        match action {
            RuleAction::DeriveFact {
                key,
                value,
                confidence,
            } => {
                output.derived_facts.push(Fact {
                    key: key.clone(),
                    value: value.clone(),
                    tags: vec![],
                    source_extractor: "rule".to_string(),
                    confidence: *confidence,
                });
            }
            RuleAction::SetVerdict(verdict) => {
                output.verdict = Some(verdict.clone());
            }
            RuleAction::Recommend(rec) => {
                output.recommendations.push(rec.clone());
            }
        }
    }
}

#[async_trait]
impl RuleEvaluator for DefaultRuleEvaluator {
    async fn evaluate(
        &self,
        enricher_id: &str,
        invocation: &OperationInvocation,
        result: &OperationResult,
        facts: &[Fact],
    ) -> RuleOutput {
        let parsed_rules = self.get_parsed_rules(enricher_id).await;
        let ctx = EvalContext::new(invocation, result, facts);
        self.evaluate_rules(parsed_rules, &ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Fact, OperationInvocation, OperationResult, RuleAction, RuleConfig};
    use async_trait::async_trait;

    fn make_result(exit_code: i32, timed_out: bool, stdout: &str) -> OperationResult {
        OperationResult {
            exit_code,
            stdout: stdout.to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out,
        }
    }

    fn make_invocation() -> OperationInvocation {
        OperationInvocation::from_command("mvn package")
    }

    fn fact(key: &str, value: &str) -> Fact {
        Fact {
            key: key.to_string(),
            value: value.to_string(),
            tags: vec![],
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }
    }

    // ─── Mock repository helpers ───────────────────────────────────────────────

    struct MockRepo {
        rules: Vec<RuleConfig>,
    }

    #[async_trait]
    impl crate::traits::RuleRepository for MockRepo {
        async fn find_rules(&self, enricher_id: &str) -> Vec<RuleConfig> {
            self.rules
                .iter()
                .filter(|r| r.enricher_id == enricher_id && r.enabled)
                .cloned()
                .collect()
        }

        async fn list_all_rules(&self) -> Vec<RuleConfig> {
            self.rules.clone()
        }
    }

    // ─── Integration tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn single_rule_match_sets_verdict() {
        let rules = vec![RuleConfig {
            id: "build_ok".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code == 0".to_string(),
            priority: 0,
            enabled: true,
            actions: vec![RuleAction::SetVerdict("PASSED".to_string())],
        }];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        let output = evaluator
            .evaluate("maven", &make_invocation(), &make_result(0, false, ""), &[])
            .await;

        assert_eq!(output.verdict.as_deref(), Some("PASSED"));
        assert!(output.derived_facts.is_empty());
        assert!(output.recommendations.is_empty());
    }

    #[tokio::test]
    async fn multi_rule_priority_ordering() {
        // R0 (priority 0) matches timed_out → verdict TIMEOUT
        // R1 (priority 1) would match exit_code != 0 but shouldn't run since R0 matched
        let rules = vec![
            RuleConfig {
                id: "timeout".to_string(),
                enricher_id: "maven".to_string(),
                condition: "timed_out".to_string(),
                priority: 0,
                enabled: true,
                actions: vec![RuleAction::SetVerdict("TIMEOUT".to_string())],
            },
            RuleConfig {
                id: "fail".to_string(),
                enricher_id: "maven".to_string(),
                condition: "exit_code != 0".to_string(),
                priority: 1,
                enabled: true,
                actions: vec![RuleAction::SetVerdict("FAILED".to_string())],
            },
        ];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        // timed_out=true, exit_code=0
        let output = evaluator
            .evaluate("maven", &make_invocation(), &make_result(0, true, ""), &[])
            .await;

        assert_eq!(output.verdict.as_deref(), Some("TIMEOUT"));
    }

    #[tokio::test]
    async fn false_condition_produces_empty_output() {
        let rules = vec![RuleConfig {
            id: "never".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code != 0".to_string(),
            priority: 0,
            enabled: true,
            actions: vec![RuleAction::SetVerdict("PASSED".to_string())],
        }];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        let output = evaluator
            .evaluate("maven", &make_invocation(), &make_result(0, false, ""), &[])
            .await;

        assert!(output.verdict.is_none());
    }

    #[tokio::test]
    async fn disabled_rule_skipped() {
        let rules = vec![RuleConfig {
            id: "never".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code == 0".to_string(),
            priority: 0,
            enabled: false, // DISABLED
            actions: vec![RuleAction::SetVerdict("PASSED".to_string())],
        }];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        let output = evaluator
            .evaluate("maven", &make_invocation(), &make_result(0, false, ""), &[])
            .await;

        assert!(output.verdict.is_none());
    }

    #[tokio::test]
    async fn derive_fact_action() {
        let rules = vec![RuleConfig {
            id: "derive".to_string(),
            enricher_id: "maven".to_string(),
            condition: "exit_code == 0".to_string(),
            priority: 0,
            enabled: true,
            actions: vec![RuleAction::DeriveFact {
                key: "build_ok".to_string(),
                value: "true".to_string(),
                confidence: 0.9,
            }],
        }];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        let output = evaluator
            .evaluate("maven", &make_invocation(), &make_result(0, false, ""), &[])
            .await;

        assert_eq!(output.derived_facts.len(), 1);
        assert_eq!(output.derived_facts[0].key, "build_ok");
        assert_eq!(output.derived_facts[0].value, "true");
    }

    #[tokio::test]
    async fn multiple_rules_accumulate() {
        let rules = vec![
            RuleConfig {
                id: "build_ok".to_string(),
                enricher_id: "maven".to_string(),
                condition: "exit_code == 0".to_string(),
                priority: 0,
                enabled: true,
                actions: vec![RuleAction::SetVerdict("PASSED".to_string())],
            },
            RuleConfig {
                id: "has_test_failures".to_string(),
                enricher_id: "maven".to_string(),
                condition: "fact('tests_failed') > '0'".to_string(),
                priority: 1,
                enabled: true,
                actions: vec![
                    RuleAction::SetVerdict("TEST_FAILURES".to_string()),
                    RuleAction::Recommend("Review failing tests".to_string()),
                ],
            },
        ];
        let repo = Arc::new(MockRepo { rules });
        let evaluator = DefaultRuleEvaluator::new(repo);

        let facts = vec![fact("tests_failed", "2")];
        let output = evaluator
            .evaluate(
                "maven",
                &make_invocation(),
                &make_result(0, false, ""),
                &facts,
            )
            .await;

        // R0 matches first → verdict="PASSED", then R1 matches → verdict="TEST_FAILURES" (last wins)
        assert_eq!(output.verdict.as_deref(), Some("TEST_FAILURES"));
        assert_eq!(output.recommendations.as_slice(), ["Review failing tests"]);
    }
}
