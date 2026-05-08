//! SuggestAdvice use case.
//!
//! Evaluates context (assertion failures, doctor failures, experience patterns)
//! against loaded advice descriptors and returns matched advice results.

use std::sync::Arc;

use bastion_domain::catalog::advice::{AdviceDescriptor, AdviceResult, AdviceTrigger};

/// Context passed to advice suggestion.
#[derive(Debug, Clone, Default)]
pub struct SuggestAdviceContext {
    /// Assertion IDs that recently failed.
    pub assertion_failures: Vec<String>,
    /// Doctor IDs that recently failed.
    pub doctor_failures: Vec<String>,
    /// Experience pattern hints (optional, for future use).
    pub experience_hints: Vec<ExperienceHint>,
}

/// A hint about experience patterns for advice matching.
#[derive(Debug, Clone)]
pub struct ExperienceHint {
    pub tool_name: String,
    pub status: String,
    pub count: u32,
}

/// Port for the advice registry (implemented by infrastructure).
pub trait AdviceRegistryPort: Send + Sync {
    /// List all loaded advice descriptors.
    fn list(&self) -> Vec<AdviceDescriptor>;

    /// Get a single advice descriptor by ID.
    fn get(&self, id: &str) -> Option<AdviceDescriptor>;

    /// Number of loaded advice items.
    fn len(&self) -> usize;

    /// Check if the registry is empty.
    fn is_empty(&self) -> bool;
}

/// Port for experience store (read-only, for ExperiencePattern triggers).
pub trait ExperienceQueryPort: Send + Sync {
    /// List experiences for a given trace ID.
    fn find_by_trace_id(
        &self,
        trace_id: &str,
    ) -> impl std::future::Future<
        Output = Result<
            Vec<bastion_domain::catalog::experience::ExperienceRecord>,
            bastion_domain::shared::DomainError,
        >,
    > + Send;
}

/// SuggestAdvice use case — evaluates context against advice triggers.
#[allow(dead_code)]
pub struct SuggestAdviceUseCase<R: AdviceRegistryPort, E: ExperienceQueryPort> {
    advice_registry: Arc<R>,
    experience_query: Option<Arc<E>>,
}

impl<R: AdviceRegistryPort, E: ExperienceQueryPort> SuggestAdviceUseCase<R, E> {
    /// Create a new use case.
    pub fn new(advice_registry: Arc<R>, experience_query: Option<Arc<E>>) -> Self {
        Self {
            advice_registry,
            experience_query,
        }
    }

    /// Suggest advice based on the given context.
    ///
    /// Returns all matching `AdviceResult` items, sorted by severity
    /// (critical first, then warning, then hint).
    pub async fn suggest(&self, context: SuggestAdviceContext) -> Vec<AdviceResult> {
        let all_advice = self.advice_registry.list();
        let mut results = Vec::new();

        for descriptor in all_advice {
            if let Some(matched_trigger) = self.evaluate_triggers(&descriptor.triggers, &context) {
                results.push(AdviceResult::new(
                    descriptor.id.clone(),
                    matched_trigger,
                    descriptor.message.clone(),
                    descriptor.suggested_actions.clone(),
                    descriptor.severity,
                ));
            }
        }

        // Sort by severity (critical > warning > hint)
        results.sort_by_key(|r| r.severity.sort_key());
        results
    }

    /// Evaluate all triggers for a descriptor and return the first matching one.
    fn evaluate_triggers(
        &self,
        triggers: &[AdviceTrigger],
        context: &SuggestAdviceContext,
    ) -> Option<AdviceTrigger> {
        for trigger in triggers {
            if self.trigger_matches(trigger, context) {
                return Some(trigger.clone());
            }
        }
        None
    }

    /// Check if a single trigger matches the given context.
    fn trigger_matches(&self, trigger: &AdviceTrigger, context: &SuggestAdviceContext) -> bool {
        match trigger {
            AdviceTrigger::AssertionFailed { assertion_id } => {
                context.assertion_failures.contains(assertion_id)
            }
            AdviceTrigger::DoctorFailed { doctor_id } => {
                context.doctor_failures.contains(doctor_id)
            }
            AdviceTrigger::ExperiencePattern {
                tool_name,
                status,
                threshold,
            } => {
                // Check if any experience hint matches the pattern
                context.experience_hints.iter().any(|h| {
                    h.tool_name == *tool_name && h.status == *status && h.count >= *threshold
                })
            }
        }
    }
}

/// Configuration for advice features.
#[derive(Debug, Clone, Default)]
pub struct AdviceConfig {
    /// Global enable/disable flag.
    pub enabled: bool,
    /// List of disabled advice IDs.
    pub disabled: Vec<String>,
}

impl AdviceConfig {
    /// Check if a specific advice ID is disabled.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.contains(&id.to_string())
    }

    /// Create default config (enabled, no disabled list).
    pub fn default_enabled() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}
