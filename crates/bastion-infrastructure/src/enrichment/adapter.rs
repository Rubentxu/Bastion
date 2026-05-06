//! Bastion enrichment adapter.
//!
//! Wires the enrichment engine into Bastion's `sandbox_run` tool.
//! Maps Bastion domain types to host-agnostic types, invokes the pipeline,
//! and extends the JSON response additively.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;

use enrichment_engine::models::{AgentContext, OperationInvocation, OperationResult};
use enrichment_engine::pipeline::FactPipeline;
use enrichment_engine::traits::CatalogRepository;

use super::config::EnrichmentConfig;
use super::fs::SandboxFileSystem;

/// The Bastion enrichment adapter.
///
/// Holds a shared `FactPipeline` and a `SandboxProvider` reference.
/// Implements the enrichment workflow: map CommandSpec → OperationInvocation,
/// call pipeline, map back to JSON extension.
pub struct BastionEnrichmentAdapter {
    pipeline: Arc<FactPipeline>,
    provider: Arc<dyn SandboxProvider>,
    config: EnrichmentConfig,
}

impl BastionEnrichmentAdapter {
    /// Create a new adapter.
    ///
    /// The pipeline is built once from the catalog and reused for all enrich() calls.
    /// The provider is used to create per-request SandboxFileSystem instances.
    pub fn new(
        catalog: Arc<dyn CatalogRepository>,
        provider: Arc<dyn SandboxProvider>,
        config: EnrichmentConfig,
    ) -> Self {
        let pipeline = FactPipeline::new(catalog);
        Self {
            pipeline: Arc::new(pipeline),
            provider,
            config,
        }
    }

    /// Create a new adapter with a pre-built pipeline (for shared pipeline scenario).
    pub fn with_pipeline(
        pipeline: Arc<FactPipeline>,
        provider: Arc<dyn SandboxProvider>,
        config: EnrichmentConfig,
    ) -> Self {
        Self {
            pipeline,
            provider,
            config,
        }
    }

    /// Enrich a sandbox command execution.
    ///
    /// Maps `CommandSpec` and `CommandResult` to the enrichment engine's types,
    /// runs the pipeline, and returns an optional `AgentContext`.
    ///
    /// Returns `None` if enrichment is disabled or no enricher matches.
    /// Errors are traced at warn level and return `None` (non-blocking).
    pub async fn enrich(
        &self,
        sandbox_id: &SandboxId,
        command_spec: &CommandSpec,
        command_result: &CommandResult,
    ) -> Option<AgentContext> {
        if !self.config.enabled {
            return None;
        }

        let invocation = Self::map_command_spec(command_spec);
        let result = Self::map_command_result(command_result);

        let fs = SandboxFileSystem::new(self.provider.clone(), sandbox_id.clone());

        let start = Instant::now();
        let ctx = match self.pipeline.run(invocation, result, &fs).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!(error = %e, "Enrichment pipeline failed");
                return None;
            }
        };
        let elapsed = start.elapsed();

        // Don't block on slow enrichment — trace and continue
        if elapsed > Duration::from_millis(100) {
            tracing::debug!(elapsed_ms = elapsed.as_millis() as u64, "Enrichment completed slowly");
        }

        // Return None if no facts were extracted (no enricher matched)
        if ctx.facts.is_empty() {
            return None;
        }

        Some(ctx)
    }

    /// Map a `CommandSpec` to an `OperationInvocation`.
    fn map_command_spec(spec: &CommandSpec) -> OperationInvocation {
        OperationInvocation {
            command: spec.command.clone(),
            args: spec.args.clone(),
            working_dir: spec.working_dir.clone(),
            env_vars: spec.env_vars.clone(),
        }
    }

    /// Map a `CommandResult` to an `OperationResult`.
    fn map_command_result(result: &CommandResult) -> OperationResult {
        OperationResult {
            exit_code: result.exit_code,
            stdout: String::from_utf8_lossy(&result.stdout).to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            duration_ms: result.duration_ms,
            timed_out: result.timed_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::execution::command::CommandSpec;
    use bastion_domain::execution::command::CommandResult;

    #[tokio::test]
    async fn test_command_spec_mapping() {
        let spec = CommandSpec::new("mvn package")
            .with_working_dir("/workspace")
            .with_env("MAVEN_OPTS", "-Xmx512m");

        let invocation = BastionEnrichmentAdapter::map_command_spec(&spec);
        assert_eq!(invocation.command, "mvn package");
        assert_eq!(invocation.working_dir.as_deref(), Some("/workspace"));
        assert_eq!(invocation.env_vars.get("MAVEN_OPTS").map(|s| s.as_str()), Some("-Xmx512m"));
    }

    #[tokio::test]
    async fn test_command_result_mapping() {
        let result = CommandResult {
            exit_code: 0,
            stdout: b"BUILD SUCCESS".to_vec(),
            stderr: b"".to_vec(),
            duration_ms: 5000,
            timed_out: false,
        };

        let op_result = BastionEnrichmentAdapter::map_command_result(&result);
        assert_eq!(op_result.exit_code, 0);
        assert_eq!(op_result.stdout, "BUILD SUCCESS");
        assert_eq!(op_result.stderr, "");
        assert_eq!(op_result.duration_ms, 5000);
        assert!(!op_result.timed_out);
    }
}
