//! RunDoctor use case.
//!
//! Evaluates a doctor descriptor against a sandbox, dispatching each check
//! to the appropriate port (provider, pool manager, or assertion registry).

use std::sync::Arc;

use bastion_domain::catalog::assertion::CheckResult as AssertionCheckResult;
use bastion_domain::catalog::doctor::{DoctorCheck, DoctorDescriptor, DoctorResult};
use bastion_domain::catalog::experience::ExperienceStore;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::shared::id::SandboxId;


/// Port for pool statistics (implemented by infrastructure).
pub trait PoolStatsPort: Send + Sync {
    /// Get current pool statistics synchronously.
    fn stats(&self) -> PoolStats;
}

/// Pool statistics.
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub templates: Vec<TemplateStats>,
    pub active: usize,
    pub idle: usize,
    pub total: usize,
}

/// Statistics for a single template pool.
#[derive(Debug, Clone)]
pub struct TemplateStats {
    pub template: String,
    pub idle: usize,
}

/// Port for assertion registry (implemented by infrastructure).
pub trait AssertionRegistryPort: Send + Sync {
    fn get(&self, id: &str) -> Option<bastion_domain::catalog::assertion::AssertionDescriptor>;
}

/// Use case for running a doctor against a sandbox.
pub struct RunDoctorUseCase<P: PoolStatsPort, R: AssertionRegistryPort> {
    provider: Arc<dyn SandboxProvider>,
    pool_stats: Option<Arc<P>>,
    assertion_registry: Option<Arc<R>>,
    experience_store: Option<Arc<dyn ExperienceStore>>,
}

impl<P: PoolStatsPort, R: AssertionRegistryPort> RunDoctorUseCase<P, R> {
    /// Create a new use case.
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        pool_stats: Option<Arc<P>>,
        assertion_registry: Option<Arc<R>>,
        experience_store: Option<Arc<dyn ExperienceStore>>,
    ) -> Self {
        Self {
            provider,
            pool_stats,
            assertion_registry,
            experience_store,
        }
    }

    /// Run a doctor against a sandbox.
    ///
    /// Returns the doctor result with per-check outcomes.
    pub async fn run(
        &self,
        descriptor: &DoctorDescriptor,
        sandbox_id: Option<&str>,
        trace_id: &str,
    ) -> DoctorResult {
        let mut result = DoctorResult::new(
            descriptor.id.clone(),
            sandbox_id.map(String::from),
            trace_id.to_string(),
            descriptor.severity,
        );

        let mut all_passed = true;
        let mut rationale_parts = Vec::new();

        for check in &descriptor.checks {
            let check_result = self.run_check(check, sandbox_id).await;
            all_passed = all_passed && check_result.passed;

            if !check_result.passed {
                rationale_parts.push(format!(
                    "{}: {}",
                    check_result.check,
                    check_result.reason.as_deref().unwrap_or("failed")
                ));
            }

            result.add_check_result(check_result);
        }

        // Generate rationale
        result.rationale = if all_passed {
            format!(
                "Doctor '{}' passed all {} checks for sandbox '{}'",
                descriptor.name,
                result.check_results.len(),
                sandbox_id.unwrap_or("(none)")
            )
        } else {
            rationale_parts.join("; ")
        };

        result.finalize();
        result
    }

    /// Run a single check.
    async fn run_check(&self, check: &DoctorCheck, sandbox_id: Option<&str>) -> AssertionCheckResult {
        match check {
            DoctorCheck::Aliveness { sandbox_id: check_sandbox_id } => {
                let target_id = check_sandbox_id
                    .as_deref()
                    .or(sandbox_id)
                    .expect("Aliveness check requires a sandbox_id");

                match self.provider.is_alive(&SandboxId::new(target_id)).await {
                    Ok(true) => AssertionCheckResult {
                        check: "Aliveness".to_string(),
                        passed: true,
                        reason: None,
                    },
                    Ok(false) => AssertionCheckResult {
                        check: "Aliveness".to_string(),
                        passed: false,
                        reason: Some(format!("Sandbox {} is not alive", target_id)),
                    },
                    Err(e) => AssertionCheckResult {
                        check: "Aliveness".to_string(),
                        passed: false,
                        reason: Some(format!("Aliveness check failed: {}", e)),
                    },
                }
            }
            DoctorCheck::Resources {
                max_total,
                max_idle_per_template,
            } => {
                if let Some(ref pool) = self.pool_stats {
                    let stats = pool.stats();

                    // Check max_total
                    if let Some(max) = max_total && stats.total > *max {
                        return AssertionCheckResult {
                            check: "Resources".to_string(),
                            passed: false,
                            reason: Some(format!(
                                "Total sandboxes {} exceeds max {}",
                                stats.total, max
                            )),
                        };
                    }

                    // Check max_idle_per_template
                    if let Some(max_idle) = max_idle_per_template {
                        for template in &stats.templates {
                            if template.idle > *max_idle {
                                return AssertionCheckResult {
                                    check: "Resources".to_string(),
                                    passed: false,
                                    reason: Some(format!(
                                        "Template '{}' idle {} exceeds max {}",
                                        template.template, template.idle, max_idle
                                    )),
                                };
                            }
                        }
                    }

                    AssertionCheckResult {
                        check: "Resources".to_string(),
                        passed: true,
                        reason: None,
                    }
                } else {
                    AssertionCheckResult {
                        check: "Resources".to_string(),
                        passed: false,
                        reason: Some("Pool manager not available".to_string()),
                    }
                }
            }
            DoctorCheck::AssertionDriven { assertion_id } => {
                // Get the assertion
                let assertion = match self
                    .assertion_registry
                    .as_ref()
                    .and_then(|r| r.get(assertion_id))
                {
                    Some(a) => a,
                    None => {
                        return AssertionCheckResult {
                            check: format!("AssertionDriven({})", assertion_id),
                            passed: false,
                            reason: Some(format!("Assertion '{}' not found", assertion_id)),
                        };
                    }
                };

                // Find an experience record to evaluate against
                let experience = if let Some(ref store) = self.experience_store {
                    // Try to find the most recent experience for this sandbox
                    if let Some(target_id) = sandbox_id {
                        match store.find_by_trace_id(target_id).await {
                            Ok(mut records) => {
                                records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                                records.into_iter().next()
                            }
                            Err(_) => None,
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Evaluate the assertion
                if let Some(record) = experience {
                    let check_results: Vec<_> = assertion
                        .checks
                        .iter()
                        .map(|check| {
                            let (passed, reason) = check.evaluate(&record);
                            AssertionCheckResult {
                                check: format!("{:?}", check),
                                passed,
                                reason,
                            }
                        })
                        .collect();

                    let all_passed = check_results.iter().all(|r| r.passed);
                    AssertionCheckResult {
                        check: format!("AssertionDriven({})", assertion_id),
                        passed: all_passed,
                        reason: if all_passed {
                            None
                        } else {
                            Some(check_results
                                .iter()
                                .filter(|r| !r.passed)
                                .map(|r| r.reason.as_deref().unwrap_or("failed"))
                                .collect::<Vec<_>>()
                                .join("; "))
                        },
                    }
                } else {
                    AssertionCheckResult {
                        check: format!("AssertionDriven({})", assertion_id),
                        passed: false,
                        reason: Some(format!(
                            "No experience record found for assertion '{}'",
                            assertion_id
                        )),
                    }
                }
            }
        }
    }
}