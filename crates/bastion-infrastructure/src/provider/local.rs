//! LocalProvider — runs commands directly on the host filesystem.
//!
//! **WARNING**: This provider executes commands on the host machine with the
//! same privileges as the Bastion process. It requires `DANGEROUS_ALLOW_LOCAL=1`
//! environment variable to be set, or the `--dangerous-allow-local` CLI flag.
//!
//! This provider is intended for development and testing only.

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::execution::stream::CommandChunk;
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Stream type for command output chunks.
pub type LocalCommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// Local sandbox provider that executes commands directly on the host filesystem.
///
/// # Security
///
/// This provider is **DANGEROUS** — it runs commands with the same privileges as
/// the Bastion process. It requires `DANGEROUS_ALLOW_LOCAL=1` to be set.
pub struct LocalProvider {
    /// Workspaces: sandbox ID -> workspace directory
    workspaces: Arc<RwLock<HashMap<SandboxId, PathBuf>>>,
    /// Sandbox entities
    sandboxes: Arc<RwLock<HashMap<SandboxId, Sandbox>>>,
    /// Base directory for all workspaces
    base_dir: PathBuf,
    /// Whether to clean up workspaces on terminate
    cleanup: bool,
}

impl LocalProvider {
    /// Create a new LocalProvider.
    ///
    /// # Errors
    ///
    /// Returns `DomainError::PermissionDenied` if `DANGEROUS_ALLOW_LOCAL`
    /// environment variable is not set.
    pub fn new(base_dir: PathBuf) -> Result<Self, DomainError> {
        if std::env::var("DANGEROUS_ALLOW_LOCAL").is_err() {
            return Err(DomainError::PermissionDenied(
                "LocalProvider requires DANGEROUS_ALLOW_LOCAL=1 for security".into(),
            ));
        }
        Ok(Self {
            workspaces: Arc::new(RwLock::new(HashMap::new())),
            sandboxes: Arc::new(RwLock::new(HashMap::new())),
            base_dir,
            cleanup: true,
        })
    }

    /// Returns the base directory for workspaces.
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }
}

impl std::fmt::Debug for LocalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProvider")
            .field("base_dir", &self.base_dir)
            .field("cleanup", &self.cleanup)
            .finish()
    }
}

