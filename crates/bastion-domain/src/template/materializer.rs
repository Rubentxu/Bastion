//! Materializer trait — abstraction for cross-provider template materialization.

use async_trait::async_trait;

use super::artifact::TemplateArtifact;
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// How to materialize the artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterializationMode {
    /// Let the provider choose the best strategy.
    Auto,
    /// Mount readonly (zero-copy if supported).
    MountReadonly,
    /// Extract artifact into sandbox filesystem.
    Extract,
    /// Bake into the container/VM image.
    BakeIntoImage,
    /// Restore from a snapshot (e.g. Firecracker).
    RestoreSnapshot,
    /// Attach as an external layer (e.g. Lambda layer).
    AttachLayer,
    /// Lazy remote layer (e.g. stargz/nydus).
    LazyRemote,
}

/// What kind of provider is materializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Podman,
    Docker,
    Containerd,
    Kubernetes,
    GVisor,
    Firecracker,
    VirtualMachine,
    FaaS,
    Custom,
    /// WebAssembly-based sandbox provider.
    Wasm,
    /// Local provider that runs commands directly on host (DANGEROUS).
    Local,
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::Podman => write!(f, "podman"),
            ProviderKind::Docker => write!(f, "docker"),
            ProviderKind::Containerd => write!(f, "containerd"),
            ProviderKind::Kubernetes => write!(f, "kubernetes"),
            ProviderKind::GVisor => write!(f, "gvisor"),
            ProviderKind::Firecracker => write!(f, "firecracker"),
            ProviderKind::VirtualMachine => write!(f, "vm"),
            ProviderKind::FaaS => write!(f, "faas"),
            ProviderKind::Custom => write!(f, "custom"),
            ProviderKind::Wasm => write!(f, "wasm"),
            ProviderKind::Local => write!(f, "local"),
        }
    }
}

/// Result of materializing a template artifact.
#[derive(Debug, Clone)]
pub struct MaterializationResult {
    /// The sandbox the artifact was materialized into.
    pub sandbox_id: SandboxId,
    /// The ID of the materialized artifact.
    pub artifact_id: String,
    /// Which mode was actually used.
    pub mode: MaterializationMode,
    /// Whether the artifact was found in cache.
    pub cache_hit: bool,
    /// The path where the artifact was materialized.
    pub mount_path: String,
    /// Environment variables that were configured.
    pub env_ref: Option<String>,
    /// Duration of materialization in milliseconds.
    pub duration_ms: u64,
}

/// Cross-provider materializer trait.
///
/// Each provider backend implements this to materialize template artifacts
/// into sandboxes.
#[async_trait]
pub trait ProviderMaterializer: Send + Sync {
    /// Which provider kind this materializer belongs to.
    fn provider_kind(&self) -> ProviderKind;

    /// Check whether the provider can materialize this artifact.
    async fn can_materialize(
        &self,
        artifact: &TemplateArtifact,
    ) -> Result<bool, DomainError>;

    /// Materialize the artifact into the sandbox.
    async fn materialize(
        &self,
        sandbox_id: &SandboxId,
        artifact: &TemplateArtifact,
        mode: MaterializationMode,
    ) -> Result<MaterializationResult, DomainError>;

    /// Get the cache path for this provider.
    fn cache_path(&self) -> std::path::PathBuf;
}
