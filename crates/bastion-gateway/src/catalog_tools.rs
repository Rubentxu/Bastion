//! Catalog MCP tools: experience and assertion management.
//!
//! Exposes `experience_list`, `experience_get`, `assertion_list`,
//! `assertion_run`, and `assertion_dry_run` as MCP tools.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::BastionGateway;

// ─── Experience tools ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExperienceListParams {
    /// Filter experiences by trace ID (required).
    pub trace_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExperienceGetParams {
    /// The experience record ID.
    pub experience_id: String,
}

// ─── Assertion tools ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssertionRunParams {
    /// The assertion ID to evaluate.
    pub assertion_id: String,
    /// The experience ID to evaluate against.
    pub experience_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssertionDryRunParams {
    /// The assertion ID to inspect.
    pub assertion_id: String,
}

// ─── Tool router function ────────────────────────────────────────────────────

/// Returns the catalog tools router, combining all catalog MCP tools.
pub fn catalog_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::experience_list_tool_attr(),
            BastionGateway::experience_list,
        ))
        .with_route((
            BastionGateway::experience_get_tool_attr(),
            BastionGateway::experience_get,
        ))
        .with_route((
            BastionGateway::assertion_list_tool_attr(),
            BastionGateway::assertion_list,
        ))
        .with_route((
            BastionGateway::assertion_run_tool_attr(),
            BastionGateway::assertion_run,
        ))
        .with_route((
            BastionGateway::assertion_dry_run_tool_attr(),
            BastionGateway::assertion_dry_run,
        ))
}

// ─── Tool implementations ────────────────────────────────────────────────────

impl BastionGateway {
    /// List experience records by trace ID, sorted by started_at descending.
    ///
    /// Experiences record tool invocations (sandbox_run, sandbox_prepare, etc.) with stdout/stderr and exit codes.
    #[tool(description = "List experience records by trace_id, sorted by started_at descending. Experiences record tool invocations with stdout/stderr and exit codes.")]
    async fn experience_list(
        &self,
        Parameters(params): Parameters<ExperienceListParams>,
    ) -> String {
        let store = match &self.catalog_config.experience_store {
            Some(s) => s,
            None => {
                return serde_json::json!({
                    "error": "experience store not configured"
                })
                .to_string();
            }
        };

        match store.find_by_trace_id(&params.trace_id).await {
            Ok(records) => serde_json::json!({
                "trace_id": params.trace_id,
                "count": records.len(),
                "experiences": records
            })
            .to_string(),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list experiences");
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Get a single experience record by ID.
    ///
    /// Returns the full experience record including all recorded tool invocations and their outputs.
    #[tool(description = "Get a single experience record by ID. Returns full invocation details and outputs.")]
    async fn experience_get(&self, Parameters(params): Parameters<ExperienceGetParams>) -> String {
        let store = match &self.catalog_config.experience_store {
            Some(s) => s,
            None => {
                return serde_json::json!({
                    "error": "experience store not configured"
                })
                .to_string();
            }
        };

        match store.find_by_id(&params.experience_id).await {
            Ok(Some(record)) => serde_json::json!({
                "experience": record
            })
            .to_string(),
            Ok(None) => serde_json::json!({
                "error": format!("experience '{}' not found", params.experience_id)
            })
            .to_string(),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to get experience");
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// List all available assertions.
    ///
    /// Assertions validate experience records against expected outcomes (e.g., exit_code=0, stdout contains pattern).
    #[tool(description = "List all loaded assertion descriptors. Assertions validate experience records against expected outcomes (exit codes, stdout patterns, etc.).")]
    async fn assertion_list(&self) -> String {
        let registry = match &self.catalog_config.assertion_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "assertion registry not configured"
                })
                .to_string();
            }
        };

        let assertions = registry.list();
        serde_json::json!({
            "count": assertions.len(),
            "assertions": assertions
        })
        .to_string()
    }

    /// Run an assertion against an experience record.
    ///
    /// Evaluates all checks in the assertion against the experience. Returns passed=true only if all checks pass.
    #[tool(description = "Evaluate an assertion against an experience record. All checks must pass for passed=true. Returns per-check results.")]
    async fn assertion_run(&self, Parameters(params): Parameters<AssertionRunParams>) -> String {
        let store = match &self.catalog_config.experience_store {
            Some(s) => s,
            None => {
                return serde_json::json!({
                    "error": "experience store not configured"
                })
                .to_string();
            }
        };

        let registry = match &self.catalog_config.assertion_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "assertion registry not configured"
                })
                .to_string();
            }
        };

        let assertion = match registry.get(&params.assertion_id) {
            Some(a) => a,
            None => {
                return serde_json::json!({
                    "error": format!("assertion '{}' not found", params.assertion_id)
                })
                .to_string();
            }
        };

        let experience = match store.find_by_id(&params.experience_id).await {
            Ok(Some(e)) => e,
            Ok(None) => {
                return serde_json::json!({
                    "error": format!("experience '{}' not found", params.experience_id)
                })
                .to_string();
            }
            Err(e) => {
                return serde_json::json!({"error": e.to_string()}).to_string();
            }
        };

        // Inline evaluation logic to avoid generic type issues with dyn ExperienceStore
        use bastion_domain::catalog::assertion::{AssertionResult, CheckResult};
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
        let result = AssertionResult {
            passed,
            assertion_id: assertion.id.clone(),
            check_results,
        };

        serde_json::json!({
            "assertion_id": result.assertion_id,
            "passed": result.passed,
            "check_results": result.check_results
        })
        .to_string()
    }

    /// Get an assertion descriptor without running it against an experience.
    ///
    /// Use to inspect what an assertion checks without evaluating it.
    #[tool(description = "Get assertion descriptor without evaluating it. Use to inspect what an assertion checks.")]
    async fn assertion_dry_run(
        &self,
        Parameters(params): Parameters<AssertionDryRunParams>,
    ) -> String {
        let registry = match &self.catalog_config.assertion_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "assertion registry not configured"
                })
                .to_string();
            }
        };

        match registry.get(&params.assertion_id) {
            Some(assertion) => serde_json::json!({
                "assertion": assertion
            })
            .to_string(),
            None => serde_json::json!({
                "error": format!("assertion '{}' not found", params.assertion_id)
            })
            .to_string(),
        }
    }
}
