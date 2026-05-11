//! Doctor MCP tools: list, run, and explain doctors.
//!
//! Exposes `doctor_list`, `doctor_run`, and `doctor_explain` as MCP tools.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool};
use schemars::JsonSchema;
use serde::Deserialize;

use bastion_domain::catalog::assertion::CheckResult as AssertionCheckResult;
use bastion_domain::catalog::doctor::{DoctorCheck, DoctorResult};

use crate::server::BastionGateway;

// ─── Doctor tool params ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorRunParams {
    /// The doctor ID to run.
    pub doctor_id: String,
    /// The sandbox ID to run the doctor against.
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorExplainParams {
    /// The doctor ID to explain.
    pub doctor_id: String,
}

// ─── Tool router function ───────────────────────────────────────────────────

/// Returns the doctor tools router.
pub fn doctor_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::doctor_list_tool_attr(),
            BastionGateway::doctor_list,
        ))
        .with_route((
            BastionGateway::doctor_run_tool_attr(),
            BastionGateway::doctor_run,
        ))
        .with_route((
            BastionGateway::doctor_explain_tool_attr(),
            BastionGateway::doctor_explain,
        ))
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// List all registered doctors.
    ///
    /// Doctors are diagnostic checks that verify sandbox health (aliveness, resource limits, assertion-driven checks).
    #[tool(
        description = "List all registered doctor descriptors. Doctors verify sandbox health (aliveness, resources, assertion-driven checks)."
    )]
    async fn doctor_list(&self, Parameters(_params): Parameters<DoctorListParams>) -> String {
        let registry = match &self.catalog_config.doctor_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "doctor registry not configured"
                })
                .to_string();
            }
        };

        let doctors = registry.list();
        serde_json::json!({
            "count": doctors.len(),
            "doctors": doctors
        })
        .to_string()
    }

    /// Run a doctor against a sandbox.
    ///
    /// Executes all checks in the doctor. Returns per-check results and an overall status. Use sandbox_info to get a valid sandbox_id first.
    #[tool(
        description = "Run a doctor against a sandbox. Executes all checks, returns per-check results and overall status. Use sandbox_info to get a valid sandbox_id first."
    )]
    async fn doctor_run(&self, Parameters(params): Parameters<DoctorRunParams>) -> String {
        let registry = match &self.catalog_config.doctor_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "doctor registry not configured"
                })
                .to_string();
            }
        };

        let descriptor = match registry.get(&params.doctor_id) {
            Some(d) => d,
            None => {
                return serde_json::json!({
                    "error": format!("doctor '{}' not found", params.doctor_id)
                })
                .to_string();
            }
        };

        // Generate trace_id for correlation
        let trace_id = uuid::Uuid::new_v4().to_string();

        // Run the doctor checks
        let mut result = DoctorResult::new(
            descriptor.id.clone(),
            Some(params.sandbox_id.clone()),
            trace_id,
            descriptor.severity,
        );

        let mut all_passed = true;
        let mut rationale_parts = Vec::new();

        for check in &descriptor.checks {
            let check_result = self.run_doctor_check(check, &params.sandbox_id).await;
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
                params.sandbox_id
            )
        } else {
            rationale_parts.join("; ")
        };

        result.finalize();

        serde_json::json!({
            "doctor_id": result.doctor_id,
            "sandbox_id": result.sandbox_id,
            "status": result.status,
            "severity": result.severity,
            "trace_id": result.trace_id,
            "check_results": result.check_results,
            "rationale": result.rationale,
            "executed_at": result.executed_at.to_rfc3339()
        })
        .to_string()
    }

    /// Get doctor descriptor and TOML source.
    ///
    /// Returns the full doctor definition including all checks, severity, category, and the raw TOML configuration.
    #[tool(
        description = "Get doctor descriptor and TOML source. Includes all checks, severity, category, and raw TOML config."
    )]
    async fn doctor_explain(&self, Parameters(params): Parameters<DoctorExplainParams>) -> String {
        let registry = match &self.catalog_config.doctor_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "doctor registry not configured"
                })
                .to_string();
            }
        };

        let descriptor = match registry.get(&params.doctor_id) {
            Some(d) => d,
            None => {
                return serde_json::json!({
                    "error": format!("doctor '{}' not found", params.doctor_id)
                })
                .to_string();
            }
        };

        let toml_source = registry.get_source(&params.doctor_id);

        serde_json::json!({
            "doctor_id": descriptor.id,
            "name": descriptor.name,
            "description": descriptor.description,
            "category": descriptor.category,
            "severity": descriptor.severity,
            "checks": descriptor.checks,
            "toml_source": toml_source
        })
        .to_string()
    }

    /// Run a single doctor check and return the result.
    async fn run_doctor_check(
        &self,
        check: &DoctorCheck,
        sandbox_id: &str,
    ) -> AssertionCheckResult {
        match check {
            DoctorCheck::Aliveness {
                sandbox_id: check_sandbox_id,
            } => {
                let target_id = check_sandbox_id.as_deref().unwrap_or(sandbox_id);

                match self.provider.is_alive(&target_id.to_string().into()).await {
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
                if let Some(ref pool) = self.gateway_config.pool_manager {
                    let stats = pool.stats().await;

                    // Check max_total
                    if let Some(max) = max_total
                        && stats.total > *max
                    {
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
                    .catalog_config
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
                let experience = if let Some(ref store) = self.catalog_config.experience_store {
                    match store.find_by_trace_id(sandbox_id).await {
                        Ok(mut records) => {
                            records.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                            records.into_iter().next()
                        }
                        Err(_) => None,
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
                            Some(
                                check_results
                                    .iter()
                                    .filter(|r| !r.passed)
                                    .map(|r| r.reason.as_deref().unwrap_or("failed"))
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            )
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
