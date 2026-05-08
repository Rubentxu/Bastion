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
    /// Template (base image) for the sandbox
    pub template: String,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Provider to use: podman, firecracker, gvisor (default: podman)
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
    /// ID of the sandbox
    pub sandbox_id: String,
    /// Command to execute
    pub command: String,
    /// Optional environment reference from sandbox_prepare
    #[serde(default)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
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
pub struct SandboxCancelParams {
    pub sandbox_id: String,
    /// Grace period in milliseconds before sending SIGKILL after SIGTERM. Default: 5000ms
    #[serde(default = "default_grace_period_ms")]
    pub grace_period_ms: u64,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_grace_period_ms() -> u64 {
    5000
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
    /// Optional environment reference from sandbox_prepare
    #[serde(default)]
    #[allow(dead_code)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterArtifactParams {
    pub name: String,
    pub version: String,
    pub digest: String,
    pub capability: String,
    #[serde(default)]
    pub tools: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxPrepareParams {
    pub sandbox_id: String,
    pub capability: String,
    /// Timeout in ms for the entire prepare operation (default: 600s for network-heavy ops)
    /// Reserved for future use — currently uses a fixed timeout
    #[serde(default = "default_prepare_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Toolchain strategy override (default: auto)
    ///
    /// - "auto": Let the resolver pick the best approach
    /// - "system_package": Prefer system package managers (apt)
    /// - "version_manager": Prefer version managers (asdf, sdkman)
    /// - "content_addressed": Use pre-packaged artifacts from CA store
    #[serde(default)]
    pub strategy: bastion_domain::template::ToolchainStrategy,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_prepare_timeout() -> u64 {
    600_000 // 10 minutes — covers apt-get install + downloads
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSnapshotParams {
    pub action: String, // "create", "restore", "list", "delete"
    pub sandbox_id: Option<String>,
    pub name: Option<String>,
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSyncParams {
    pub sandbox_id: String,
    pub mode: String, // "push", "pull", "auto"
    pub source: String,
    pub target: String,
    /// Exclude patterns for sync (reserved for future rsync --exclude support)
    #[serde(default)]
    #[allow(dead_code)]
    pub exclude: Vec<String>,
    /// Optional sync backend override: tar, rsync, podman-cp, auto
    #[serde(default)]
    pub backend: Option<String>,
    /// Timeout in ms (default: 300s for large transfers)
    /// Reserved for future use — currently uses backend-level defaults
    #[serde(default = "default_sync_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_sync_timeout() -> u64 {
    300_000 // 5 minutes
}
