//! Filesystem-based artifact store.
//!
//! Stores artifacts as files in a directory, keyed by artifact ID and digest.

use std::path::PathBuf;

use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use bastion_domain::template::ArtifactStore;

/// Simple filesystem-backed artifact store.
pub struct FsArtifactStore {
    root: PathBuf,
}

impl FsArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn artifact_path(&self, artifact_id: &str, digest: &str) -> PathBuf {
        // Use digest as filename for content-addressing
        self.root.join(format!(
            "{}-{}.tar.gz",
            artifact_id.replace('/', "-"),
            &digest[..digest.len().min(16)]
        ))
    }
}

#[async_trait]
impl ArtifactStore for FsArtifactStore {
    async fn fetch(&self, artifact_id: &str, digest: &str) -> Result<Vec<u8>, DomainError> {
        let path = self.artifact_path(artifact_id, digest);
        tokio::fs::read(&path).await.map_err(|e| {
            DomainError::NotFound(format!(
                "Artifact {} not found at {}: {}",
                artifact_id,
                path.display(),
                e
            ))
        })
    }

    async fn is_cached(&self, artifact_id: &str, digest: &str) -> Result<bool, DomainError> {
        let path = self.artifact_path(artifact_id, digest);
        Ok(path.exists())
    }

    fn local_cache_path(&self) -> PathBuf {
        self.root.clone()
    }
}

/// Store an artifact from bytes.
impl FsArtifactStore {
    pub async fn store(
        &self,
        artifact_id: &str,
        digest: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        let path = self.artifact_path(artifact_id, digest);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| DomainError::Internal(format!("Failed to create store dir: {}", e)))?;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to write artifact: {}", e)))?;
        Ok(())
    }
}
