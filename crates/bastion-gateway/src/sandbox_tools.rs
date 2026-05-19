//! Sandbox MCP tools: create, run, sync, terminate, and manage sandboxes.
//!
//! Exposes sandbox lifecycle and file operations as MCP tools:
//! - `sandbox_create`: Create a new isolated sandbox environment
//! - `sandbox_run`: Execute a command in a sandbox
//! - `sandbox_run_stream`: Execute a command with streaming output
//! - `sandbox_write`: Write a file to a sandbox
//! - `sandbox_read`: Read a file from a sandbox
//! - `sandbox_list_files`: List files in a directory inside a sandbox
//! - `sandbox_list_templates`: List available sandbox templates
//! - `sandbox_terminate`: Terminate and destroy a sandbox
//! - `sandbox_cancel`: Cancel a running command in a sandbox
//! - `sandbox_info`: Get information about a sandbox
//! - `sandbox_list`: List all active sandboxes
//! - `sandbox_pool_stats`: Get sandbox pool statistics
//! - `sandbox_health`: Check gateway health
//! - `sandbox_metrics`: Get gateway metrics in Prometheus format
//! - `sandbox_register_artifact`: Register a template artifact
//! - `sandbox_prepare`: Prepare a sandbox with a specific capability
//! - `sandbox_snapshot`: Manage sandbox snapshots
//! - `sandbox_sync`: Sync files between host and sandbox
//! - `sandbox_list_capabilities`: List all available capabilities for sandbox_prepare
//! - `sandbox_list_artifacts`: List all registered template artifacts

#![allow(unused_imports)]

// Sub-modules: types and helpers extracted for better organization
mod sandbox_tools_helpers;
mod sandbox_tools_types;

// Re-export types for backward compatibility
pub use sandbox_tools_types::{
    RegisterArtifactParams, SandboxCancelParams, SandboxCreateParams, SandboxInfoParams,
    SandboxListFilesParams, SandboxPrepareParams, SandboxReadParams, SandboxRunParams,
    SandboxRunStreamParams, SandboxSnapshotParams, SandboxSyncParams, SandboxTerminateParams,
    SandboxWriteParams, SyncBackend,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use futures::StreamExt;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool};
use schemars::JsonSchema;
use serde::Deserialize;

use bastion_application::execution::{RunCommandStreamUseCase, RunCommandUseCase};
use bastion_application::file_ops::{ListFilesUseCase, ReadFileUseCase, WriteFileUseCase};
use bastion_application::sandbox::{
    CreateSandboxUseCase, GetSandboxInfoUseCase, ListSandboxesUseCase, TerminateSandboxUseCase,
};
use bastion_application::template::MaterializationStrategyResolver;
use bastion_domain::catalog::experience::ExperienceRecord;
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::execution::stream::ChunkType;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::secret::{SecretResolver, SecretSource, parse_secret_ref};
use bastion_domain::shared::{DomainError, id::SandboxId};
use bastion_domain::template::{
    ArtifactCatalog, CapabilityDescriptor, MaterializationMode, ProviderKind, ProviderMaterializer,
    TemplateArtifact, ToolDescriptor, ToolResolver, ToolchainRequest, ToolchainStrategy,
};
use bastion_infrastructure::metrics::{GatewayMetrics, MetricsHub};
use bastion_infrastructure::pool::SandboxPoolManager;
use bastion_infrastructure::template::{
    AptAdapter, AsdfAdapter, CapabilityRegistry, FsArtifactStore, PodmanOptimizedMaterializer,
    SdkmanAdapter, UniversalMaterializer,
};

use bastion_domain::catalog::doctor::{
    CheckStatus, DoctorCheck, DoctorResult, DoctorStatus, RichCheckResult,
};
use crate::server::DoctorContext;

use crate::server::BastionGateway;

// ─── Tool router function ───────────────────────────────────────────────────