#[async_trait]
impl SandboxProvider for LocalProvider {
    fn name(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 0, // Unlimited
            max_cpu_count: 0, // Unlimited
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 10,
        }
    }

    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        _timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        let dir_name = format!("local-{}", id);
        let workspace = self.base_dir.join(&dir_name);

        // Create workspace directory
        std::fs::create_dir_all(&workspace)
            .map_err(|e| DomainError::Internal(format!("Failed to create workspace: {e}")))?;

        // Record workspace
        self.workspaces.write().await.insert(id.clone(), workspace);

        // Create sandbox entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("local"),
            _resources.clone(),
            _network.clone(),
        );

        sandbox.mark_running()?;
        sandbox.set_timeout(_timeout_ms);

        self.sandboxes
            .write()
            .await
            .insert(id.clone(), sandbox.clone());

        tracing::info!(sandbox_id = %id, workspace = %dir_name, "LocalProvider: created sandbox");
        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        // Remove workspace
        if let Some(ws) = self.workspaces.write().await.remove(id)
            && self.cleanup
            && let Err(e) = std::fs::remove_dir_all(&ws)
        {
            tracing::warn!(sandbox_id = %id, error = %e, "Failed to remove workspace");
        }

        // Remove sandbox entity
        self.sandboxes.write().await.remove(id);

        tracing::info!(sandbox_id = %id, "LocalProvider: terminated sandbox");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        Ok(self.sandboxes.read().await.contains_key(id))
    }

    /// Cancel a running command in the local sandbox.
    ///
    /// For LocalProvider, command cancellation is best-effort. Since commands
    /// run via `std::process::Command::output()` (synchronous), we cannot
    /// cancel mid-execution. However, if the command was spawned with
    /// process group tracking, we can signal the group.
    ///
    /// Currently returns Ok(false) (no running command found) as the
    /// synchronous execution model doesn't support mid-execution cancellation.
    async fn cancel_command(
        &self,
        id: &SandboxId,
        grace_period_ms: u64,
    ) -> Result<bool, DomainError> {
        tracing::info!(sandbox_id = %id, grace_period_ms, "Cancelling running command (LocalProvider) — best-effort");
        // LocalProvider currently runs commands synchronously via std::process::Command::output()
        // which doesn't support mid-execution cancellation.
        // Future improvement: track spawned processes and signal their process groups.
        Ok(false)
    }

    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        let ws = self
            .workspaces
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let t0 = std::time::Instant::now();

        let mut cmd = std::process::Command::new(&command.command);
        cmd.args(&command.args);
        cmd.current_dir(&ws);
        for (k, v) in &command.env_vars {
            cmd.env(k, v);
        }

        let output = cmd
            .output()
            .map_err(|e| DomainError::Internal(format!("Failed to execute command: {e}")))?;

        let duration_ms = t0.elapsed().as_millis() as u64;

        Ok(CommandResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms,
            timed_out: false,
        })
    }

    async fn run_command_stream(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<LocalCommandStream, DomainError> {
        // Run the command and stream results
        let result = self.run_command(id, command).await?;

        let (tx, rx) = mpsc::channel::<Result<CommandChunk, DomainError>>(4);

        tokio::spawn(async move {
            // Send stdout
            if !result.stdout.is_empty() {
                let _ = tx
                    .send(Ok(CommandChunk::stdout(result.stdout.clone())))
                    .await;
            }
            // Send stderr
            if !result.stderr.is_empty() {
                let _ = tx
                    .send(Ok(CommandChunk::stderr(result.stderr.clone())))
                    .await;
            }
            // Send exit code
            let _ = tx.send(Ok(CommandChunk::exit_code(result.exit_code))).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        let ws = self
            .workspaces
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let full_path = ws.join(path.trim_start_matches('/'));

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::Internal(format!("Failed to create directory: {e}")))?;
        }

        std::fs::write(&full_path, content)
            .map_err(|e| DomainError::Internal(format!("Failed to write file: {e}")))?;

        Ok(())
    }

    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
        let ws = self
            .workspaces
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let full_path = ws.join(path.trim_start_matches('/'));

        std::fs::read(&full_path)
            .map_err(|e| DomainError::Internal(format!("Failed to read file: {e}")))
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        let ws = self
            .workspaces
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let full_path = ws.join(dir.trim_start_matches('/'));
        let mut entries = Vec::new();

        if full_path.is_dir() {
            for entry in std::fs::read_dir(&full_path)
                .map_err(|e| DomainError::Internal(format!("Failed to read directory: {e}")))?
            {
                let entry = entry.map_err(|e| DomainError::Internal(e.to_string()))?;

                let metadata = entry
                    .metadata()
                    .map_err(|e| DomainError::Internal(e.to_string()))?;
                entries.push(FileEntry {
                    path: entry.path().to_string_lossy().to_string(),
                    is_directory: metadata.is_dir(),
                    size_bytes: metadata.len(),
                    modified_at: Some(Utc::now()),
                    permissions: String::new(),
                });
            }
        }

        Ok(entries)
    }

    async fn create_snapshot(
        &self,
        _id: &SandboxId,
        _name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "snapshots not supported for local provider".into(),
        ))
    }

    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "snapshots not supported for local provider".into(),
        ))
    }

    async fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        Ok(self.sandboxes.read().await.values().cloned().collect())
    }

    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        self.sandboxes
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))
    }

    async fn set_timeout(&self, _id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        // No-op for local provider
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn create_test_provider() -> Result<LocalProvider, DomainError> {
        // Ensure the env var is set before creating provider
        // Some tests may have removed it, so we need to set it again
        // SAFETY: This is intentional for testing purposes
        unsafe {
            env::remove_var("DANGEROUS_ALLOW_LOCAL");
            env::set_var("DANGEROUS_ALLOW_LOCAL", "1");
        }
        let temp_dir = std::env::temp_dir().join("bastion-local-test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        LocalProvider::new(temp_dir)
    }

    #[tokio::test]
    #[ignore] // Requires sequential test execution due to global env var state
    async fn test_local_provider_requires_env_var() {
        // Save original env var state
        let had_var = env::var("DANGEROUS_ALLOW_LOCAL").is_ok();
        let original_value = had_var.then(|| env::var("DANGEROUS_ALLOW_LOCAL").unwrap());

        // Ensure the env var is NOT set
        // SAFETY: This is intentional for testing purposes
        unsafe { env::remove_var("DANGEROUS_ALLOW_LOCAL") };

        // Verify LocalProvider::new fails without the env var
        let result = LocalProvider::new(std::env::temp_dir().join("test"));
        assert!(
            result.is_err(),
            "Expected LocalProvider::new to fail without DANGEROUS_ALLOW_LOCAL"
        );
        if let Err(DomainError::PermissionDenied(msg)) = result {
            assert!(msg.contains("DANGEROUS_ALLOW_LOCAL"));
        } else {
            panic!("Expected PermissionDenied error, got: {:?}", result);
        }

        // Restore original state
        // SAFETY: This is intentional for testing purposes
        unsafe {
            if had_var {
                env::set_var("DANGEROUS_ALLOW_LOCAL", original_value.unwrap());
            }
        }
    }

    #[tokio::test]
    async fn test_create_and_terminate_sandbox() {
        let provider = create_test_provider().unwrap();
        let sandbox_id = SandboxId::generate();

        // Create sandbox
        let sandbox = provider
            .create(
                &sandbox_id,
                "test-template",
                &ResourcesSpec::default(),
                &NetworkSpec::default(),
                &HashMap::new(),
                60000,
            )
            .await
            .unwrap();

        assert_eq!(sandbox.id, sandbox_id);

        // Check is_alive
        assert!(provider.is_alive(&sandbox_id).await.unwrap());

        // Terminate
        provider.terminate(&sandbox_id).await.unwrap();

        // Should no longer be alive
        assert!(!provider.is_alive(&sandbox_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_write_and_read_file() {
        let provider = create_test_provider().unwrap();
        let sandbox_id = SandboxId::generate();

        provider
            .create(
                &sandbox_id,
                "test-template",
                &ResourcesSpec::default(),
                &NetworkSpec::default(),
                &HashMap::new(),
                60000,
            )
            .await
            .unwrap();

        // Write file
        provider
            .write_file(&sandbox_id, "/test.txt", b"Hello, World!")
            .await
            .unwrap();

        // Read file
        let content = provider.read_file(&sandbox_id, "/test.txt").await.unwrap();
        assert_eq!(content, b"Hello, World!");

        provider.terminate(&sandbox_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_command() {
        let provider = create_test_provider().unwrap();
        let sandbox_id = SandboxId::generate();

        provider
            .create(
                &sandbox_id,
                "test-template",
                &ResourcesSpec::default(),
                &NetworkSpec::default(),
                &HashMap::new(),
                60000,
            )
            .await
            .unwrap();

        let result = provider
            .run_command(
                &sandbox_id,
                &CommandSpec::new("echo").with_args(vec!["Hello".to_string()]),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(String::from_utf8_lossy(&result.stdout).contains("Hello"));

        provider.terminate(&sandbox_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_list_sandboxes() {
        let provider = create_test_provider().unwrap();

        let sandbox1 = SandboxId::generate();
        let sandbox2 = SandboxId::generate();

        provider
            .create(
                &sandbox1,
                "template1",
                &ResourcesSpec::default(),
                &NetworkSpec::default(),
                &HashMap::new(),
                60000,
            )
            .await
            .unwrap();

        provider
            .create(
                &sandbox2,
                "template2",
                &ResourcesSpec::default(),
                &NetworkSpec::default(),
                &HashMap::new(),
                60000,
            )
            .await
            .unwrap();

        let sandboxes = provider
            .list_sandboxes(&SandboxFilter::default())
            .await
            .unwrap();

        assert_eq!(sandboxes.len(), 2);

        provider.terminate(&sandbox1).await.unwrap();
        provider.terminate(&sandbox2).await.unwrap();
    }
}
