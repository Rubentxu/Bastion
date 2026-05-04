//! Zip-layer materializer for FaaS compatible layer artifacts.
//!
//! Packages template artifacts as zip layers and deploys them
//! to `/opt/bastion/layers/<digest>` in the execution environment.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{
    ArtifactStore, LayerArtifact, MaterializationMode, MaterializationResult, ProviderKind,
    ProviderMaterializer, TemplateArtifact, LAYER_MOUNT_PREFIX,
};

/// Materializer that deploys artifacts as zip layers.
///
/// Strategy:
/// 1. Fetch artifact content from store
/// 2. Create zip archive from extracted files
/// 3. Deploy to `/opt/bastion/layers/<digest>` in sandbox
/// 4. Returns LayerArtifact with mount info
pub struct ZipLayerMaterializer<S: ArtifactStore> {
    provider: Arc<dyn SandboxProvider>,
    store: Arc<S>,
    cache_root: PathBuf,
}

impl<S: ArtifactStore> ZipLayerMaterializer<S> {
    pub fn new(provider: Arc<dyn SandboxProvider>, store: Arc<S>, cache_root: PathBuf) -> Self {
        Self {
            provider,
            store,
            cache_root,
        }
    }

    /// Build a zip layer from the artifact content and cache it on host.
    async fn build_zip_layer(
        &self,
        artifact: &TemplateArtifact,
    ) -> Result<Vec<u8>, DomainError> {
        let zip_path = self
            .cache_root
            .join("layers")
            .join(format!("{}.zip", artifact.digest));

        // Check cache
        if zip_path.exists() {
            return tokio::fs::read(&zip_path)
                .await
                .map_err(|e| DomainError::Internal(format!("read cached zip: {}", e)));
        }

        // Fetch artifact content (tar)
        let tar_bytes = self
            .store
            .fetch(&artifact.id.to_string(), &artifact.digest)
            .await?;

        // Create zip from tar content
        let zip_bytes = self.tar_to_zip(&tar_bytes)?;

        // Cache
        if let Some(parent) = zip_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&zip_path, &zip_bytes)
            .await
            .map_err(|e| DomainError::Internal(format!("write zip cache: {}", e)))?;

        Ok(zip_bytes)
    }

    /// Convert a tar archive to a zip archive (simple repackaging).
    fn tar_to_zip(&self, tar_bytes: &[u8]) -> Result<Vec<u8>, DomainError> {
        let mut tar_archive = tar::Archive::new(tar_bytes);
        let zip_buffer = Vec::new();
        let cursor = std::io::Cursor::new(zip_buffer);
        let mut zip_writer = zip::ZipWriter::new(cursor);

        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for entry in tar_archive.entries().map_err(|e| {
            DomainError::Internal(format!("tar entries: {}", e))
        })? {
            let mut entry = entry.map_err(|e| {
                DomainError::Internal(format!("tar entry: {}", e))
            })?;

            let path = entry.path().map_err(|e| {
                DomainError::Internal(format!("tar path: {}", e))
            })?;

            if path.as_os_str().is_empty() || entry.header().entry_type().is_dir() {
                continue;
            }

            zip_writer
                .start_file(path.to_string_lossy().as_ref(), options)
                .map_err(|e| DomainError::Internal(format!("zip start: {}", e)))?;

            let mut contents = Vec::new();
            std::io::copy(&mut entry, &mut contents)
                .map_err(|e| DomainError::Internal(format!("tar read: {}", e)))?;
            zip_writer
                .write_all(&contents)
                .map_err(|e| DomainError::Internal(format!("zip write: {}", e)))?;
        }

        let finished = zip_writer
            .finish()
            .map_err(|e| DomainError::Internal(format!("zip finish: {}", e)))?;

        Ok(finished.into_inner())
    }

    /// Deploy the zip layer to the sandbox at `/opt/bastion/layers/<digest>`.
    async fn deploy_to_sandbox(
        &self,
        sandbox_id: &SandboxId,
        zip_bytes: &[u8],
        target_dir: &str,
    ) -> Result<String, DomainError> {
        use bastion_domain::execution::command::CommandSpec;

        // Create target directory
        let mkdir = CommandSpec::new(format!("mkdir -p {}", target_dir));
        let _ = self.provider.run_command(sandbox_id, &mkdir).await;

        // Write zip to sandbox
        let zip_path = format!("{}/layer.zip", target_dir);
        self.provider
            .write_file(sandbox_id, &zip_path, zip_bytes)
            .await?;

        // Extract zip
        let extract = CommandSpec::new(format!(
            "unzip -o {} -d {} 2>/dev/null || python3 -c \"import zipfile; zipfile.ZipFile('{}').extractall('{}')\" 2>/dev/null || true",
            zip_path, target_dir, zip_path, target_dir
        ))
        .with_timeout(60_000);
        let _ = self.provider.run_command(sandbox_id, &extract).await;

        // Cleanup zip
        let rm = CommandSpec::new(format!("rm -f {}", zip_path));
        let _ = self.provider.run_command(sandbox_id, &rm).await;

        Ok(target_dir.to_string())
    }

    /// Create a LayerArtifact from the template for tracking.
    fn to_layer(&self, artifact: &TemplateArtifact) -> LayerArtifact {
        LayerArtifact::new(
            artifact.clone(),
            Some(format!(
                "Layer for capability: {}",
                artifact
                    .capabilities
                    .first()
                    .map(|c| c.name.as_str())
                    .unwrap_or("unknown")
            )),
        )
    }
}

#[async_trait]
impl<S: ArtifactStore + Sync + Send> ProviderMaterializer for ZipLayerMaterializer<S> {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::FaaS
    }

    async fn can_materialize(
        &self,
        artifact: &TemplateArtifact,
    ) -> Result<bool, DomainError> {
        self.store
            .is_cached(&artifact.id.to_string(), &artifact.digest)
            .await
    }

    async fn materialize(
        &self,
        sandbox_id: &SandboxId,
        artifact: &TemplateArtifact,
        mode: MaterializationMode,
    ) -> Result<MaterializationResult, DomainError> {
        let start = Instant::now();

        if artifact.security.contains_secrets {
            return Err(DomainError::PermissionDenied(
                "Artifact contains secrets".into(),
            ));
        }

        // Build the zip layer
        let zip_bytes = self.build_zip_layer(artifact).await?;

        // Determine mount path
        let mount_path = format!("{}/{}", LAYER_MOUNT_PREFIX, artifact.digest);

        match mode {
            MaterializationMode::AttachLayer | MaterializationMode::Auto => {
                // Deploy as zip layer
                self.deploy_to_sandbox(sandbox_id, &zip_bytes, &mount_path)
                    .await?;
            }
            _ => {
                return Err(DomainError::UnsupportedOperation(format!(
                    "ZipLayer only supports AttachLayer mode, got {:?}",
                    mode
                )));
            }
        }

        // Create layer artifact for reference
        let layer = self.to_layer(artifact);

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(MaterializationResult {
            sandbox_id: sandbox_id.clone(),
            artifact_id: artifact.id.to_string(),
            mode: MaterializationMode::AttachLayer,
            cache_hit: true,
            mount_path,
            env_ref: Some(format!("layer:{}:{}", layer.arn, artifact.digest)),
            duration_ms,
        })
    }

    fn cache_path(&self) -> PathBuf {
        self.cache_root.clone()
    }
}
