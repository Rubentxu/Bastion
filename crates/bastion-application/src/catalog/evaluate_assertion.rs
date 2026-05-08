//! EvaluateAssertion use case.
//!
//! Evaluates an assertion descriptor against an experience record.

use std::sync::Arc;

use bastion_domain::catalog::assertion::{AssertionDescriptor, AssertionResult, CheckResult};
use bastion_domain::catalog::experience::{ExperienceRecord, ExperienceStore};
use bastion_domain::shared::DomainError;

/// Use case for evaluating an assertion against an experience record.
#[derive(Debug)]
pub struct EvaluateAssertionUseCase<S: ExperienceStore> {
    store: Arc<S>,
}

impl<S: ExperienceStore> EvaluateAssertionUseCase<S> {
    /// Create a new use case with the given store.
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    /// Evaluate an assertion against an experience record.
    ///
    /// Returns the assertion result with per-check outcomes.
    pub async fn execute(
        &self,
        assertion: &AssertionDescriptor,
        experience_id: &str,
    ) -> Result<AssertionResult, DomainError> {
        let experience = self
            .store
            .find_by_id(experience_id)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("experience '{}'", experience_id)))?;

        let check_results: Vec<CheckResult> = assertion
            .checks
            .iter()
            .map(|check| {
                let (passed, reason) = check.evaluate(&experience);
                CheckResult {
                    check: format!("{:?}", check),
                    passed,
                    reason,
                }
            })
            .collect();

        let passed = check_results.iter().all(|r| r.passed);

        Ok(AssertionResult {
            passed,
            assertion_id: assertion.id.clone(),
            check_results,
        })
    }

    /// Evaluate an assertion against a pre-fetched experience record.
    pub fn evaluate_against(
        &self,
        assertion: &AssertionDescriptor,
        experience: &ExperienceRecord,
    ) -> AssertionResult {
        let check_results: Vec<CheckResult> = assertion
            .checks
            .iter()
            .map(|check| {
                let (passed, reason) = check.evaluate(experience);
                CheckResult {
                    check: format!("{:?}", check),
                    passed,
                    reason,
                }
            })
            .collect();

        let passed = check_results.iter().all(|r| r.passed);

        AssertionResult {
            passed,
            assertion_id: assertion.id.clone(),
            check_results,
        }
    }
}
