//! Artifact store — fetches artifact content bytes for materializers.

use async_trait::async_trait;

use crate::shared::DomainError;

/// Fetches the actual content (bytes) of a template artifact.
///
/// Implementations can be local filesystem, OCI registry, HTTP, etc.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Fetch all content bytes for the given artifact digest.
    async fn fetch(&self, artifact_id: &str, digest: &str) -> Result<Vec<u8>, DomainError>;

    /// Check if the artifact is available locally (cached).
    async fn is_cached(&self, artifact_id: &str, digest: &str) -> Result<bool, DomainError>;

    /// Get the local cache path where artifacts are stored.
    fn local_cache_path(&self) -> std::path::PathBuf;
}
