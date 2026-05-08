//! Sandbox tool parameter types and defaults.

use rmcp::{schemars, tool};
use schemars::JsonSchema;
use serde::Deserialize;

// ─── Sync Backend ─────────────────────────────────────────────────────────────

/// Sync backend selection for sandbox file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncBackend {
    /// Use tar piped via podman exec (most compatible)
    Tar,
    /// Use rsync (fastest for large trees, requires rsync in sandbox)
    Rsync,
    /// Use podman cp (simplest, but limited)
    PodmanCp,
    /// Auto-detect best backend
    Auto,
}

// ─── Tool parameter types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxCreateParams {
    /// Template (base image) for the sandbox, e.g. "debian:bookworm-slim". Use sandbox_list_templates to see available images.
    pub template: String,
    /// Timeout in milliseconds for sandbox creation (default: 3.6M ms = 1 hour). Creation fails if exceeded.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Provider to use: podman, firecracker, gvisor (default: podman). Must support the template image.
    #[serde(default = "default_provider_name")]
    pub provider: String,
}

fn default_timeout() -> u64 {
    3_600_000
}

fn default_provider_name() -> String {
    "podman".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunParams {
    /// ID of the sandbox to run the command in. Must be running (created + prepared if capability needed).
    pub sandbox_id: String,
    /// Command to execute inside the sandbox (e.g. "mvn --version", "node index.js").
    pub command: String,
    /// Optional environment reference from sandbox_prepare. If omitted, auto-injects env from most recent sandbox_prepare for this sandbox_id.
    /// Pass explicitly in concurrent workflows to avoid race conditions.
    #[serde(default)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate this command with other tools across the same workflow.
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxWriteParams {
    /// ID of the sandbox to write the file into.
    pub sandbox_id: String,
    /// Destination path inside the sandbox. Parent directories created automatically.
    pub path: String,
    /// File content as a string. For binary content, prefer sandbox_sync (push) instead.
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxReadParams {
    /// ID of the sandbox to read from.
    pub sandbox_id: String,
    /// Path of the file to read inside the sandbox.
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxTerminateParams {
    /// ID of the sandbox to terminate and destroy. Call when done to free resources.
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxCancelParams {
    /// ID of the sandbox whose running command to cancel.
    pub sandbox_id: String,
    /// Grace period in milliseconds before SIGKILL (after SIGTERM). Default: 5000ms. Increase for long cleanup scripts.
    #[serde(default = "default_grace_period_ms")]
    pub grace_period_ms: u64,
    /// Optional trace ID to correlate this cancel with other tools across the same workflow.
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_grace_period_ms() -> u64 {
    5000
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxInfoParams {
    /// ID of the sandbox to query. Use sandbox_list to discover sandbox_ids.
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxListFilesParams {
    /// ID of the sandbox to list files in.
    pub sandbox_id: String,
    /// Directory path to list. Use "/" for root or "." for current working directory inside sandbox.
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunStreamParams {
    /// ID of the sandbox to run the command in. Must be running.
    pub sandbox_id: String,
    /// Command to execute inside the sandbox with streaming output.
    pub command: String,
    /// Optional environment reference from sandbox_prepare. Auto-injected if omitted. Pass explicitly in concurrent workflows.
    #[serde(default)]
    #[allow(dead_code)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate this command with other tools across the same workflow.
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterArtifactParams {
    /// Artifact name, e.g. "my-jdk-artifact".
    pub name: String,
    /// Artifact version, e.g. "1.0.0".
    pub version: String,
    /// SHA256 digest of the artifact for content-addressed retrieval.
    pub digest: String,
    /// Capability this artifact provides, e.g. "jvm-build". Used by sandbox_prepare to find artifacts.
    pub capability: String,
    /// Comma-separated list of tools provided, e.g. "mvn:3.9,javac:17". Version is optional.
    #[serde(default)]
    pub tools: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxPrepareParams {
    /// ID of the sandbox to prepare. Sandbox must exist (created via sandbox_create).
    pub sandbox_id: String,
    /// Capability to install, e.g. "jvm-build" (Java+Maven) or "node-build" (Node.js+npm).
    pub capability: String,
    /// Timeout in ms for the entire prepare operation (default: 600s = 10min). Covers downloads and installs.
    #[serde(default = "default_prepare_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Toolchain strategy override (default: auto). Values: "auto" (resolver picks), "system_package" (apt), "version_manager" (asdf/sdkman), "content_addressed" (CA store).
    #[serde(default)]
    pub strategy: bastion_domain::template::ToolchainStrategy,
    /// Optional trace ID to correlate this prepare with other tools across the same workflow.
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_prepare_timeout() -> u64 {
    600_000 // 10 minutes — covers apt-get install + downloads
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSnapshotParams {
    /// Action to perform: "create", "restore", "list", or "delete".
    pub action: String,
    /// Sandbox ID required for "create" action. Not needed for list/delete/restore.
    pub sandbox_id: Option<String>,
    /// Snapshot name — required for "create" action.
    pub name: Option<String>,
    /// Snapshot ID — required for "restore" and "delete" actions.
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSyncParams {
    /// ID of the sandbox for file transfer.
    pub sandbox_id: String,
    /// Sync direction: "push" (host→sandbox) or "pull" (sandbox→host).
    pub mode: String,
    /// Source path: local path for push, sandbox path for pull. Path must exist.
    pub source: String,
    /// Target path: sandbox path for push, local path for pull. Parent dirs created automatically.
    pub target: String,
    /// Exclude patterns for rsync sync (reserved for future rsync --exclude support).
    #[serde(default)]
    #[allow(dead_code)]
    pub exclude: Vec<String>,
    /// Sync backend override: "tar" (default for rootless podman), "rsync" (fastest, requires rsync in sandbox), "podman-cp" (simplest), "auto" (backend picks).
    #[serde(default)]
    pub backend: Option<String>,
    /// Timeout in ms for sync operation (default: 300s = 5min). Increase for large transfers.
    #[serde(default = "default_sync_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Optional trace ID to correlate this sync with other tools across the same workflow.
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_sync_timeout() -> u64 {
    300_000 // 5 minutes
}
