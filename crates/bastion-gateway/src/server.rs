//! MCP Server handler for Bastion Gateway.
//!
//! Implements the rmcp ServerHandler with sandbox tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{schemars, tool, tool_router};
use serde::Deserialize;
use schemars::JsonSchema;

/// Bastion MCP Gateway server.
///
/// Exposes sandbox management tools to AI agents via MCP protocol.
#[derive(Debug, Clone)]
pub struct BastionGateway;

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

#[tool_router(server_handler)]
impl BastionGateway {
    #[tool(description = "Create a new isolated sandbox environment")]
    async fn sandbox_create(
        &self,
        Parameters(params): Parameters<SandboxCreateParams>,
    ) -> String {
        tracing::info!(template = %params.template, "Creating sandbox");
        // TODO: Delegate to CreateSandboxUseCase
        format!(r#"{{"sandbox_id": "{}", "status": "running", "template": "{}"}}"#,
            uuid::Uuid::new_v4(),
            params.template
        )
    }

    #[tool(description = "Execute a command in a sandbox")]
    async fn sandbox_run(
        &self,
        Parameters(params): Parameters<SandboxRunParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running command");
        // TODO: Delegate to RunCommandUseCase
        format!(r#"{{"exit_code": 0, "stdout": "Executed: {}\n", "stderr": ""}}"#,
            params.command
        )
    }

    #[tool(description = "Write a file to a sandbox")]
    async fn sandbox_write(
        &self,
        Parameters(params): Parameters<SandboxWriteParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Writing file");
        r#"{"status": "ok"}"#.to_string()
    }

    #[tool(description = "Read a file from a sandbox")]
    async fn sandbox_read(
        &self,
        Parameters(params): Parameters<SandboxReadParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Reading file");
        r#"{"content": "", "encoding": "utf-8"}"#.to_string()
    }

    #[tool(description = "Terminate and destroy a sandbox")]
    async fn sandbox_terminate(
        &self,
        Parameters(params): Parameters<SandboxTerminateParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Terminating sandbox");
        r#"{"status": "terminated"}"#.to_string()
    }

    #[tool(description = "Get information about a sandbox")]
    async fn sandbox_info(
        &self,
        Parameters(params): Parameters<SandboxInfoParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Getting sandbox info");
        // TODO: Delegate to GetSandboxInfoUseCase
        format!(r#"{{"sandbox_id": "{}", "status": "running"}}"#, params.sandbox_id)
    }
}
