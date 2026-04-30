//! MCP Server handler for Bastion Gateway.
//!
//! Implements the rmcp ServerHandler with sandbox tools.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool, tool_router};
use serde::Deserialize;
use schemars::JsonSchema;

use bastion_application::execution::RunCommandUseCase;
use bastion_application::file_ops::{ListFilesUseCase, ReadFileUseCase, WriteFileUseCase};
use bastion_application::sandbox::{CreateSandboxUseCase, GetSandboxInfoUseCase, ListSandboxesUseCase, TerminateSandboxUseCase};
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::shared::id::SandboxId;

/// Bastion MCP Gateway server.
///
/// Exposes sandbox management tools to AI agents via MCP protocol.
#[derive(Debug, Clone)]
pub struct BastionGateway {
    provider: Arc<dyn SandboxProvider>,
    repository: Arc<dyn SandboxRepository>,
}

impl BastionGateway {
    pub fn new(provider: Arc<dyn SandboxProvider>, repository: Arc<dyn SandboxRepository>) -> Self {
        Self { provider, repository }
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

fn default_timeout() -> u64 { 3_600_000 }

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

#[tool_router(server_handler)]
impl BastionGateway {
    #[tool(description = "Create a new isolated sandbox environment")]
    async fn sandbox_create(
        &self,
        Parameters(params): Parameters<SandboxCreateParams>,
    ) -> String {
        tracing::info!(template = %params.template, "Creating sandbox");

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
            Ok(sandbox) => serde_json::json!({
                "sandbox_id": sandbox.id.to_string(),
                "status": sandbox.status.to_string(),
                "template": sandbox.template_id.to_string()
            }).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Execute a command in a sandbox")]
    async fn sandbox_run(
        &self,
        Parameters(params): Parameters<SandboxRunParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = RunCommandUseCase::new(self.repository.clone());

        let command_spec = CommandSpec::new(&params.command);

        match use_case.execute(&sandbox_id, &command_spec, self.provider.as_ref()).await {
            Ok(result) => serde_json::json!({
                "exit_code": result.exit_code,
                "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
                "duration_ms": result.duration_ms,
                "timed_out": result.timed_out
            }).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Write a file to a sandbox")]
    async fn sandbox_write(
        &self,
        Parameters(params): Parameters<SandboxWriteParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Writing file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = WriteFileUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id, &params.path, params.content.as_bytes(), self.provider.as_ref()).await {
            Ok(()) => serde_json::json!({"status": "ok"}).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Read a file from a sandbox")]
    async fn sandbox_read(
        &self,
        Parameters(params): Parameters<SandboxReadParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Reading file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = ReadFileUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id, &params.path, self.provider.as_ref()).await {
            Ok(content) => serde_json::json!({
                "content": String::from_utf8_lossy(&content).to_string(),
                "encoding": "utf-8"
            }).to_string(),
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

        match use_case.execute(&sandbox_id, &params.path, self.provider.as_ref()).await {
            Ok(entries) => {
                let list: Vec<serde_json::Value> = entries.iter().map(|e| {
                    serde_json::json!({
                        "path": e.path,
                        "is_directory": e.is_directory,
                        "size_bytes": e.size_bytes,
                        "permissions": e.permissions,
                    })
                }).collect();
                serde_json::json!({
                    "count": list.len(),
                    "entries": list
                }).to_string()
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

        let use_case = TerminateSandboxUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id, self.provider.as_ref()).await {
            Ok(()) => serde_json::json!({"status": "terminated"}).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Get information about a sandbox")]
    async fn sandbox_info(
        &self,
        Parameters(params): Parameters<SandboxInfoParams>,
    ) -> String {
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
            }).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "List all active sandboxes")]
    async fn sandbox_list(
        &self,
    ) -> String {
        tracing::info!("Listing active sandboxes");

        let use_case = ListSandboxesUseCase::new(self.repository.clone());

        match use_case.execute().await {
            Ok(sandboxes) => {
                let list: Vec<serde_json::Value> = sandboxes.iter().map(|s| {
                    serde_json::json!({
                        "sandbox_id": s.id.to_string(),
                        "status": s.status.to_string(),
                        "template": s.template_id.to_string(),
                        "created_at": s.created_at.to_rfc3339(),
                    })
                }).collect();
                serde_json::json!({
                    "count": list.len(),
                    "sandboxes": list
                }).to_string()
            }
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }
}
