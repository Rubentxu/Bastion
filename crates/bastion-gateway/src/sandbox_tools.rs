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
use bastion_domain::catalog::experience::ExperienceRecord;
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::execution::stream::ChunkType;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::secret::{SecretResolver, SecretSource, parse_secret_ref};
use bastion_domain::shared::{DomainError, id::SandboxId};
use bastion_domain::template::{
    ArtifactCatalog, CapabilityDescriptor, MaterializationMode, ProviderMaterializer,
    TemplateArtifact, ToolDescriptor, ToolResolver, ToolchainRequest, ToolchainStrategy,
};
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::pool::SandboxPoolManager;
use bastion_infrastructure::template::{
    AptAdapter, AsdfAdapter, CapabilityRegistry, FsArtifactStore, PodmanOptimizedMaterializer,
    SdkmanAdapter, SnapshotManager,
};

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
}

// ─── Tool implementations ───────────────────────────────────────────────────

impl BastionGateway {
    /// Create a new isolated sandbox environment.
    #[tool(description = "Create a new isolated sandbox environment")]
    async fn sandbox_create(&self, Parameters(params): Parameters<SandboxCreateParams>) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        let selected_provider = self.resolve_provider(&params.provider);
        tracing::info!(template = %params.template, provider = %params.provider, "Creating sandbox");

        // Try pool checkout first if pool is available
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
    #[tool(description = "Execute a command in a sandbox")]
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
        match use_case
            .execute(&sandbox_id, &command_spec, self.provider.as_ref())
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
    #[tool(
        description = "Execute a command with streaming output (returns stdout/stderr separately with exit code)"
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

        // Register cancel token for this streaming command
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_tokens
            .insert(params.sandbox_id.clone(), cancel_flag.clone());

        match use_case
            .execute(&sandbox_id, &command_spec, self.provider.as_ref())
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

    /// Write a file to a sandbox.
    #[tool(description = "Write a file to a sandbox")]
    async fn sandbox_write(&self, Parameters(params): Parameters<SandboxWriteParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Writing file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = WriteFileUseCase::new(self.repository.clone());

        match use_case
            .execute(
                &sandbox_id,
                &params.path,
                params.content.as_bytes(),
                self.provider.as_ref(),
            )
            .await
        {
            Ok(()) => serde_json::json!({"status": "ok"}).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    /// Read a file from a sandbox.
    #[tool(description = "Read a file from a sandbox")]
    async fn sandbox_read(&self, Parameters(params): Parameters<SandboxReadParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Reading file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = ReadFileUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, self.provider.as_ref())
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
    #[tool(description = "List files in a directory inside a sandbox")]
    async fn sandbox_list_files(
        &self,
        Parameters(params): Parameters<SandboxListFilesParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Listing files");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = ListFilesUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, self.provider.as_ref())
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
    #[tool(description = "List available sandbox templates (container images)")]
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
    #[tool(description = "Terminate and destroy a sandbox")]
    async fn sandbox_terminate(
        &self,
        Parameters(params): Parameters<SandboxTerminateParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Terminating sandbox");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Try to return to pool first if pool is available
        if let Some(ref pool) = self.gateway_config.pool_manager {
            match pool.checkin(&sandbox_id).await {
                Ok(()) => {
                    tracing::debug!(sandbox_id = %params.sandbox_id, "Sandbox returned to pool");
                    self.gateway_config.metrics.record_sandbox_terminated();
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

        match use_case.execute(&sandbox_id, self.provider.as_ref()).await {
            Ok(()) => {
                self.gateway_config.metrics.record_sandbox_terminated();
                serde_json::json!({"status": "terminated"}).to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    /// Cancel a running command in a sandbox.
    #[tool(description = "Cancel a running command in a sandbox")]
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

        // Also ask the provider to cancel the command (SIGTERM/SIGKILL)
        match self
            .provider
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
    #[tool(description = "Get information about a sandbox")]
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
    #[tool(description = "List all active sandboxes")]
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
    #[tool(description = "Get sandbox pool statistics")]
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
    #[tool(description = "Check gateway health including provider connectivity and pool status")]
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

    /// Get gateway metrics in Prometheus format.
    #[tool(description = "Get gateway metrics in Prometheus format")]
    async fn sandbox_metrics(&self) -> String {
        tracing::debug!("Getting metrics");
        self.gateway_config.metrics.prometheus_export()
    }

    /// Register a template artifact that provides a capability.
    #[tool(description = "Register a template artifact that provides a capability")]
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

    /// Prepare a sandbox with a specific capability (e.g. jvm-build).
    #[tool(description = "Prepare a sandbox with a specific capability (e.g. jvm-build)")]
    async fn sandbox_prepare(
        &self,
        Parameters(params): Parameters<SandboxPrepareParams>,
    ) -> String {
        let sandbox_id = SandboxId::new(&params.sandbox_id);
        let capability = &params.capability;

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
            let materializer = PodmanOptimizedMaterializer::new(
                self.provider.clone(),
                self.artifact_store.clone(),
                PathBuf::from("/tmp/bastion-cache"),
            );
            match materializer
                .materialize(&sandbox_id, &artifact, MaterializationMode::Auto)
                .await
            {
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

                match self.provider.run_command(&sandbox_id, &cmd).await {
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

                match self.provider.run_command(&sandbox_id, &cmd).await {
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

                    match self.provider.run_command(&sandbox_id, &cmd).await {
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
    #[tool(description = "Manage sandbox snapshots (create, restore, list, delete)")]
    async fn sandbox_snapshot(
        &self,
        Parameters(params): Parameters<SandboxSnapshotParams>,
    ) -> String {
        let snapshot_manager = SnapshotManager::new(bastion_domain::template::ProviderKind::Podman);

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

                match snapshot_manager.create_snapshot(&sandbox_id, name).await {
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

                match snapshot_manager.restore_snapshot(snapshot_id).await {
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
            "list" => match snapshot_manager.list_snapshots().await {
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

                match snapshot_manager.delete_snapshot(snapshot_id).await {
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
    #[tool(description = "Sync files between host and sandbox (push/pull)")]
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
        if !provider.capabilities().supports_streaming {
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
}
