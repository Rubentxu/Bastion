//! Advice MCP tools: list, get, suggest, and configure advice.
//!
//! Exposes `advice_list`, `advice_get`, `advice_suggest`, and `advice_configure`
//! as MCP tools.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool};
use schemars::JsonSchema;
use serde::Deserialize;

use bastion_domain::catalog::advice::{AdviceDescriptor, AdviceResult, AdviceTrigger};
use bastion_infrastructure::catalog::toml_advice_parser::AdviceConfig;

use crate::server::BastionGateway;
use bastion_application::catalog::suggest_advice::{ExperienceHint, SuggestAdviceContext};

// ─── Tool params ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AdviceListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AdviceGetParams {
    /// The advice ID to retrieve.
    pub advice_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExperienceHintParams {
    /// Tool name (e.g., "sandbox_run").
    pub tool_name: String,
    /// Status to match ("failure", "success", "timeout", "cancelled").
    pub status: String,
    /// Count of matching experiences.
    pub count: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AdviceSuggestParams {
    /// Assertion IDs that recently failed.
    #[serde(default)]
    pub assertion_failures: Vec<String>,
    /// Doctor IDs that recently failed.
    #[serde(default)]
    pub doctor_failures: Vec<String>,
    /// Experience pattern hints (optional).
    #[serde(default)]
    pub experience_hints: Vec<ExperienceHintParams>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AdviceConfigureParams {
    /// Advice ID to configure, or "global" for global settings.
    /// Use "clear_disabled" flag to clear the disabled list.
    pub advice_id: String,
    /// Enable (true) or disable (false) the advice.
    pub enabled: bool,
    /// When true and combined with global_enabled:true, clears the disabled list.
    #[serde(default)]
    pub clear_disabled: bool,
}

// ─── Tool router function ────────────────────────────────────────────────────

/// Returns the advice tools router.
pub fn advice_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::advice_list_tool_attr(),
            BastionGateway::advice_list,
        ))
        .with_route((
            BastionGateway::advice_get_tool_attr(),
            BastionGateway::advice_get,
        ))
        .with_route((
            BastionGateway::advice_suggest_tool_attr(),
            BastionGateway::advice_suggest,
        ))
        .with_route((
            BastionGateway::advice_configure_tool_attr(),
            BastionGateway::advice_configure,
        ))
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// List all registered advice descriptors.
    ///
    /// Returns advice that is not disabled by configuration. Filtered by global_enabled and per-ID disabled list.
    #[tool(
        description = "List all loaded and enabled advice descriptors. Respects global_enabled and per-ID disabled list."
    )]
    async fn advice_list(&self, Parameters(_params): Parameters<AdviceListParams>) -> String {
        let registry = match &self.catalog_config.advice_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "advice registry not configured"
                })
                .to_string();
            }
        };

        let config = self
            .catalog_config
            .advice_config
            .as_ref()
            .map(|c| c.get_config())
            .unwrap_or_else(AdviceConfig::default_enabled);

        // Filter out disabled advice
        let all_advice: Vec<AdviceDescriptor> = registry.list();
        let filtered: Vec<AdviceDescriptor> = if config.enabled {
            all_advice
                .into_iter()
                .filter(|a| !config.is_disabled(&a.id))
                .collect()
        } else {
            Vec::new()
        };

        serde_json::json!({
            "count": filtered.len(),
            "advice": filtered,
            "global_enabled": config.enabled
        })
        .to_string()
    }

    /// Get a single advice descriptor by ID.
    ///
    /// Returns the full advice descriptor including triggers, suggested_actions, and the raw TOML source.
    #[tool(
        description = "Get a single advice descriptor by ID. Includes triggers, suggested_actions, and raw TOML source."
    )]
    async fn advice_get(&self, Parameters(params): Parameters<AdviceGetParams>) -> String {
        let registry = match &self.catalog_config.advice_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "advice registry not configured"
                })
                .to_string();
            }
        };

        let descriptor = match registry.get(&params.advice_id) {
            Some(d) => d,
            None => {
                return serde_json::json!({
                    "error": format!("advice '{}' not found", params.advice_id)
                })
                .to_string();
            }
        };

        let toml_source = registry.get_source(&params.advice_id);

        serde_json::json!({
            "advice": descriptor,
            "toml_source": toml_source
        })
        .to_string()
    }

    /// Suggest advice based on current context (failures, patterns).
    ///
    /// Pass assertion_failures, doctor_failures, and experience_hints to get relevant advice sorted by severity.
    /// Use after running assertions or doctors to get actionable suggestions.
    #[tool(
        description = "Suggest advice based on assertion failures, doctor failures, and experience patterns. Returns advice sorted by severity. Use after running assertions/doctors."
    )]
    async fn advice_suggest(&self, Parameters(params): Parameters<AdviceSuggestParams>) -> String {
        let registry = match &self.catalog_config.advice_registry {
            Some(r) => r,
            None => {
                return serde_json::json!({
                    "error": "advice registry not configured"
                })
                .to_string();
            }
        };

        let config = self
            .catalog_config
            .advice_config
            .as_ref()
            .map(|c| c.get_config())
            .unwrap_or_else(AdviceConfig::default_enabled);

        // If globally disabled, return empty
        if !config.enabled {
            return serde_json::json!({
                "matches": Vec::<AdviceResult>::new(),
                "global_enabled": false
            })
            .to_string();
        }

        // Build context
        let experience_hints = params
            .experience_hints
            .into_iter()
            .map(|h| ExperienceHint {
                tool_name: h.tool_name,
                status: h.status,
                count: h.count,
            })
            .collect();

        let context = SuggestAdviceContext {
            assertion_failures: params.assertion_failures,
            doctor_failures: params.doctor_failures,
            experience_hints,
        };

        // Evaluate all advice against context
        let all_advice = registry.list();
        let mut results: Vec<AdviceResult> = Vec::new();

        for descriptor in all_advice {
            // Skip disabled advice
            if config.is_disabled(&descriptor.id) {
                continue;
            }

            // Evaluate triggers
            for trigger in &descriptor.triggers {
                let matched = match trigger {
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
                    } => context.experience_hints.iter().any(|h| {
                        h.tool_name == *tool_name && h.status == *status && h.count >= *threshold
                    }),
                };

                if matched {
                    results.push(AdviceResult::new(
                        descriptor.id.clone(),
                        trigger.clone(),
                        descriptor.message.clone(),
                        descriptor.suggested_actions.clone(),
                        descriptor.severity,
                    ));
                    break; // Only first matching trigger per advice
                }
            }
        }

        // Sort by severity (critical > warning > hint)
        results.sort_by_key(|r| r.severity.sort_key());

        serde_json::json!({
            "matches": results,
            "global_enabled": true
        })
        .to_string()
    }

    /// Configure advice (enable/disable globally or per-ID).
    ///
    /// Set advice_id to "global" to control global_enabled. Use clear_disabled=true with global_enabled=true to reset the disabled list.
    #[tool(
        description = "Enable or disable advice globally or per-ID. Use advice_id='global' for global settings. clear_disabled=true resets the disabled list."
    )]
    async fn advice_configure(
        &self,
        Parameters(params): Parameters<AdviceConfigureParams>,
    ) -> String {
        let config_store = match &self.catalog_config.advice_config {
            Some(c) => c,
            None => {
                return serde_json::json!({
                    "error": "advice config store not configured"
                })
                .to_string();
            }
        };

        let result = if params.advice_id == "global" {
            if params.clear_disabled {
                config_store.clear_disabled()
            } else {
                config_store.set_global_enabled(params.enabled)
            }
        } else if params.enabled {
            config_store.enable_advice(&params.advice_id)
        } else {
            config_store.disable_advice(&params.advice_id)
        };

        match result {
            Ok(cfg) => serde_json::json!({
                "status": "ok",
                "advice_id": params.advice_id,
                "enabled": params.enabled,
                "global_enabled": cfg.enabled,
                "disabled_count": cfg.disabled.len()
            })
            .to_string(),
            Err(e) => {
                tracing::error!(error = %e, "Failed to update advice config");
                serde_json::json!({
                    "error": format!("Failed to update config: {}", e)
                })
                .to_string()
            }
        }
    }
}