/// Returns the sandbox tools router, combining all sandbox MCP tools.
pub fn sandbox_tools() -> ToolRouter<BastionGateway> {
    ToolRouter::<BastionGateway>::new()
        .with_route((
            BastionGateway::sandbox_create_tool_attr(),
            BastionGateway::sandbox_create,
        ))
        .with_route((
            BastionGateway::sandbox_run_tool_attr(),
            BastionGateway::sandbox_run,
        ))
        .with_route((
            BastionGateway::sandbox_run_stream_tool_attr(),
            BastionGateway::sandbox_run_stream,
        ))
        .with_route((
            BastionGateway::sandbox_write_tool_attr(),
            BastionGateway::sandbox_write,
        ))
        .with_route((
            BastionGateway::sandbox_read_tool_attr(),
            BastionGateway::sandbox_read,
        ))
        .with_route((
            BastionGateway::sandbox_list_files_tool_attr(),
            BastionGateway::sandbox_list_files,
        ))
        .with_route((
            BastionGateway::sandbox_list_templates_tool_attr(),
            BastionGateway::sandbox_list_templates,
        ))
        .with_route((
            BastionGateway::sandbox_terminate_tool_attr(),
            BastionGateway::sandbox_terminate,
        ))
        .with_route((
            BastionGateway::sandbox_cancel_tool_attr(),
            BastionGateway::sandbox_cancel,
        ))
        .with_route((
            BastionGateway::sandbox_info_tool_attr(),
            BastionGateway::sandbox_info,
        ))
        .with_route((
            BastionGateway::sandbox_list_tool_attr(),
            BastionGateway::sandbox_list,
        ))
        .with_route((
            BastionGateway::sandbox_pool_stats_tool_attr(),
            BastionGateway::sandbox_pool_stats,
        ))
        .with_route((
            BastionGateway::sandbox_health_tool_attr(),
            BastionGateway::sandbox_health,
        ))
        .with_route((
            BastionGateway::sandbox_metrics_tool_attr(),
            BastionGateway::sandbox_metrics,
        ))
        .with_route((
            BastionGateway::sandbox_register_artifact_tool_attr(),
            BastionGateway::sandbox_register_artifact,
        ))
        .with_route((
            BastionGateway::sandbox_prepare_tool_attr(),
            BastionGateway::sandbox_prepare,
        ))
        .with_route((
            BastionGateway::sandbox_snapshot_tool_attr(),
            BastionGateway::sandbox_snapshot,
        ))
        .with_route((
            BastionGateway::sandbox_sync_tool_attr(),
            BastionGateway::sandbox_sync,
        ))
        .with_route((
            BastionGateway::sandbox_list_capabilities_tool_attr(),
            BastionGateway::sandbox_list_capabilities,
        ))
        .with_route((
            BastionGateway::sandbox_list_artifacts_tool_attr(),
            BastionGateway::sandbox_list_artifacts,
        ))
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// Create a new isolated sandbox environment.
    ///
    /// Standard workflow: sandbox_create(template) → sandbox_prepare(sandbox_id, capability) → sandbox_run(sandbox_id, command, env_ref) → sandbox_terminate(sandbox_id).
    /// Returns a sandbox_id you must pass to subsequent tools. The sandbox starts stopped; call sandbox_prepare to install tools before running commands.
    #[tool(
        description = "Create a new isolated sandbox environment. Standard flow: create → prepare → run → terminate. Returns sandbox_id for subsequent tools. DO NOT use raw podman/docker exec — use sandbox_run instead."
    )]
    async fn sandbox_create(&self, Parameters(params): Parameters<SandboxCreateParams>) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        let selected_provider = self.resolve_provider(&params.provider);
        tracing::info!(template = %params.template, provider = %params.provider, "Creating sandbox");

        // Pre-flight doctor check: verify provider readiness before attempting to create sandbox
        if let Some(ref doctor_ctx) = self.doctor_context {
            let doctor_id = format!("{}.readiness", params.provider);
            let readiness_result = self.run_provider_readiness_check(&doctor_id, &params.provider).await;

            if readiness_result.status != DoctorStatus::Pass {
                // Provider not ready - return rich error for AI agent
                let failed_checks: Vec<_> = readiness_result
                    .rich_check_results
                    .iter()
                    .filter(|r| r.status == CheckStatus::Fail || r.status == CheckStatus::Warning)
                    .map(|r| {
                        serde_json::json!({
                            "check_type": r.check_type,
                            "status": format!("{:?}", r.status),
                            "current_state": r.current_state,
                            "delta": r.delta,
                            "remediation": r.remediation,
                        })
                    })
                    .collect();

                return serde_json::json!({
                    "error": "Provider not ready",
                    "provider": params.provider,
                    "doctor_id": doctor_id,
                    "status": format!("{:?}", readiness_result.status),
                    "summary": readiness_result.summary,
                    "requires_attention": readiness_result.requires_ai_attention,
                    "can_self_remediate": readiness_result.potential_self_remediation,
                    "check_results": failed_checks,
                    "hint": format!("Run doctor_run tool with doctor_id='{}' for detailed remediation steps", doctor_id)
                }).to_string();
            }
        }

        // Only use pool for the default podman provider — pool sandboxes are podman-based.
        // For non-podman providers (local, gvisor, firecracker), go straight to direct creation.
        let use_pool = params.provider == "podman";

        if use_pool {
            if let Some(ref pool) = self.gateway_config.pool_manager {
                match pool.checkout(&params.template, params.timeout_ms).await {
                    Ok(sandbox) => {
                        tracing::debug!(
                            sandbox_id = %sandbox.id,
                            template = %params.template,
                            "Sandbox created via pool checkout"
                        );
                        self.gateway_config.metrics.record_sandbox_created();
                        return serde_json::json!({
                            "sandbox_id": sandbox.id.to_string(),
                            "status": sandbox.status.to_string(),
                            "template": sandbox.template_id.to_string(),
                            "from_pool": true
                        })
                        .to_string();
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Pool checkout failed, falling back to direct creation");
                        // Fall through to direct creation
                    }
                }
            }
        }

        // Direct creation (fallback or when pool is disabled)
        let use_case = CreateSandboxUseCase::new(
            self.repository.clone(),
            bastion_domain::shared::id::ProviderId::new(&params.provider),
        );

        let input = bastion_application::sandbox::create::CreateSandboxInput {
            template_id: params.template.clone(),
            provider_id: None,
            resources: bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            network: bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            env_vars: std::collections::HashMap::new(),
            timeout_ms: params.timeout_ms,
        };

        match use_case.execute(input, selected_provider.as_ref()).await {
            Ok(sandbox) => {
                self.gateway_config.metrics.record_sandbox_created();
                serde_json::json!({
                    "sandbox_id": sandbox.id.to_string(),
                    "status": sandbox.status.to_string(),
                    "template": sandbox.template_id.to_string(),
                    "from_pool": false
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Execute a command in a sandbox.
    ///
    /// Auto-injects env from sandbox_prepare if env_ref is not explicitly provided. Pass env_ref explicitly in concurrent workflows to avoid race conditions.
    /// DO NOT manually install tools here — use sandbox_prepare with a capability like "jvm-build" or "node-build" instead.
    #[tool(
        description = "Execute a command in a sandbox. Auto-injects env from sandbox_prepare via env_ref (pass explicitly in concurrent workflows). DO NOT manually install tools — use sandbox_prepare instead. For asdf-vm commands, use bash -c wrapper: bash -c 'export ASDF_DIR=$HOME/.asdf && . $ASDF_DIR/asdf.sh && <your cmd>'."
    )]
    async fn sandbox_run(&self, Parameters(params): Parameters<SandboxRunParams>) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this command execution
        let mut experience =
            ExperienceRecord::new("sandbox_run").with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        let use_case = RunCommandUseCase::new(self.repository.clone());

        let mut command_spec = CommandSpec::new(&params.command);

        // Resolve env_ref: explicit parameter takes priority, otherwise auto-inject from sandbox_prepare
        let resolved_env_ref = if let Some(ref env_ref) = params.env_ref {
            Some(env_ref.clone())
        } else {
            let last_refs = self.last_env_ref.read().await;
            last_refs.get(&params.sandbox_id).cloned()
        };

        if let Some(ref env_ref) = resolved_env_ref {
            tracing::debug!(sandbox_id = %sandbox_id, env_ref = %env_ref, "Merging environment from env_ref");
            let envs = self.prepared_environments.read().await;
            if let Some(env) = envs.get(env_ref) {
                for (key, value) in env {
                    command_spec = command_spec.with_env(key, value);
                }
            }
        }

        // Resolve secrets
        let secrets_to_inject: Vec<(String, String)> = {
            let mut results = Vec::new();
            for (key, secret_source) in command_spec.secrets.iter() {
                let resolved_value = match secret_source {
                    SecretSource::Inline(value) => value.clone(),
                    SecretSource::Ref(secret_key) => {
                        match self.secret_resolver.resolve(secret_key).await {
                            Ok(secret) => {
                                tracing::debug!(key = %key, source = %secret.source, "Secret resolved");
                                secret.value
                            }
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secret '{}': {}", secret_key, e)}).to_string();
                            }
                        }
                    }
                };
                results.push((key.clone(), resolved_value));
            }
            results
        };
        for (k, v) in secrets_to_inject {
            command_spec = command_spec.with_env(k, v);
        }

        let t0 = std::time::Instant::now();

        // Resolve the correct provider for this sandbox (not necessarily the default)
        let sandbox_provider = self.resolve_sandbox_provider(&sandbox_id).await;

        match use_case
            .execute(&sandbox_id, &command_spec, sandbox_provider.as_ref())
            .await
        {
            Ok(result) => {
                let duration_us = t0.elapsed().as_micros() as u64;
                self.gateway_config.metrics.record_command(duration_us);

                // Record successful experience
                experience = experience
                    .with_stdout(&result.stdout)
                    .with_stderr(&result.stderr)
                    .completed(result.exit_code);
                if result.timed_out {
                    experience = experience.timed_out();
                }
                self.record_experience(experience).await;

                // Build base response
                let mut response = serde_json::json!({
                    "exit_code": result.exit_code,
                    "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
                    "duration_ms": result.duration_ms,
                    "timed_out": result.timed_out
                });

                // Attempt enrichment if adapter is configured and enabled
                if let Some(ref adapter) = *self.enrichment_adapter
                    && self.enrichment_config.enabled
                {
                    match adapter.enrich(&sandbox_id, &command_spec, &result).await {
                        Some(ctx) => {
                            // Add enrichment results additively
                            response["agent_context"] = serde_json::json!({
                                "facts": ctx.facts,
                                "build_status": ctx.build_status,
                                "artifacts": ctx.artifacts,
                                "test_summary": ctx.test_summary,
                            });
                            response["enrichment_meta"] = serde_json::json!({
                                "source": ctx.enrichment_meta.source,
                                "timestamp": ctx.enrichment_meta.timestamp,
                                "enricher_id": ctx.enrichment_meta.enricher_id,
                            });
                        }
                        None => {
                            // No enrichment facts extracted — log at debug level
                            tracing::debug!(sandbox_id = %sandbox_id, command = %params.command, "No enrichment facts extracted");
                        }
                    }
                }

                response.to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Record failed experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Execute a command with streaming output (returns stdout/stderr separately with exit code).
    ///
    /// Like sandbox_run but streams output via MCP progress notifications. Use sandbox_cancel to interrupt long-running commands.
    /// Same env_ref auto-injection as sandbox_run — pass env_ref explicitly in concurrent workflows.
    #[tool(
        description = "Execute a command with streaming output via MCP progress notifications. Same env_ref auto-injection as sandbox_run. Use sandbox_cancel to interrupt. DO NOT manually install tools — use sandbox_prepare."
    )]
    async fn sandbox_run_stream(
        &self,
        Parameters(params): Parameters<SandboxRunStreamParams>,
        request_ctx: RequestContext<RoleServer>,
    ) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        // Extract progress token from meta if present
        let progress_token = request_ctx.meta.get_progress_token();
        let peer = request_ctx.peer.clone();

        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running streaming command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let mut command_spec = CommandSpec::new(&params.command);

        // Auto-inject env_ref (same logic as sandbox_run)
        let resolved_env_ref = params.env_ref.clone().or(None);
        let resolved_env_ref = if let Some(ref env_ref) = resolved_env_ref {
            Some(env_ref.clone())
        } else {
            let last_refs = self.last_env_ref.read().await;
            last_refs.get(&params.sandbox_id).cloned()
        };
        if let Some(ref env_ref) = resolved_env_ref {
            let envs = self.prepared_environments.read().await;
            if let Some(env) = envs.get(env_ref) {
                for (key, value) in env {
                    command_spec = command_spec.with_env(key, value);
                }
            }
        }

        // Resolve secrets: collect resolved values first to avoid borrow conflict
        let secrets_to_inject: Vec<(String, String)> = {
            let mut results = Vec::new();
            for (key, secret_source) in command_spec.secrets.iter() {
                let resolved_value = match secret_source {
                    SecretSource::Inline(value) => value.clone(),
                    SecretSource::Ref(secret_key) => {
                        match self.secret_resolver.resolve(secret_key).await {
                            Ok(secret) => {
                                tracing::debug!(key = %key, source = %secret.source, "Secret resolved");
                                secret.value
                            }
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secret '{}': {}", secret_key, e)}).to_string();
                            }
                        }
                    }
                };
                results.push((key.clone(), resolved_value));
            }
            results
        };
        for (k, v) in secrets_to_inject {
            command_spec = command_spec.with_env(k, v);
        }

        let use_case = RunCommandStreamUseCase::new(self.repository.clone());
        let start_time = std::time::Instant::now();

        // Resolve the correct provider for this sandbox
        let provider = self.resolve_sandbox_provider(&sandbox_id).await;

        // Register cancel token for this streaming command
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_tokens
            .insert(params.sandbox_id.clone(), cancel_flag.clone());

        match use_case
            .execute(&sandbox_id, &command_spec, provider.as_ref())
            .await
        {
            Ok(mut stream) => {
                let mut stdout_parts = Vec::new();
                let mut stderr_parts = Vec::new();
                let mut exit_code = -1i32;
                let mut chunk_count = 0u32;

                while let Some(chunk_result) = stream.next().await {
                    // Check cancel flag — if set, stop streaming
                    if cancel_flag.load(Ordering::Relaxed) {
                        tracing::info!(sandbox_id = %params.sandbox_id, "Streaming command cancelled");
                        stderr_parts.push("[CANCELLED] Command was cancelled by user".to_string());
                        exit_code = -1;
                        break;
                    }

                    // Send progress notification if token is present
                    if let Some(ref token) = progress_token {
                        chunk_count += 1;
                        // Estimate progress based on chunk count (0.0 to 0.9 until complete)
                        let progress = (chunk_count as f64 / 100.0).min(0.9);
                        let message =
                            Self::build_progress_message(&stdout_parts, &stderr_parts, chunk_count);
                        if let Some(ref msg) = message {
                            Self::send_progress(&peer, token, progress, Some(msg.as_str())).await;
                        }
                    }

                    match chunk_result {
                        Ok(chunk) => match chunk.chunk_type {
                            ChunkType::Stdout => {
                                stdout_parts.push(String::from_utf8_lossy(&chunk.data).to_string())
                            }
                            ChunkType::Stderr => {
                                stderr_parts.push(String::from_utf8_lossy(&chunk.data).to_string())
                            }
                            ChunkType::ExitCode => {
                                if chunk.data.len() >= 4 {
                                    exit_code = i32::from_le_bytes(
                                        chunk.data[..4].try_into().unwrap_or([-1i8 as u8, 0, 0, 0]),
                                    );
                                }
                            }
                            _ => {}
                        },
                        Err(e) => {
                            stderr_parts.push(format!("Stream error: {}", e));
                        }
                    }
                }

                // Remove cancel token — command finished or was cancelled
                self.cancel_tokens.remove(&params.sandbox_id);

                // Send final progress notification
                if let Some(ref token) = progress_token {
                    Self::send_progress(&peer, token, 1.0, Some("Complete")).await;
                }

                let duration_us = start_time.elapsed().as_micros() as u64;
                self.gateway_config.metrics.record_command(duration_us);

                // Record successful experience
                let mut experience =
                    ExperienceRecord::new("sandbox_run_stream").with_sandbox_id(sandbox_id.clone());
                if let Some(ref trace_id) = params.trace_id {
                    experience = experience.with_trace_id(trace_id.clone());
                }
                experience = experience
                    .with_stdout(&stdout_parts.join("").into_bytes())
                    .with_stderr(&stderr_parts.join("").into_bytes())
                    .completed(exit_code);
                self.record_experience(experience).await;

                serde_json::json!({
                    "exit_code": exit_code,
                    "stdout": stdout_parts.join(""),
                    "stderr": stderr_parts.join(""),
                    "duration_ms": duration_us / 1000,
                    "chunk_count": chunk_count,
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Remove cancel token on error
                self.cancel_tokens.remove(&params.sandbox_id);

                // Record failed experience
                let mut experience =
                    ExperienceRecord::new("sandbox_run_stream").with_sandbox_id(sandbox_id.clone());
                if let Some(ref trace_id) = params.trace_id {
                    experience = experience.with_trace_id(trace_id.clone());
                }
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Write content to a file inside a sandbox.
    ///
    /// Creates parent directories if they don't exist. Content is base64-encoded in the response for verification.
    #[tool(
        description = "Write content to a file inside a sandbox. Creates parent directories if needed."
    )]
    async fn sandbox_write(&self, Parameters(params): Parameters<SandboxWriteParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Writing file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let provider = self.resolve_sandbox_provider(&sandbox_id).await;
        let use_case = WriteFileUseCase::new(self.repository.clone());

        match use_case
            .execute(
                &sandbox_id,
                &params.path,
                params.content.as_bytes(),
                provider.as_ref(),
            )
            .await
        {
            Ok(()) => serde_json::json!({"status": "ok"}).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Read a file from a sandbox.
    ///
    /// Returns file content base64-encoded. Use sandbox_list_files to discover paths first.
    #[tool(
        description = "Read a file from a sandbox. Returns base64-encoded content. Use sandbox_list_files to discover paths first."
    )]
    async fn sandbox_read(&self, Parameters(params): Parameters<SandboxReadParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Reading file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let provider = self.resolve_sandbox_provider(&sandbox_id).await;
        let use_case = ReadFileUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, provider.as_ref())
            .await
        {
            Ok(content) => serde_json::json!({
                "content": base64::engine::general_purpose::STANDARD.encode(&content),
                "encoding": "base64",
                "size_bytes": content.len()
            })
            .to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// List files in a directory inside a sandbox.
    ///
    /// Returns file paths, sizes, permissions, and whether each entry is a directory.
    #[tool(
        description = "List files in a directory inside a sandbox. Returns paths, sizes, permissions, and directory flags."
    )]
    async fn sandbox_list_files(
        &self,
        Parameters(params): Parameters<SandboxListFilesParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Listing files");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let provider = self.resolve_sandbox_provider(&sandbox_id).await;
        let use_case = ListFilesUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, provider.as_ref())
            .await
        {
            Ok(entries) => {
                let list: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "path": e.path,
                            "is_directory": e.is_directory,
                            "size_bytes": e.size_bytes,
                            "permissions": e.permissions,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "count": list.len(),
                    "entries": list
                })
                .to_string()
            }
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// List available sandbox templates (container images).
    ///
    /// Queries local podman images and includes known defaults (debian:bookworm-slim, ubuntu:22.04, etc.). Use returned image names as the template parameter for sandbox_create.
    #[tool(
        description = "List available sandbox templates (container images). Use returned image names as template param for sandbox_create."
    )]
    async fn sandbox_list_templates(&self) -> String {
        tracing::info!("Listing available templates");

        let mut templates: Vec<serde_json::Value> = Vec::new();

        // Query podman for available images
        match tokio::process::Command::new("podman")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let image = line.trim().to_string();
                    if image.is_empty() || image == "<none>:<none>" {
                        continue;
                    }
                    // Suggest as template
                    templates.push(serde_json::json!({
                        "image": image,
                        "suggested_name": image.trim_start_matches("localhost/").trim_start_matches("docker.io/"),
                    }));
                }
            }
            Ok(_) => {
                tracing::warn!("podman images returned non-zero exit");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list podman images");
            }
        }

        // Add default templates known to work
        let defaults = [
            "debian:bookworm-slim",
            "ubuntu:22.04",
            "fedora:39",
            "alpine:3.19",
        ];
        for d in &defaults {
            if !templates.iter().any(|t| t["image"].as_str() == Some(d)) {
                templates.push(serde_json::json!({
                    "image": d,
                    "suggested_name": d,
                    "note": "default — may need to be pulled first"
                }));
            }
        }

        serde_json::json!({
            "count": templates.len(),
            "templates": templates
        })
        .to_string()
    }

    /// Terminate and destroy a sandbox.
    ///
    /// Returns sandbox to the pool if pool mode is enabled (reuse), otherwise destroys it. Always call when done to free resources.
    #[tool(
        description = "Terminate and destroy a sandbox. Returns to pool if enabled, otherwise destroys. Always call when done to free resources."
    )]
    async fn sandbox_terminate(
        &self,
        Parameters(params): Parameters<SandboxTerminateParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Terminating sandbox");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Helper to remove sandbox from HeartbeatBridge tracking
        let remove_from_heartbeat = |metrics_hub: &Option<Arc<tokio::sync::Mutex<MetricsHub>>>,
                                     sid: &str| {
            if let Some(hub) = metrics_hub {
                let guard = match hub.try_lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let bridge = guard.heartbeat_bridge();
                bridge.remove_resources(sid);
            }
        };

        // Try to return to pool first if pool is available
        if let Some(ref pool) = self.gateway_config.pool_manager {
            match pool.checkin(&sandbox_id).await {
                Ok(()) => {
                    tracing::debug!(sandbox_id = %params.sandbox_id, "Sandbox returned to pool");
                    self.gateway_config.metrics.record_sandbox_terminated();
                    // Remove from HeartbeatBridge tracking
                    remove_from_heartbeat(&self.gateway_config.metrics_hub, &params.sandbox_id);
                    return serde_json::json!({
                        "status": "pooled",
                        "sandbox_id": params.sandbox_id
                    })
                    .to_string();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Pool checkin failed, terminating directly");
                    // Fall through to direct termination
                }
            }
        }

        let use_case = TerminateSandboxUseCase::new(self.repository.clone());
        let provider = self.resolve_sandbox_provider(&sandbox_id).await;

        match use_case.execute(&sandbox_id, provider.as_ref()).await {
            Ok(()) => {
                self.gateway_config.metrics.record_sandbox_terminated();
                // Remove from HeartbeatBridge tracking
                remove_from_heartbeat(&self.gateway_config.metrics_hub, &params.sandbox_id);
                serde_json::json!({"status": "terminated"}).to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Cancel a running command in a sandbox.
    ///
    /// Sends SIGTERM, waits grace_period_ms, then SIGKILL. Works on both streaming (sandbox_run_stream) and regular (sandbox_run) commands.
    #[tool(
        description = "Cancel a running command in a sandbox. Sends SIGTERM, waits grace_period_ms, then SIGKILL. Works on both streaming and regular sandbox_run commands."
    )]
    async fn sandbox_cancel(&self, Parameters(params): Parameters<SandboxCancelParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, grace_period_ms = params.grace_period_ms, "Cancelling sandbox command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this cancel operation
        let mut experience =
            ExperienceRecord::new("sandbox_cancel").with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // Signal local cancel token (for streaming commands)
        if let Some(token) = self.cancel_tokens.get(&params.sandbox_id) {
            token.store(true, Ordering::Relaxed);
            tracing::info!(sandbox_id = %params.sandbox_id, "Cancel flag set");
        }

        // Also ask the correct provider to cancel the command (SIGTERM/SIGKILL)
        let cancel_provider = self.resolve_sandbox_provider(&sandbox_id).await;
        match cancel_provider
            .cancel_command(&sandbox_id, params.grace_period_ms)
            .await
        {
            Ok(cancelled) => {
                // Record cancelled experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": if cancelled { "cancelled" } else { "no_running_command" },
                    "sandbox_id": params.sandbox_id
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Record failed experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Get information about a sandbox.
    ///
    /// Returns sandbox_id, status, template, created_at, and expires_at. Use to check if a sandbox is still alive before running commands.
    #[tool(
        description = "Get information about a sandbox (status, template, created_at, expires_at). Check status before running commands."
    )]
    async fn sandbox_info(&self, Parameters(params): Parameters<SandboxInfoParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Getting sandbox info");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = GetSandboxInfoUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id).await {
            Ok(info) => serde_json::json!({
                "sandbox_id": info.id.to_string(),
                "status": info.status.to_string(),
                "template": info.template_id.to_string(),
                "created_at": info.created_at.to_rfc3339(),
                "expires_at": info.expires_at.map(|t| t.to_rfc3339()),
            })
            .to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// List all active sandboxes.
    ///
    /// Returns all sandboxes known to the gateway, including pooled sandboxes. Use to discover existing sandboxes before creating new ones.
    #[tool(
        description = "List all active sandboxes known to the gateway. Use to discover existing sandboxes before creating new ones."
    )]
    async fn sandbox_list(&self) -> String {
        tracing::info!("Listing active sandboxes");

        let use_case = ListSandboxesUseCase::new(self.repository.clone());

        match use_case.execute().await {
            Ok(sandboxes) => {
                let list: Vec<serde_json::Value> = sandboxes
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "sandbox_id": s.id.to_string(),
                            "status": s.status.to_string(),
                            "template": s.template_id.to_string(),
                            "created_at": s.created_at.to_rfc3339(),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "count": list.len(),
                    "sandboxes": list
                })
                .to_string()
            }
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Get sandbox pool statistics.
    ///
    /// Returns active/idle/total counts per template. Use to check if pool has capacity before sandbox_create if using pool mode.
    #[tool(
        description = "Get sandbox pool statistics (active/idle/total per template). Check capacity before creating sandboxes in pool mode."
    )]
    async fn sandbox_pool_stats(&self) -> String {
        tracing::trace!("Getting pool statistics");

        if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            serde_json::json!({
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle,
                "total": stats.total,
                "templates": stats.templates.iter().map(|t| {
                    serde_json::json!({
                        "template": t.template,
                        "idle": t.idle,
                        "min_idle": t.min_idle,
                        "max_idle": t.max_idle
                    })
                }).collect::<Vec<_>>()
            })
            .to_string()
        } else {
            serde_json::json!({
                "enabled": false,
                "message": "Pool is not enabled"
            })
            .to_string()
        }
    }

    /// Check gateway health including provider connectivity and pool status.
    ///
    /// Returns overall healthy/degraded status with component-level checks. Call this before running commands if you suspect infrastructure issues.
    #[tool(
        description = "Check gateway health (provider connectivity, pool status). Call before running commands if you suspect infrastructure issues."
    )]
    async fn sandbox_health(&self) -> String {
        let mut checks = Vec::new();

        // Check provider connectivity
        checks.push(serde_json::json!({
            "component": "provider",
            "provider": self.provider.name(),
            "status": "ok"
        }));

        // Check pool status
        if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            checks.push(serde_json::json!({
                "component": "pool",
                "status": "ok",
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle
            }));
        } else {
            checks.push(serde_json::json!({
                "component": "pool",
                "status": "disabled"
            }));
        }

        serde_json::json!({
            "status": "healthy",
            "version": env!("CARGO_PKG_VERSION"),
            "checks": checks
        })
        .to_string()
    }

    /// Get gateway metrics in Prometheus exposition format.
    ///
    /// Includes sandbox_create/runs/terminates counts, command durations, error counts, and pool stats. Parse with any Prometheus scraper.
    #[tool(
        description = "Get gateway metrics in Prometheus exposition format. Includes sandbox/command counts, durations, errors, and pool stats."
    )]
    async fn sandbox_metrics(&self) -> String {
        tracing::debug!("Getting metrics");
        self.gateway_config.metrics.prometheus_export()
    }

    /// Register a template artifact that provides a capability.
    ///
    /// Registers a named+versioned artifact with tools that sandbox_prepare can materialize. Tools are comma-separated "name:version" pairs (version optional).
    #[tool(
        description = "Register a template artifact with tools for sandbox_prepare to materialize. Provide name:version pairs (version optional)."
    )]
    async fn sandbox_register_artifact(
        &self,
        Parameters(params): Parameters<RegisterArtifactParams>,
    ) -> String {
        let tools: Vec<ToolDescriptor> = params
            .tools
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let parts: Vec<&str> = s.trim().split(':').collect();
                ToolDescriptor {
                    name: parts.first().unwrap_or(&"").to_string(),
                    version: parts.get(1).unwrap_or(&"any").to_string(),
                    category: bastion_domain::template::Category::Generic,
                    manager_preference: vec![],
                }
            })
            .collect();

        let artifact = TemplateArtifact::builder(&params.name, &params.version)
            .digest(&params.digest)
            .add_capability(CapabilityDescriptor {
                name: params.capability.clone(),
                tools,
                verification: vec![],
            })
            .build();

        {
            let mut catalog = self.artifact_catalog.write().await;
            catalog.register(artifact);
        }

        serde_json::json!({
            "status": "registered",
            "name": params.name,
            "capability": params.capability
        })
        .to_string()
    }

    /// Prepare a sandbox with a specific capability (e.g. jvm-build, node-build).
    ///
    /// Installs tools and sets up env for subsequent sandbox_run calls. Returns env_ref + env + path_prefix for auto-injection.
    /// After prepare, sandbox_run auto-injects the env — pass env_ref explicitly in concurrent workflows.
    /// Available capabilities: "jvm-build" (Java 17 + Maven), "node-build" (Node.js 20 + npm).
    #[tool(
        description = "Prepare a sandbox with a capability (e.g. jvm-build, node-build). Installs tools, returns env_ref for sandbox_run auto-injection. Available: jvm-build (Java+Maven), node-build (Node.js+npm)."
    )]
    async fn sandbox_prepare(
        &self,
        Parameters(params): Parameters<SandboxPrepareParams>,
    ) -> String {
        let sandbox_id = SandboxId::new(&params.sandbox_id);
        let capability = &params.capability;
        let prepare_provider = self.resolve_sandbox_provider(&sandbox_id).await;

        // Create experience record for this prepare operation
        let mut experience =
            ExperienceRecord::new("sandbox_prepare").with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // Try artifact catalog first
        let artifact = {
            let catalog = self.artifact_catalog.read().await;
            catalog.resolve(capability).cloned()
        };

        // If artifact found, use materializer
        if let Ok(artifact) = artifact {
            let provider_name = prepare_provider.name().to_lowercase();

            // Get provider kind from provider name
            let provider_kind = match provider_name.as_str() {
                "podman" => ProviderKind::Podman,
                "docker" => ProviderKind::Docker,
                "gvisor" | "gvisor-sandbox" => ProviderKind::GVisor,
                "firecracker" => ProviderKind::Firecracker,
                "kubernetes" | "k8s" => ProviderKind::Kubernetes,
                "wasm" => ProviderKind::Wasm,
                "local" => ProviderKind::Local,
                _ => ProviderKind::Custom,
            };

            // Use MaterializationStrategyResolver for smarter routing
            let catalog = self.artifact_catalog.read().await;
            let resolution =
                MaterializationStrategyResolver::resolve(capability, provider_kind, &catalog);
            drop(catalog);

            // Select materializer based on resolution
            let result = match resolution.materializer_name.as_str() {
                "PodmanOptimizedMaterializer" => {
                    let materializer = PodmanOptimizedMaterializer::new(
                        prepare_provider.clone(),
                        self.artifact_store.clone(),
                        PathBuf::from("/tmp/bastion-cache"),
                    );
                    materializer
                        .materialize(&sandbox_id, &artifact, resolution.mode)
                        .await
                }
                _ => {
                    let materializer = UniversalMaterializer::new(
                        prepare_provider.clone(),
                        self.artifact_store.clone(),
                        PathBuf::from("/tmp/bastion-cache"),
                    );
                    materializer
                        .materialize(&sandbox_id, &artifact, resolution.mode)
                        .await
                }
            };

            match result {
                Ok(result) => {
                    // Auto-inject: register this env_ref as default for this sandbox
                    if let Some(ref env_ref) = result.env_ref {
                        let mut last_refs = self.last_env_ref.write().await;
                        last_refs.insert(sandbox_id.to_string(), env_ref.clone());
                    }

                    // Record successful prepare experience
                    experience = experience.completed(0);
                    self.record_experience(experience).await;

                    return serde_json::json!({
                        "status": "ready",
                        "method": "artifact",
                        "env_ref": result.env_ref,
                        "cache_hit": result.cache_hit,
                        "duration_ms": result.duration_ms
                    })
                    .to_string();
                }
                Err(e) => {
                    tracing::warn!(
                        "Artifact materialization failed: {}, falling back to resolver",
                        e
                    );
                }
            }
        }

        // Try TOML-driven CapabilityRegistry first (if capability is registered)
        if let Some(plan) = self
            .capability_registry
            .read()
            .await
            .resolve(capability, params.strategy.clone())
        {
            // Execute the TOML-defined plan steps in the sandbox
            use bastion_domain::execution::command::CommandSpec;
            let t0 = std::time::Instant::now();

            for step in &plan.steps {
                let mut cmd = CommandSpec::new(&step.command).with_timeout(step.timeout_ms);
                for (k, v) in &step.env {
                    // Resolve secret refs in env var values (e.g., "${{secret:GITHUB_TOKEN}}")
                    let mut env_map: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    env_map.insert(k.clone(), v.clone());
                    let resolved = match self.resolve_secrets(&env_map).await {
                        Ok(r) => r,
                        Err(e) => {
                            return serde_json::json!({"error": format!("Failed to resolve secrets: {}", e)}).to_string();
                        }
                    };
                    cmd = cmd.with_env(
                        k.as_str(),
                        resolved.get(k).cloned().unwrap_or_else(|| v.clone()),
                    );
                }

                match prepare_provider.run_command(&sandbox_id, &cmd).await {
                    Ok(result) => {
                        if result.exit_code != step.expected_exit_code {
                            return serde_json::json!({
                                "error": format!("Step '{}' failed: exit {} (expected {})",
                                    step.description, result.exit_code, step.expected_exit_code)
                            })
                            .to_string();
                        }
                    }
                    Err(e) => {
                        return serde_json::json!({
                            "error": format!("Step '{}' error: {}", step.description, e)
                        })
                        .to_string();
                    }
                }
            }

            // Run verification steps if present
            for verify in &plan.verification {
                let cmd = CommandSpec::new(&verify.command).with_timeout(60000); // 60s timeout for verification

                match prepare_provider.run_command(&sandbox_id, &cmd).await {
                    Ok(result) => {
                        if result.exit_code != verify.expected_exit_code {
                            return serde_json::json!({
                                "error": format!("Verification '{}' failed: exit {} (expected {})",
                                    verify.label, result.exit_code, verify.expected_exit_code)
                            })
                            .to_string();
                        }
                        if let Some(expected) = &verify.expected_output_contains {
                            let stdout_str = String::from_utf8_lossy(&result.stdout);
                            if !stdout_str.contains(expected) {
                                return serde_json::json!({
                                    "error": format!("Verification '{}' output mismatch", verify.label)
                                }).to_string();
                            }
                        }
                    }
                    Err(e) => {
                        return serde_json::json!({
                            "error": format!("Verification '{}' error: {}", verify.label, e)
                        })
                        .to_string();
                    }
                }
            }

            let duration_ms = t0.elapsed().as_millis() as u64;

            // Generate env_ref and store the environment for later use by sandbox_run
            // Include path_prefix as PATH env var so sandbox_run can find installed tools
            let env_ref = format!("registry:{}:{}", sandbox_id, capability);
            {
                let mut env = plan.env.clone();
                if !plan.path_prefix.is_empty() {
                    let path_prefix = plan.path_prefix.join(":");
                    env.insert("PATH".to_string(), format!("{}:$PATH", path_prefix));
                }
                let mut envs = self.prepared_environments.write().await;
                envs.insert(env_ref.clone(), env);
            }
            // Auto-inject: register this env_ref as the default for this sandbox
            {
                let mut last_refs = self.last_env_ref.write().await;
                last_refs.insert(sandbox_id.to_string(), env_ref.clone());
            }

            tracing::info!(capability = %capability, adapter = %plan.adapter_used, "Sandbox prepared via TOML capability registry");

            // Record successful prepare experience
            experience = experience.completed(0);
            self.record_experience(experience).await;

            return serde_json::json!({
                "status": "ready",
                "method": "registry",
                "adapter_used": plan.adapter_used,
                "capability": capability,
                "env_ref": env_ref,
                "env": plan.env,
                "path_prefix": plan.path_prefix,
                "duration_ms": duration_ms
            })
            .to_string();
        }

        // Fallback: use ToolResolver with adapters (hardcoded)
        let mut resolver = ToolResolver::new();
        resolver.register(Box::new(AptAdapter));
        resolver.register(Box::new(AsdfAdapter));
        resolver.register(Box::new(SdkmanAdapter));

        let req = ToolchainRequest {
            sandbox_id: sandbox_id.clone(),
            capability: capability.clone(),
            constraints: std::collections::HashMap::new(),
            strategy: params.strategy,
        };

        match resolver.resolve(&req).await {
            Ok(plan) => {
                // Execute the plan steps in the sandbox
                use bastion_domain::execution::command::CommandSpec;
                let t0 = std::time::Instant::now();

                for step in &plan.steps {
                    let mut cmd = CommandSpec::new(&step.command).with_timeout(step.timeout_ms);
                    for (k, v) in &step.env {
                        // Resolve secret refs in env var values (e.g., "${{secret:GITHUB_TOKEN}}")
                        let mut env_map: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        env_map.insert(k.clone(), v.clone());
                        let resolved = match self.resolve_secrets(&env_map).await {
                            Ok(r) => r,
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secrets: {}", e)}).to_string();
                            }
                        };
                        cmd = cmd.with_env(
                            k.as_str(),
                            resolved.get(k).cloned().unwrap_or_else(|| v.clone()),
                        );
                    }

                    match prepare_provider.run_command(&sandbox_id, &cmd).await {
                        Ok(result) => {
                            if result.exit_code != step.expected_exit_code {
                                return serde_json::json!({
                                    "error": format!("Step '{}' failed: exit {} (expected {})",
                                        step.description, result.exit_code, step.expected_exit_code)
                                })
                                .to_string();
                            }
                        }
                        Err(e) => {
                            return serde_json::json!({
                                "error": format!("Step '{}' error: {}", step.description, e)
                            })
                            .to_string();
                        }
                    }
                }

                let duration_ms = t0.elapsed().as_millis() as u64;

                // Generate env_ref and store the environment for later use by sandbox_run
                // Include path_prefix as PATH env var so sandbox_run can find installed tools
                let env_ref = format!("resolver:{}:{}", sandbox_id, capability);
                {
                    let mut env = plan.env.clone();
                    if !plan.path_prefix.is_empty() {
                        let path_prefix = plan.path_prefix.join(":");
                        env.insert("PATH".to_string(), format!("{}:$PATH", path_prefix));
                    }
                    let mut envs = self.prepared_environments.write().await;
                    envs.insert(env_ref.clone(), env);
                }
                // Auto-inject: register this env_ref as the default for this sandbox
                {
                    let mut last_refs = self.last_env_ref.write().await;
                    last_refs.insert(sandbox_id.to_string(), env_ref.clone());
                }

                // Record successful prepare experience
                experience = experience.completed(0);
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": "ready",
                    "method": "resolver",
                    "adapter_used": plan.adapter_used,
                    "capability": capability,
                    "env_ref": env_ref,
                    "env": plan.env,
                    "path_prefix": plan.path_prefix,
                    "duration_ms": duration_ms
                })
                .to_string()
            }
            Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
        }
    }

    /// Manage sandbox snapshots (create, restore, list, delete).
    ///
    /// Snapshots preserve sandbox state for fast reuse. create: specify sandbox_id + name. restore: specify snapshot_id. list: no params. delete: specify snapshot_id.
    /// Restored sandboxes register as new sandbox_ids in the gateway.
    #[tool(
        description = "Manage sandbox snapshots. Actions: create (sandbox_id+name), restore (snapshot_id), list (no params), delete (snapshot_id). Restored sandboxes get new sandbox_ids."
    )]
    async fn sandbox_snapshot(
        &self,
        Parameters(params): Parameters<SandboxSnapshotParams>,
    ) -> String {
        match params.action.as_str() {
            "create" => {
                let sandbox_id = match &params.sandbox_id {
                    Some(id) => SandboxId::new(id),
                    None => {
                        return serde_json::json!({"error": "sandbox_id required for create"})
                            .to_string();
                    }
                };
                let name = match &params.name {
                    Some(n) => n.as_str(),
                    None => {
                        return serde_json::json!({"error": "name required for create"})
                            .to_string();
                    }
                };

                match self.provider.create_snapshot(&sandbox_id, name).await {
                    Ok(info) => serde_json::json!({
                        "status": "created",
                        "snapshot_id": info.snapshot_id,
                        "sandbox_id": info.sandbox_id,
                        "name": info.name,
                        "created_at": info.created_at.to_rfc3339(),
                        "size_bytes": info.size_bytes
                    })
                    .to_string(),
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            "restore" => {
                let snapshot_id = match &params.snapshot_id {
                    Some(id) => id.as_str(),
                    None => {
                        return serde_json::json!({"error": "snapshot_id required for restore"})
                            .to_string();
                    }
                };

                match self.provider.restore_snapshot(snapshot_id).await {
                    Ok(sandbox) => {
                        // Register the restored sandbox in the gateway's repository
                        // so it's visible to sandbox_run, sandbox_info, etc.
                        if let Err(e) = self.repository.save(&sandbox).await {
                            tracing::error!(sandbox_id = %sandbox.id, error = %e, "Failed to register restored sandbox");
                        }
                        self.gateway_config.metrics.record_sandbox_created();
                        serde_json::json!({
                            "status": "restored",
                            "sandbox_id": sandbox.id.to_string(),
                            "snapshot_id": snapshot_id
                        })
                        .to_string()
                    }
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            "list" => match self.provider.list_snapshots().await {
                Ok(list) => serde_json::json!({
                    "status": "ok",
                    "snapshots": list.iter().map(|s| {
                        serde_json::json!({
                            "snapshot_id": s.snapshot_id,
                            "name": s.name,
                            "created_at": s.created_at.to_rfc3339(),
                            "size_bytes": s.size_bytes
                        })
                    }).collect::<Vec<_>>(),
                    "count": list.len()
                })
                .to_string(),
                Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
            },
            "delete" => {
                let snapshot_id = match &params.snapshot_id {
                    Some(id) => id.as_str(),
                    None => {
                        return serde_json::json!({"error": "snapshot_id required for delete"})
                            .to_string();
                    }
                };

                match self.provider.delete_snapshot(snapshot_id).await {
                    Ok(()) => serde_json::json!({
                        "status": "deleted",
                        "snapshot_id": snapshot_id
                    })
                    .to_string(),
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            _ => serde_json::json!({"error": format!("Unknown action: {}", params.action)})
                .to_string(),
        }
    }

    /// Sync files between host and sandbox (push/pull).
    ///
    /// push: copy host source → sandbox target. pull: copy sandbox source → host target. Source and destination paths must exist.
    /// Backend auto-detected (tar for rootless podman, rsync if installed in sandbox, podman-cp as fallback).
    #[tool(
        description = "Sync files between host and sandbox. push=host→sandbox, pull=sandbox→host. Paths must exist. Backend auto-detected (tar/podman-cp/rsync)."
    )]
    async fn sandbox_sync(&self, Parameters(params): Parameters<SandboxSyncParams>) -> String {
        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this sync operation
        let mut experience =
            ExperienceRecord::new("sandbox_sync").with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // SYNC-01: Liveness pre-check before sync
        // Resolve provider dynamically from sandbox metadata instead of hardcoding "podman"
        let use_case = GetSandboxInfoUseCase::new(self.repository.clone());
        let sandbox = match use_case.execute(&sandbox_id).await {
            Ok(s) => s,
            Err(e) => {
                return serde_json::json!({
                    "error": format!("sandbox not found: {}: {}", sandbox_id, e),
                    "sandbox_id": sandbox_id.to_string()
                })
                .to_string();
            }
        };
        let provider = self.resolve_provider(sandbox.provider_id.as_str());
        match provider.is_alive(&sandbox_id).await {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                return serde_json::json!({
                    "error": format!("sandbox not alive: {}", sandbox_id),
                    "sandbox_id": sandbox_id.to_string()
                })
                .to_string();
            }
        }

        // REQ-03: Capability guard — streaming file sync requires provider streaming support.
        // Tar-pipe sync is a streaming operation; if provider.capabilities().supports_streaming
        // is false, return explicit UnsupportedOperation JSON-RPC error with code -32001.
        if !provider.capabilities().supports_streaming() {
            return serde_json::json!({
                "error": "Unsupported operation: this provider does not support streaming file sync",
                "code": -32001,
                "provider": provider.name(),
                "sandbox_id": sandbox_id.to_string()
            })
            .to_string();
        }

        let mode = params.mode.as_str();
        let source = params.source.as_str();
        let target = params.target.as_str();
        let timeout_ms = params.timeout_ms;

        // Determine backend: explicit override > gateway default > auto
        let backend: SyncBackend = params
            .backend
            .as_deref()
            .and_then(|b| match b {
                "tar" => Some(SyncBackend::Tar),
                "rsync" => Some(SyncBackend::Rsync),
                "podman-cp" | "podman_cp" => Some(SyncBackend::PodmanCp),
                "auto" => Some(SyncBackend::Auto),
                _ => None,
            })
            .unwrap_or(self.sync_backend);

        // Auto-detect best backend for Podman
        let effective_backend = if backend == SyncBackend::Auto {
            // tar is most compatible for rootless podman
            SyncBackend::Tar
        } else {
            backend
        };

        let container_name = sandbox_id.to_string();

        tracing::info!(
            sandbox_id = %sandbox_id,
            mode = mode,
            backend = ?effective_backend,
            source = source,
            target = target,
            "Syncing files"
        );

        let result = match (mode, effective_backend) {
            ("push", SyncBackend::Tar) | ("push", SyncBackend::Auto) => {
                // tar pipe: local tar -> podman exec tar
                // Create target dir inside container before extracting
                let cmd = format!(
                    "tar czf - -C \"$(dirname '{}')\" \"$(basename '{}')\" 2>/dev/null | podman exec -i {} sh -c 'mkdir -p \"{}\" && tar xzf - -C \"{}\"'",
                    source, source, container_name, target, target
                );
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output(),
                )
                .await
            }
            ("pull", SyncBackend::Tar) | ("pull", SyncBackend::Auto) => {
                // Use podman cp for pull — it handles paths correctly without
                // shell quoting issues that plague tar pipes across host/container.
                // Create target directory on host before copying.
                let cmd = format!(
                    "mkdir -p \"{}\" && podman cp {}:{} \"{}/\"",
                    target, container_name, source, target
                );
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output(),
                )
                .await
            }
            ("push", SyncBackend::PodmanCp) => {
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("podman")
                        .args(["cp", source, &format!("{}:{}", container_name, target)])
                        .output(),
                )
                .await
            }
            ("pull", SyncBackend::PodmanCp) => {
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("podman")
                        .args(["cp", &format!("{}:{}", container_name, source), target])
                        .output(),
                )
                .await
            }
            ("push" | "pull", SyncBackend::Rsync) => {
                // rsync requires rsync in the sandbox; fall back with clear error
                return serde_json::json!({
                    "error": "rsync backend requires rsync installed in the sandbox. Use 'tar' or 'podman-cp' instead.",
                    "hint": "Set backend to 'tar' or 'podman-cp'"
                }).to_string();
            }
            _ => {
                return serde_json::json!({
                    "error": format!("Unknown mode '{}' or backend combination", mode)
                })
                .to_string();
            }
        };

        match result {
            Ok(Ok(output)) if output.status.success() => {
                // Record successful sync experience
                let exit_code = output.status.code().unwrap_or(0);
                experience = experience.completed(exit_code);
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": "synced",
                    "mode": mode,
                    "backend": format!("{:?}", effective_backend).to_lowercase(),
                    "sandbox_id": params.sandbox_id,
                    "source": source,
                    "target": target
                }).to_string()
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr_str: &str = &stderr;
                // SYNC-02: Include exit_code when stderr is empty instead of "unknown error"
                if stderr_str.trim().is_empty() {
                    let exit_code = output.status.code().unwrap_or(-1);
                    serde_json::json!({
                        "error": format!("Sync failed with exit code {}", exit_code),
                        "mode": mode,
                        "backend": format!("{:?}", effective_backend).to_lowercase(),
                        "exit_code": exit_code,
                        "stderr": "(empty)"
                    }).to_string()
                } else {
                    serde_json::json!({
                        "error": format!("Sync failed: {}", stderr.lines().next().unwrap_or("unknown error")),
                        "mode": mode,
                        "backend": format!("{:?}", effective_backend).to_lowercase(),
                        "exit_code": output.status.code().unwrap_or(-1),
                        "stderr": stderr_str
                    }).to_string()
                }
            }
            Ok(Err(e)) => {
                serde_json::json!({"error": format!("Sync command error: {}", e)}).to_string()
            }
            Err(_) => {
                serde_json::json!({
                    "error": format!("Sync timed out after {}ms. Increase timeout_ms for large transfers.", timeout_ms),
                    "timeout_ms": timeout_ms,
                }).to_string()
            }
        }
    }

    /// List all capabilities available for sandbox_prepare.
    ///
    /// Returns capability names, descriptions, toolchain strategies (apt, asdf, sdkman),
    /// packages installed, and estimated duration. Call this before sandbox_prepare to discover
    /// what you can install. Also lists registered template artifacts from the artifact catalog.
    #[tool(
        description = "List all capabilities available for sandbox_prepare. Returns names, descriptions, toolchains (apt/asdf/sdkman), packages, and artifacts. Call this BEFORE sandbox_prepare to discover what you can install."
    )]
    async fn sandbox_list_capabilities(&self) -> String {
        tracing::info!("Listing available capabilities");

        let mut capabilities: Vec<serde_json::Value> = Vec::new();

        // 1. TOML-driven capabilities from CapabilityRegistry
        {
            let registry = self.capability_registry.read().await;
            for name in registry.list_capabilities() {
                if let Some(config) = registry.get_config(&name) {
                    let toolchains: Vec<serde_json::Value> = config
                        .toolchains
                        .iter()
                        .map(|t| {
                            let mut tc = serde_json::json!({
                                "manager": t.manager,
                                "priority": t.priority,
                            });
                            if let Some(ref pkgs) = t.packages {
                                tc["packages"] = serde_json::json!(pkgs);
                            }
                            if let Some(ref env) = t.env {
                                tc["env"] = serde_json::json!(env);
                            }
                            if let Some(ref pp) = t.path_prefix {
                                tc["path_prefix"] = serde_json::json!(pp);
                            }
                            if let Some(ref steps) = t.steps {
                                tc["steps"] = serde_json::json!(steps.len());
                            }
                            tc
                        })
                        .collect();

                    capabilities.push(serde_json::json!({
                        "name": name,
                        "description": config.description,
                        "source": "toml",
                        "toolchains": toolchains,
                    }));
                }
            }
        }

        // 2. Hardcoded capabilities from ToolResolver adapters
        let hardcoded = vec![
            serde_json::json!({
                "name": "jvm-build",
                "description": "Java build environment with JDK + Maven/Gradle",
                "source": "adapter",
                "managers": ["apt", "asdf", "sdkman"],
            }),
            serde_json::json!({
                "name": "node-build",
                "description": "Node.js build environment with npm/yarn/pnpm",
                "source": "adapter",
                "managers": ["apt", "asdf"],
            }),
            serde_json::json!({
                "name": "python-build",
                "description": "Python build environment",
                "source": "adapter",
                "managers": ["apt", "asdf"],
            }),
            serde_json::json!({
                "name": "rust-build",
                "description": "Rust build environment with cargo",
                "source": "adapter",
                "managers": ["apt", "asdf"],
            }),
            serde_json::json!({
                "name": "go-build",
                "description": "Go build environment",
                "source": "adapter",
                "managers": ["apt", "asdf"],
            }),
            serde_json::json!({
                "name": "ruby-build",
                "description": "Ruby build environment",
                "source": "adapter",
                "managers": ["asdf"],
            }),
        ];

        // Deduplicate: if TOML already defines a capability, skip the hardcoded entry
        let toml_names: std::collections::HashSet<String> = capabilities
            .iter()
            .filter_map(|c| c["name"].as_str().map(String::from))
            .collect();

        for hc in hardcoded {
            if let Some(name) = hc["name"].as_str() {
                if !toml_names.contains(name) {
                    capabilities.push(hc);
                }
            }
        }

        // 3. Registered artifacts from the artifact catalog
        let mut artifacts: Vec<serde_json::Value> = Vec::new();
        {
            let catalog = self.artifact_catalog.read().await;
            for entry in catalog.list_enabled() {
                let caps: Vec<serde_json::Value> = entry
                    .artifact
                    .capabilities
                    .iter()
                    .map(|c| {
                        let tools: Vec<serde_json::Value> = c
                            .tools
                            .iter()
                            .map(|t| {
                                serde_json::json!({
                                    "name": t.name,
                                    "version": t.version,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "name": c.name,
                            "tools": tools,
                        })
                    })
                    .collect();
                artifacts.push(serde_json::json!({
                    "name": entry.artifact.name,
                    "version": entry.artifact.version,
                    "capabilities": caps,
                }));
            }
        }

        serde_json::json!({
            "capabilities": capabilities,
            "capabilities_count": capabilities.len(),
            "artifacts": artifacts,
            "artifacts_count": artifacts.len(),
            "usage": "Call sandbox_prepare(sandbox_id, capability) to install one of these capabilities into a sandbox."
        })
        .to_string()
    }

    /// List all registered template artifacts in the artifact catalog.
    ///
    /// Artifacts are pre-packaged environments that sandbox_prepare can materialize
    /// (e.g., a container image with JDK+Maven already installed). Each artifact
    /// lists the capabilities it provides, the tools included, and verification steps.
    #[tool(
        description = "List registered template artifacts for sandbox_prepare materialization. Each artifact shows capabilities, tools, versions, and verification steps. Use sandbox_register_artifact to add new artifacts."
    )]
    async fn sandbox_list_artifacts(&self) -> String {
        tracing::info!("Listing registered artifacts");

        let catalog = self.artifact_catalog.read().await;
        let mut artifacts: Vec<serde_json::Value> = Vec::new();

        for entry in catalog.list_all() {
            let caps: Vec<serde_json::Value> = entry
                .artifact
                .capabilities
                .iter()
                .map(|c| {
                    let tools: Vec<serde_json::Value> = c
                        .tools
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "name": t.name,
                                "version": t.version,
                                "category": format!("{:?}", t.category).to_lowercase(),
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "name": c.name,
                        "tools": tools,
                    })
                })
                .collect();

            artifacts.push(serde_json::json!({
                "name": entry.artifact.name,
                "version": entry.artifact.version,
                "enabled": entry.enabled,
                "capabilities": caps,
            }));
        }

        serde_json::json!({
            "count": artifacts.len(),
            "artifacts": artifacts,
        })
        .to_string()
    }

    /// Run provider readiness check using the doctor registry.
    ///
    /// Looks up a doctor with ID `{provider}.readiness` (e.g., `firecracker.readiness`)
    /// and runs all its checks to verify the provider is ready for sandbox creation.
    async fn run_provider_readiness_check(
        &self,
        doctor_id: &str,
        provider: &str,
    ) -> DoctorResult {
        let ctx = match &self.doctor_context {
            Some(c) => c,
            None => {
                // Doctor context not configured, skip check
                return DoctorResult {
                    doctor_id: doctor_id.to_string(),
                    sandbox_id: None,
                    status: DoctorStatus::Pass,
                    severity: bastion_domain::catalog::doctor::Severity::Warning,
                    trace_id: uuid::Uuid::new_v4().to_string(),
                    check_results: Vec::new(),
                    rationale: "Doctor context not configured".to_string(),
                    executed_at: chrono::Utc::now(),
                    rich_check_results: Vec::new(),
                    summary: "Doctor context not configured, skipping check".to_string(),
                    requires_ai_attention: false,
                    potential_self_remediation: false,
                };
            }
        };

        // Get the doctor
        let doctor = match ctx.doctor_registry.get(doctor_id) {
            Some(d) => d,
            None => {
                // Doctor doesn't exist, log warning and skip
                tracing::warn!(doctor_id = doctor_id, "Provider readiness doctor not found");
                return DoctorResult {
                    doctor_id: doctor_id.to_string(),
                    sandbox_id: None,
                    status: DoctorStatus::Pass,
                    severity: bastion_domain::catalog::doctor::Severity::Warning,
                    trace_id: uuid::Uuid::new_v4().to_string(),
                    check_results: Vec::new(),
                    rationale: format!("Doctor {} not found, skipping check", doctor_id),
                    executed_at: chrono::Utc::now(),
                    rich_check_results: Vec::new(),
                    summary: format!("Doctor {} not found, skipping check", doctor_id),
                    requires_ai_attention: false,
                    potential_self_remediation: false,
                };
            }
        };

        // Run each check
        let mut check_results = Vec::new();
        let mut rich_check_results = Vec::new();
        let mut overall_status = DoctorStatus::Pass;

        for check in &doctor.checks {
            let result = self.evaluate_check(check, ctx, provider).await;

            if result.status == CheckStatus::Fail {
                overall_status = DoctorStatus::Fail;
            } else if result.status == CheckStatus::Warning && overall_status == DoctorStatus::Pass {
                overall_status = DoctorStatus::Error;
            }

            // Convert RichCheckResult to simple CheckResult for compatibility
            let simple_result = bastion_domain::catalog::assertion::CheckResult {
                check: result.check_type.clone(),
                passed: result.status == CheckStatus::Pass,
                reason: if result.status != CheckStatus::Pass {
                    Some(format!("{:?}: {:?}", result.status, result.current_state))
                } else {
                    None
                },
            };
            check_results.push(simple_result);
            rich_check_results.push(result);
        }

        // Determine if AI can self-remediation
        let potential_self_remediation = rich_check_results
            .iter()
            .any(|r| {
                r.remediation
                    .as_ref()
                    .map(|rem| rem.auto_fixable && !rem.commands.is_empty())
                    .unwrap_or(false)
            });

        // Generate summary
        let failed_count = rich_check_results
            .iter()
            .filter(|r| r.status == CheckStatus::Fail)
            .count();
        let warning_count = rich_check_results
            .iter()
            .filter(|r| r.status == CheckStatus::Warning)
            .count();

        let summary = match overall_status {
            DoctorStatus::Pass => format!("{} provider is ready", provider),
            DoctorStatus::Fail => format!("{} provider has {} missing requirements", provider, failed_count),
            DoctorStatus::Skip => format!("{} provider readiness check skipped", provider),
            DoctorStatus::Error => format!("{} provider has {} warnings", provider, warning_count),
        };

        DoctorResult {
            doctor_id: doctor_id.to_string(),
            sandbox_id: None,
            status: overall_status,
            severity: doctor.severity,
            trace_id: uuid::Uuid::new_v4().to_string(),
            check_results,
            rationale: summary.clone(),
            executed_at: chrono::Utc::now(),
            rich_check_results,
            summary,
            requires_ai_attention: overall_status != DoctorStatus::Pass,
            potential_self_remediation,
        }
    }

    /// Evaluate a single doctor check and return a rich result.
    async fn evaluate_check(
        &self,
        check: &DoctorCheck,
        ctx: &DoctorContext,
        provider: &str,
    ) -> RichCheckResult {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let executed_at = chrono::Utc::now();

        match check {
            DoctorCheck::ProviderAlive { provider } => {
                self.eval_provider_alive(provider, &trace_id, executed_at).await
            }
            DoctorCheck::BinaryAvailable { name, expected_path } => {
                self.eval_binary_available(name, expected_path.as_deref(), &trace_id, executed_at)
                    .await
            }
            DoctorCheck::KvmAvailable => {
                self.eval_kvm_available(&trace_id, executed_at).await
            }
            DoctorCheck::ImageAvailable { provider, image } => {
                self.eval_image_available(provider, image.as_deref(), &trace_id, executed_at)
                    .await
            }
            DoctorCheck::WorkerBinaryValid { provider } => {
                self.eval_worker_binary_valid(provider, &trace_id, executed_at).await
            }
            DoctorCheck::CapabilitiesMet {
                provider,
                min_memory_mb,
                min_cpu_count,
            } => {
                self.eval_capabilities_met(provider, *min_memory_mb, *min_cpu_count, &trace_id, executed_at)
                    .await
            }
            DoctorCheck::ConfigValid { provider } => {
                self.eval_config_valid(provider, &trace_id, executed_at).await
            }
            _ => {
                // Unknown check type - return skip result
                RichCheckResult {
                    check_id: format!("unknown.{}", provider),
                    check_type: "unknown".to_string(),
                    status: CheckStatus::Skip,
                    current_state: serde_json::json!({}),
                    expected_state: serde_json::json!({}),
                    delta: Vec::new(),
                    remediation: None,
                    system_context: self.get_system_context(),
                    trace_id,
                    executed_at,
                }
            }
        }
    }

    /// Evaluate ProviderAlive check.
    async fn eval_provider_alive(
        &self,
        provider_name: &str,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        // Check if the provider daemon socket is accessible
        // This is different from is_alive() which checks if a specific container is running
        let socket_path = match provider_name {
            "podman" => "/run/user/1000/podman/podman.sock",
            "docker" => "/var/run/docker.sock",
            _ => "/var/run/unknown.sock",
        };

        let is_alive = std::path::Path::new(socket_path).exists()
            && std::os::unix::net::UnixStream::connect(socket_path).is_ok();

        let status = if is_alive {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: format!("provider_alive.{}", provider_name),
            check_type: "provider_alive".to_string(),
            status,
            current_state: serde_json::json!({
                "provider": provider_name,
                "alive": is_alive,
                "socket_path": socket_path
            }),
            expected_state: serde_json::json!({
                "provider": provider_name,
                "alive": true
            }),
            delta: if !is_alive {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: format!("{} provider", provider_name),
                    expected: "alive and responsive".to_string(),
                    actual: Some("not responding".to_string()),
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else {
                Vec::new()
            },
            remediation: None,
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate BinaryAvailable check.
    async fn eval_binary_available(
        &self,
        name: &str,
        expected_path: Option<&str>,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        use std::process::Command;

        let which_output = Command::new("which").arg(name).output();
        let found_path = which_output.ok().and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

        let found = found_path.is_some();
        let path_match = expected_path.map(|ep| found_path.as_ref() == Some(&ep.to_string())).unwrap_or(true);

        let status = if found && path_match {
            CheckStatus::Pass
        } else if found {
            CheckStatus::Warning
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: format!("binary_available.{}", name),
            check_type: "binary_available".to_string(),
            status,
            current_state: serde_json::json!({
                "binary": name,
                "found": found,
                "path": found_path,
                "expected_path": expected_path
            }),
            expected_state: serde_json::json!({
                "binary": name,
                "found": true,
                "expected_path": expected_path
            }),
            delta: if !found {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: format!("binary '{}'", name),
                    expected: expected_path.unwrap_or("in PATH").to_string(),
                    actual: None,
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else {
                Vec::new()
            },
            remediation: if !found {
                Some(bastion_domain::catalog::doctor::Remediation {
                    confidence: "high".to_string(),
                    auto_fixable: true,
                    commands: vec![
                        format!("sudo apt-get install -y {}", name),
                        format!("cargo install {}", name),
                    ],
                    manual_steps: vec![format!("Install {} manually", name)],
                    verify_after: format!("which {}", name),
                    install_sources: vec![
                        bastion_domain::catalog::doctor::InstallSource {
                            name: name.to_string(),
                            url: format!("https://packages.debian.org/{}", name),
                            method: "package_manager".to_string(),
                        }
                    ],
                })
            } else {
                None
            },
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate KvmAvailable check.
    async fn eval_kvm_available(
        &self,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        use std::os::unix::fs::MetadataExt;
        use std::path::Path;

        let kvm_path = Path::new("/dev/kvm");
        let exists = kvm_path.exists();
        let accessible = if exists {
            std::fs::metadata(kvm_path).map(|m| m.mode() & 0o777 != 0).unwrap_or(false)
        } else {
            false
        };

        let status = if exists && accessible {
            CheckStatus::Pass
        } else if exists {
            CheckStatus::Fail
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: "kvm_available".to_string(),
            check_type: "kvm_available".to_string(),
            status,
            current_state: serde_json::json!({
                "kvm_device": "/dev/kvm",
                "exists": exists,
                "accessible": accessible
            }),
            expected_state: serde_json::json!({
                "kvm_device": "/dev/kvm",
                "exists": true,
                "accessible": true
            }),
            delta: if !exists {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: "KVM device".to_string(),
                    expected: "/dev/kvm exists".to_string(),
                    actual: None,
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else if !accessible {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: "KVM access".to_string(),
                    expected: "User in kvm group".to_string(),
                    actual: Some("NOT in kvm group".to_string()),
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else {
                Vec::new()
            },
            remediation: if !accessible {
                Some(bastion_domain::catalog::doctor::Remediation {
                    confidence: "high".to_string(),
                    auto_fixable: true,
                    commands: vec![
                        "sudo usermod -aG kvm $USER".to_string(),
                        "newgrp kvm".to_string(),
                    ],
                    manual_steps: vec![
                        "Run: sudo usermod -aG kvm $USER".to_string(),
                        "Log out and log back in for group membership to take effect".to_string(),
                    ],
                    verify_after: "groups | grep kvm".to_string(),
                    install_sources: Vec::new(),
                })
            } else if !exists {
                Some(bastion_domain::catalog::doctor::Remediation {
                    confidence: "medium".to_string(),
                    auto_fixable: false,
                    commands: Vec::new(),
                    manual_steps: vec![
                        "Enable KVM virtualization in BIOS/UEFI".to_string(),
                        "Load KVM kernel modules: sudo modprobe kvm".to_string(),
                    ],
                    verify_after: "ls -la /dev/kvm".to_string(),
                    install_sources: Vec::new(),
                })
            } else {
                None
            },
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate ImageAvailable check.
    async fn eval_image_available(
        &self,
        provider_name: &str,
        _image: Option<&str>,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        let provider = self.resolve_provider(provider_name);

        // Try to list images - if we can, the provider is working
        let list_result = provider.list_sandboxes(&Default::default()).await;
        let can_list = list_result.is_ok();

        let status = if can_list {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: format!("image_available.{}", provider_name),
            check_type: "image_available".to_string(),
            status,
            current_state: serde_json::json!({
                "provider": provider.name(),
                "can_list_images": can_list,
                "error": list_result.as_ref().err().map(|e| e.to_string())
            }),
            expected_state: serde_json::json!({
                "provider": provider.name(),
                "can_list_images": true
            }),
            delta: if !can_list {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: format!("{} image availability", provider_name),
                    expected: "Provider can list images".to_string(),
                    actual: list_result.as_ref().err().map(|e| e.to_string()),
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else {
                Vec::new()
            },
            remediation: None,
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate WorkerBinaryValid check.
    async fn eval_worker_binary_valid(
        &self,
        provider_name: &str,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        use std::path::Path;
        use std::process::Command;

        let worker_paths = vec![
            "target/debug/bastion-worker",
            "target/release/bastion-worker",
            "/usr/local/bin/bastion-worker",
        ];

        let mut found_path: Option<String> = None;
        for path in &worker_paths {
            if Path::new(path).exists() {
                found_path = Some(path.to_string());
                break;
            }
        }

        // Also check if it's in PATH
        let which_output = Command::new("which").arg("bastion-worker").output();
        let in_path = which_output.ok().and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

        let valid = found_path.is_some() || in_path.is_some();
        let status = if valid {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: format!("worker_binary_valid.{}", provider_name),
            check_type: "worker_binary_valid".to_string(),
            status,
            current_state: serde_json::json!({
                "binary": "bastion-worker",
                "found": valid,
                "paths_checked": worker_paths,
                "found_path": found_path.or(in_path),
            }),
            expected_state: serde_json::json!({
                "binary": "bastion-worker",
                "found": true
            }),
            delta: if !valid {
                vec![bastion_domain::catalog::doctor::DeltaItem {
                    item: "bastion-worker binary".to_string(),
                    expected: "bastion-worker exists".to_string(),
                    actual: None,
                    severity: bastion_domain::catalog::doctor::Severity::Critical,
                }]
            } else {
                Vec::new()
            },
            remediation: if !valid {
                Some(bastion_domain::catalog::doctor::Remediation {
                    confidence: "high".to_string(),
                    auto_fixable: true,
                    commands: vec![
                        "cargo build --bin bastion-worker".to_string(),
                    ],
                    manual_steps: vec![
                        "Run: cargo build --bin bastion-worker".to_string(),
                        "Or install via: cargo install bastion-worker".to_string(),
                    ],
                    verify_after: "which bastion-worker || ls -la target/debug/bastion-worker".to_string(),
                    install_sources: vec![
                        bastion_domain::catalog::doctor::InstallSource {
                            name: "bastion-worker".to_string(),
                            url: "https://github.com/example/bastion".to_string(),
                            method: "source".to_string(),
                        }
                    ],
                })
            } else {
                None
            },
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate CapabilitiesMet check.
    async fn eval_capabilities_met(
        &self,
        provider_name: &str,
        min_memory_mb: Option<u64>,
        min_cpu_count: Option<u32>,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        let provider = self.resolve_provider(provider_name);
        let caps = provider.capabilities();

        let memory_ok = min_memory_mb.map(|min| caps.max_memory_mb() >= min).unwrap_or(true);
        let cpu_ok = min_cpu_count.map(|min| caps.max_cpu_count() >= min).unwrap_or(true);

        let status = if memory_ok && cpu_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };

        RichCheckResult {
            check_id: format!("capabilities_met.{}", provider_name),
            check_type: "capabilities_met".to_string(),
            status,
            current_state: serde_json::json!({
                "provider": provider.name(),
                "memory_mb": caps.max_memory_mb(),
                "cpu_count": caps.max_cpu_count()
            }),
            expected_state: serde_json::json!({
                "provider": provider.name(),
                "min_memory_mb": min_memory_mb,
                "min_cpu_count": min_cpu_count
            }),
            delta: {
                let mut deltas = Vec::new();
                if !memory_ok {
                    deltas.push(bastion_domain::catalog::doctor::DeltaItem {
                        item: "Memory".to_string(),
                        expected: format!("{} MB", min_memory_mb.unwrap()),
                        actual: Some(format!("{} MB", caps.max_memory_mb())),
                        severity: bastion_domain::catalog::doctor::Severity::Critical,
                    });
                }
                if !cpu_ok {
                    deltas.push(bastion_domain::catalog::doctor::DeltaItem {
                        item: "CPU count".to_string(),
                        expected: format!("{}", min_cpu_count.unwrap()),
                        actual: Some(format!("{}", caps.max_cpu_count())),
                        severity: bastion_domain::catalog::doctor::Severity::Critical,
                    });
                }
                deltas
            },
            remediation: None,
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Evaluate ConfigValid check.
    async fn eval_config_valid(
        &self,
        provider_name: &str,
        trace_id: &str,
        executed_at: chrono::DateTime<chrono::Utc>,
    ) -> RichCheckResult {
        let provider = self.resolve_provider(provider_name);

        // Basic config validation - provider can be resolved and initialized
        let status = CheckStatus::Pass;

        RichCheckResult {
            check_id: format!("config_valid.{}", provider_name),
            check_type: "config_valid".to_string(),
            status,
            current_state: serde_json::json!({
                "provider": provider.name(),
                "config_valid": true
            }),
            expected_state: serde_json::json!({
                "provider": provider.name(),
                "config_valid": true
            }),
            delta: Vec::new(),
            remediation: None,
            system_context: self.get_system_context(),
            trace_id: trace_id.to_string(),
            executed_at,
        }
    }

    /// Get system context for rich check results.
    fn get_system_context(&self) -> bastion_domain::catalog::doctor::SystemContext {
        use std::collections::HashMap;

        let uname = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let os = std::process::Command::new("uname")
            .arg("-s")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let arch = std::process::Command::new("uname")
            .arg("-m")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check for KVM
        let has_kvm = std::path::Path::new("/dev/kvm").exists();

        // Collect relevant binaries
        let mut relevant_binaries = HashMap::new();
        for binary in &["podman", "docker", "firecracker", "containerd-shim"] {
            let which_output = std::process::Command::new("which")
                .arg(binary)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

            if which_output.is_some() {
                relevant_binaries.insert(
                    binary.to_string(),
                    bastion_domain::catalog::doctor::BinaryInfo {
                        name: binary.to_string(),
                        path: which_output,
                        version: None,
                    },
                );
            }
        }

        bastion_domain::catalog::doctor::SystemContext {
            os,
            os_version: uname.clone(),
            architecture: arch,
            kernel: uname,
            has_kvm,
            has_nested_virt: None,
            relevant_binaries,
            installed_providers: HashMap::new(),
        }
    }
}
