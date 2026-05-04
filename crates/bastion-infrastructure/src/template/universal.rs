//! Universal extract materializer.
//!
//! Fallback materializer that works with any provider by:
//! 1. Fetching artifact content from store
//! 2. Uploading via write_file to sandbox
//! 3. Extracting via run_command
//! 4. Verifying capabilities

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{
    ArtifactStore, MaterializationMode, MaterializationResult, ProviderKind,
    ProviderMaterializer, TemplateArtifact,
};

/// Universal materializer that works via SandboxProvider.
pub struct UniversalMaterializer<S: ArtifactStore> {
    provider: Arc<dyn SandboxProvider>,
    store: Arc<S>,
    cache_root: PathBuf,
}

impl<S: ArtifactStore> UniversalMaterializer<S> {
    pub fn new(provider: Arc<dyn SandboxProvider>, store: Arc<S>, cache_root: PathBuf) -> Self {
        Self {
            provider,
            store,
            cache_root,
        }
    }

    /// Extract an artifact to a specific path in the sandbox.
    async fn extract_to(
        &self,
        sandbox_id: &SandboxId,
        artifact: &TemplateArtifact,
        target_path: &str,
    ) -> Result<(), DomainError> {
        // 1. Fetch artifact content
        let content = self
            .store
            .fetch(&artifact.id.to_string(), &artifact.digest)
            .await?;

        // 2. Write tar to sandbox
        let tar_path = format!("{}/artifact.tar.gz", target_path);
        let _ = self
            .provider
            .run_command(
                sandbox_id,
                &CommandSpec::new(format!("mkdir -p {}", target_path)),
            )
            .await;

        self.provider
            .write_file(sandbox_id, &tar_path, &content)
            .await?;

        // 3. Extract (try gzip first, fallback to plain tar)
        // First try: tar xzf (gzip)
        let cmd = CommandSpec::new(format!(
            "tar xzf {} -C {} 2>/dev/null || tar xf {} -C {}",
            tar_path, target_path, tar_path, target_path
        ))
        .with_timeout(120_000);

        let result = self.provider.run_command(sandbox_id, &cmd).await?;

        if !result.is_success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(DomainError::CommandFailed {
                exit_code: result.exit_code,
                stderr: stderr.to_string(),
            });
        }

        // 4. Cleanup tar
        let _ = self
            .provider
            .run_command(
                sandbox_id,
                &CommandSpec::new(format!("rm -f {}", tar_path)),
            )
            .await;

        Ok(())
    }

    /// Run verification steps for the installed capability.
    async fn verify_capabilities(
        &self,
        sandbox_id: &SandboxId,
        artifact: &TemplateArtifact,
        env: &std::collections::HashMap<String, String>,
    ) -> Result<(), DomainError> {
        for cap in &artifact.capabilities {
            for step in &cap.verification {
                let mut cmd = CommandSpec::new(&step.command);
                for (k, v) in env {
                    cmd = cmd.with_env(k.as_str(), v.as_str());
                }
                // Add PATH prefix if configured
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
                            "Verification '{}' failed: expected '{}' in output, got: {}",
                            step.label,
                            expected,
                            &combined[..combined.len().min(200)]
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<S: ArtifactStore + Sync + Send> ProviderMaterializer for UniversalMaterializer<S> {
    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::Custom // Universal fallback
    }

    async fn can_materialize(
        &self,
        artifact: &TemplateArtifact,
    ) -> Result<bool, DomainError> {
        // Universal can always materialize artifacts that are in the store
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

        // Security: reject artifacts containing secrets
        if artifact.security.contains_secrets {
            return Err(DomainError::PermissionDenied(
                "Artifact contains secrets and cannot be materialized as template".into(),
            ));
        }

        let mount_path = format!("/opt/bastion/artifacts/{}", artifact.digest);
        let cache_hit = self
            .store
            .is_cached(&artifact.id.to_string(), &artifact.digest)
            .await
            .unwrap_or(false);

        match mode {
            MaterializationMode::Extract | MaterializationMode::Auto => {
                // Extract to /opt/bastion/artifacts/<digest>
                self.extract_to(sandbox_id, artifact, &mount_path).await?;
            }
            _ => {
                return Err(DomainError::UnsupportedOperation(format!(
                    "Universal materializer does not support mode {:?}",
                    mode
                )));
            }
        }

        // Configure environment
        let mut env = std::collections::HashMap::new();
        for (k, v) in &artifact.env.env {
            env.insert(k.clone(), v.clone());
        }

        // Run verification
        self.verify_capabilities(sandbox_id, artifact, &env).await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(MaterializationResult {
            sandbox_id: sandbox_id.clone(),
            artifact_id: artifact.id.to_string(),
            mode: MaterializationMode::Extract,
            cache_hit,
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
