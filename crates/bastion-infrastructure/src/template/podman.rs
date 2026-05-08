//! Podman-optimized materializer.
//!
//! Uses host-side extraction + `podman cp` instead of `podman exec tar xf`,
//! and shares extracted artifacts via host cache across sandboxes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::template::{
    ArtifactStore, MaterializationMode, MaterializationResult, ProviderKind, ProviderMaterializer,
    TemplateArtifact,
};

/// Podman-optimized materializer.
///
/// Strategy:
/// 1. Extract artifact to host cache (once)
/// 2. Copy from host cache into sandbox via `podman cp` (fast)
/// 3. Verify capabilities inside sandbox
///
/// Future optimization: bind mount at container creation time
/// to avoid copying entirely.
pub struct PodmanOptimizedMaterializer<S: ArtifactStore> {
    provider: Arc<dyn SandboxProvider>,
    store: Arc<S>,
    /// Host-side cache where artifacts are extracted.
    cache_root: PathBuf,
}

impl<S: ArtifactStore> PodmanOptimizedMaterializer<S> {
    pub fn new(provider: Arc<dyn SandboxProvider>, store: Arc<S>, cache_root: PathBuf) -> Self {
        Self {
            provider,
            store,
            cache_root,
        }
    }

    /// Get the host-side extraction path for an artifact.
    fn host_cache_path(&self, artifact: &TemplateArtifact) -> PathBuf {
        self.cache_root.join("artifacts").join(&artifact.digest)
    }

    /// Ensure the artifact is extracted to the host cache.
    async fn ensure_host_cache(
        &self,
        artifact: &TemplateArtifact,
    ) -> Result<(PathBuf, bool), DomainError> {
        let target = self.host_cache_path(artifact);

        // Already extracted?
        if target.join(".extracted").exists() {
            return Ok((target, true));
        }

        // Fetch artifact bytes
        let content = self
            .store
            .fetch(&artifact.id.to_string(), &artifact.digest)
            .await?;

        // Create directory and write tar
        tokio::fs::create_dir_all(&target)
            .await
            .map_err(|e| DomainError::Internal(format!("mkdir: {}", e)))?;

        let tar_path = target.join("artifact.tar");
        tokio::fs::write(&tar_path, &content)
            .await
            .map_err(|e| DomainError::Internal(format!("write tar: {}", e)))?;

        // Extract on host using system tar
        let output = tokio::process::Command::new("tar")
            .args([
                "xf",
                tar_path.to_str().unwrap_or("/dev/null"),
                "-C",
                target.to_str().unwrap_or("/dev/null"),
            ])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("host tar: {}", e)))?;

        if !output.status.success() {
            // Try gzip
            let output2 = tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    tar_path.to_str().unwrap_or("/dev/null"),
                    "-C",
                    target.to_str().unwrap_or("/dev/null"),
                ])
                .output()
                .await
                .map_err(|e| DomainError::Internal(format!("host tar xzf: {}", e)))?;

            if !output2.status.success() {
                let stderr = String::from_utf8_lossy(&output2.stderr);
                return Err(DomainError::Internal(format!(
                    "Failed to extract artifact on host: {}",
                    stderr
                )));
            }
        }

        // Cleanup tar and mark as extracted
        let _ = tokio::fs::remove_file(&tar_path).await;
        let _ = tokio::fs::write(target.join(".extracted"), b"ok").await;

        Ok((target, false))
    }

    /// Copy the extracted host cache into the sandbox via the provider's copy_to (bollard put_archive).
    async fn copy_to_sandbox(
        &self,
        sandbox_id: &SandboxId,
        host_dir: &Path,
        container_target: &str,
    ) -> Result<(), DomainError> {
        // Create target directory
        let mkdir_cmd = bastion_domain::execution::command::CommandSpec::new(format!(
            "mkdir -p {}",
            container_target
        ));
        let _ = self.provider.run_command(sandbox_id, &mkdir_cmd).await;

        // Use provider's copy_to (bollard upload_to_container) instead of podman cp CLI
        self.provider.copy_to(sandbox_id, host_dir, container_target).await
    }

    /// Run verification steps inside the sandbox.
    async fn verify_capabilities(
        &self,
        sandbox_id: &SandboxId,
        artifact: &TemplateArtifact,
    ) -> Result<(), DomainError> {
        for cap in &artifact.capabilities {
            for step in &cap.verification {
                let mut cmd = bastion_domain::execution::command::CommandSpec::new(&step.command);

                // Add env vars
                for (k, v) in &artifact.env.env {
                    cmd = cmd.with_env(k.as_str(), v.as_str());
                }
                if let Some(path_prefix) = artifact.env.path_prefix.first() {
                    cmd = cmd.with_env("PATH", format!("{}:$PATH", path_prefix));
                }

                let result = self.provider.run_command(sandbox_id, &cmd).await?;

                if result.exit_code != step.expected_exit_code {
                    return Err(DomainError::Validation(format!(
                        "Verification '{}' failed: exit code {} (expected {})",
                        step.label, result.exit_code, step.expected_exit_code
                    )));
                }

                if let Some(expected) = &step.expected_output_contains {
                    let output = String::from_utf8_lossy(&result.stdout);
                    let combined = format!("{}{}", output, String::from_utf8_lossy(&result.stderr));
                    if !combined.to_lowercase().contains(&expected.to_lowercase()) {
                        return Err(DomainError::Validation(format!(
                            "Verification '{}' failed: expected '{}' in output",
                            step.label, expected
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<S: ArtifactStore + Sync + Send> ProviderMaterializer for PodmanOptimizedMaterializer<S> {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Podman
    }

    async fn can_materialize(&self, artifact: &TemplateArtifact) -> Result<bool, DomainError> {
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

        let mount_path = format!("/opt/bastion/artifacts/{}", artifact.digest);

        match mode {
            MaterializationMode::MountReadonly | MaterializationMode::Auto => {
                // Step 1: Extract to host cache (once)
                let (host_dir, _cache_hit) = self.ensure_host_cache(artifact).await?;

                // Step 2: Copy to sandbox via podman cp
                self.copy_to_sandbox(sandbox_id, &host_dir, &mount_path)
                    .await?;
            }
            MaterializationMode::Extract => {
                // Fallback: use extraction inside sandbox (already implemented in UniversalMaterializer)
                // For now, we do the same copy approach
                let (host_dir, _cache_hit) = self.ensure_host_cache(artifact).await?;
                self.copy_to_sandbox(sandbox_id, &host_dir, &mount_path)
                    .await?;
            }
            _ => {
                return Err(DomainError::UnsupportedOperation(format!(
                    "Podman optimized does not support mode {:?}",
                    mode
                )));
            }
        }

        // Verify
        self.verify_capabilities(sandbox_id, artifact).await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(MaterializationResult {
            sandbox_id: sandbox_id.clone(),
            artifact_id: artifact.id.to_string(),
            mode: MaterializationMode::MountReadonly,
            cache_hit: true,
            mount_path,
            env_ref: Some(format!(
                "materialized:{}:{}",
                artifact.name, artifact.digest
            )),
            duration_ms,
        })
    }

    fn cache_path(&self) -> PathBuf {
        self.cache_root.clone()
    }
}
