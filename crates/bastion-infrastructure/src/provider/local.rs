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
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::executor::TaskExecutor;
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::state_machine::SandboxStateMachine;
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
    /// State machine for sandbox lifecycle (when use-segregated-traits is enabled)
    state_machine: Arc<SandboxStateMachine>,
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
            state_machine: Arc::new(SandboxStateMachine::new()),
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
#[async_trait]
impl SandboxLifecycle for LocalProvider {
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

        // Register with state machine when feature is enabled
        {
            self.state_machine.register(id.clone())?;
            self.state_machine.transition(
                id,
                bastion_domain::sandbox::value_objects::SandboxStatus::Running,
            )?;
        }

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

        // Remove from state machine when feature is enabled
        {
            self.state_machine.remove(id);
        }

        tracing::info!(sandbox_id = %id, "LocalProvider: terminated sandbox");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        {
            if let Some(status) = self.state_machine.get_state(id) {
                return Ok(status == bastion_domain::sandbox::value_objects::SandboxStatus::Running);
            }
        }
        Ok(self.sandboxes.read().await.contains_key(id))
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
        {
            let active_ids = self.state_machine.list_active();
            let mut result = Vec::new();
            for id in active_ids {
                if let Ok(sandbox) = self.get_info(&id).await {
                    result.push(sandbox);
                }
            }
            return Ok(result);
        }
    }

    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        {
            if self.state_machine.get_state(id).is_none() {
                return Err(DomainError::NotFound(format!("Sandbox {} not found", id)));
            }
        }
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

#[async_trait]
impl TaskExecutor for LocalProvider {
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

        // Wrap in sh -c for shell interpretation (same as podman provider)
        let shell_cmd = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!("{} {}", command.command, command.args.join(" "))
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c");
        cmd.arg(&shell_cmd);
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
}
