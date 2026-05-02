//! MCP Server handler for Bastion Gateway.
//!
//! Implements the rmcp ServerHandler with sandbox tools.

use std::sync::Arc;

use futures::StreamExt;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::RequestContext;
use rmcp::{schemars, tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use bastion_application::execution::{RunCommandStreamUseCase, RunCommandUseCase};
use bastion_application::file_ops::{ListFilesUseCase, ReadFileUseCase, WriteFileUseCase};
use bastion_application::sandbox::{
    CreateSandboxUseCase, GetSandboxInfoUseCase, ListSandboxesUseCase, TerminateSandboxUseCase,
};
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::execution::stream::ChunkType;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::id::SandboxId;
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::pool::SandboxPoolManager;

/// Bastion MCP Gateway server.
///
/// Exposes sandbox management tools to AI agents via MCP protocol.
#[derive(Debug, Clone)]
pub struct BastionGateway {
    provider: Arc<dyn SandboxProvider>,
    repository: Arc<dyn SandboxRepository>,
    pool_manager: Option<Arc<SandboxPoolManager>>,
    metrics: GatewayMetrics,
}

impl BastionGateway {
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        repository: Arc<dyn SandboxRepository>,
        pool_manager: Option<Arc<SandboxPoolManager>>,
        metrics: GatewayMetrics,
    ) -> Self {
        Self {
            provider,
            repository,
            pool_manager,
            metrics,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxCreateParams {
    /// Template (base image) for the sandbox
    pub template: String,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    3_600_000
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunParams {
    /// ID of the sandbox
    pub sandbox_id: String,
    /// Command to execute
    pub command: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxWriteParams {
    pub sandbox_id: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxReadParams {
    pub sandbox_id: String,
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxTerminateParams {
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxInfoParams {
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxListFilesParams {
    pub sandbox_id: String,
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunStreamParams {
    /// ID of the sandbox
    pub sandbox_id: String,
    /// Command to execute
    pub command: String,
}

#[tool_router(server_handler)]
impl BastionGateway {
    #[tool(description = "Create a new isolated sandbox environment")]
    async fn sandbox_create(&self, Parameters(params): Parameters<SandboxCreateParams>) -> String {
        tracing::info!(template = %params.template, "Creating sandbox");

        // Try pool checkout first if pool is available
        if let Some(ref pool) = self.pool_manager {
            match pool.checkout(&params.template, params.timeout_ms).await {
                Ok(sandbox) => {
                    tracing::debug!(
                        sandbox_id = %sandbox.id,
                        template = %params.template,
                        "Sandbox created via pool checkout"
                    );
                    self.metrics.record_sandbox_created();
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
            bastion_domain::shared::id::ProviderId::new("podman"),
        );

        let input = bastion_application::sandbox::create::CreateSandboxInput {
            template_id: params.template.clone(),
            provider_id: None,
            resources: bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            network: bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            env_vars: std::collections::HashMap::new(),
            timeout_ms: params.timeout_ms,
        };

        match use_case.execute(input, self.provider.as_ref()).await {
            Ok(sandbox) => {
                self.metrics.record_sandbox_created();
                serde_json::json!({
                    "sandbox_id": sandbox.id.to_string(),
                    "status": sandbox.status.to_string(),
                    "template": sandbox.template_id.to_string(),
                    "from_pool": false
                })
                .to_string()
            }
            Err(e) => {
                self.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(description = "Execute a command in a sandbox")]
    async fn sandbox_run(&self, Parameters(params): Parameters<SandboxRunParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = RunCommandUseCase::new(self.repository.clone());

        let command_spec = CommandSpec::new(&params.command);

        match use_case
            .execute(&sandbox_id, &command_spec, self.provider.as_ref())
            .await
        {
            Ok(result) => {
                self.metrics.record_command(result.duration_ms * 1000);
                serde_json::json!({
                    "exit_code": result.exit_code,
                    "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
                    "duration_ms": result.duration_ms,
                    "timed_out": result.timed_out
                })
                .to_string()
            }
            Err(e) => {
                self.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(
        description = "Execute a command with streaming output (returns stdout/stderr separately with exit code)"
    )]
    async fn sandbox_run_stream(
        &self,
        Parameters(params): Parameters<SandboxRunStreamParams>,
        request_ctx: RequestContext<RoleServer>,
    ) -> String {
        // Extract progress token from meta if present
        let progress_token = request_ctx.meta.get_progress_token();
        let peer = request_ctx.peer.clone();

        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running streaming command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let command_spec = CommandSpec::new(&params.command);

        let use_case = RunCommandStreamUseCase::new(self.repository.clone());
        let start_time = std::time::Instant::now();

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
                    // Send progress notification if token is present
                    if let Some(ref token) = progress_token {
                        chunk_count += 1;
                        // Estimate progress based on chunk count (0.0 to 0.9 until complete)
                        let progress = (chunk_count as f64 / 100.0).min(0.9);
                        let message = Self::build_progress_message(&stdout_parts, &stderr_parts, chunk_count);
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

                // Send final progress notification
                if let Some(ref token) = progress_token {
                    Self::send_progress(&peer, token, 1.0, Some("Complete")).await;
                }

                let duration_us = start_time.elapsed().as_micros() as u64;
                self.metrics.record_command(duration_us);

                serde_json::json!({
                    "exit_code": exit_code,
                    "stdout": stdout_parts.join(""),
                    "stderr": stderr_parts.join(""),
                    "chunks_received": stdout_parts.len() + stderr_parts.len(),
                })
                .to_string()
            }
            Err(e) => {
                self.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

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
                "content": String::from_utf8_lossy(&content).to_string(),
                "encoding": "utf-8"
            })
            .to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

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

    #[tool(description = "Terminate and destroy a sandbox")]
    async fn sandbox_terminate(
        &self,
        Parameters(params): Parameters<SandboxTerminateParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Terminating sandbox");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Try to return to pool first if pool is available
        if let Some(ref pool) = self.pool_manager {
            match pool.checkin(&sandbox_id).await {
                Ok(()) => {
                    tracing::debug!(sandbox_id = %params.sandbox_id, "Sandbox returned to pool");
                    self.metrics.record_sandbox_terminated();
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
                self.metrics.record_sandbox_terminated();
                serde_json::json!({"status": "terminated"}).to_string()
            }
            Err(e) => {
                self.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

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

    #[tool(description = "Get sandbox pool statistics")]
    async fn sandbox_pool_stats(&self) -> String {
        tracing::debug!("Getting pool statistics");

        if let Some(ref pool) = self.pool_manager {
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
        if let Some(ref pool) = self.pool_manager {
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

    #[tool(description = "Get gateway metrics in Prometheus format")]
    async fn sandbox_metrics(&self) -> String {
        tracing::debug!("Getting metrics");
        self.metrics.prometheus_export()
    }

    /// Send a progress notification to the MCP client.
    /// If sending fails, logs a warning but continues execution.
    async fn send_progress(
        peer: &rmcp::Peer<rmcp::RoleServer>,
        token: &ProgressToken,
        progress: f64,
        message: Option<&str>,
    ) {
        let params = match message {
            Some(msg) => ProgressNotificationParam::new(token.clone(), progress).with_message(msg),
            None => ProgressNotificationParam::new(token.clone(), progress),
        };
        if let Err(e) = peer.notify_progress(params).await {
            tracing::warn!(error = %e, "Failed to send progress notification");
        }
    }

    /// Build a progress message from current stdout/stderr accumulated output.
    fn build_progress_message(
        stdout_parts: &[String],
        stderr_parts: &[String],
        chunk_count: u32,
    ) -> Option<String> {
        // Show last 200 chars of stdout as preview, truncated for notification size
        let stdout_preview = stdout_parts
            .last()
            .map(|s| {
                if s.len() > 200 {
                    format!("{}...", &s[s.len() - 200..])
                } else {
                    s.clone()
                }
            })
            .filter(|s| !s.is_empty());

        let message = match (stdout_preview, stderr_parts.is_empty()) {
            (Some(preview), true) => format!("[{} chunks] {}", chunk_count, preview),
            (Some(preview), false) => format!("[{} chunks] {} (+stderr)", chunk_count, preview),
            (None, false) => format!("[{} chunks] (stderr: {})", chunk_count, stderr_parts.len()),
            (None, true) => format!("[{} chunks] processing...", chunk_count),
        };

        // Truncate message if too long for notification
        if message.len() > 500 {
            Some(format!("{}...", &message[..500]))
        } else {
            Some(message)
        }
    }
}
