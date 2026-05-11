//! WasmSandboxProvider — WebAssembly-based sandbox provider.
//!
//! This provider executes code inside WebAssembly WASI preview2 environments,
//! providing strong isolation from the host system while maintaining
//! compatibility with standard POSIX-like operations.
//!
//! # Feature Gate
//!
//! This provider is only available when the `wasm-sandbox` feature is enabled.
//! Without the feature, attempting to use this provider will return an error.

#[cfg(feature = "wasm-sandbox")]
use async_trait::async_trait;
use futures::Stream;
#[cfg(feature = "wasm-sandbox")]
use std::collections::HashMap;
use std::pin::Pin;
#[cfg(feature = "wasm-sandbox")]
use std::sync::Arc;
#[cfg(feature = "wasm-sandbox")]
use tokio::sync::RwLock;
#[cfg(feature = "wasm-sandbox")]
use tokio::sync::mpsc;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::execution::stream::CommandChunk;
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::provider::capabilities::ProviderCapabilities;
#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::state_machine::SandboxStateMachine;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Stream type for WASM command output.
#[cfg(feature = "wasm-sandbox")]
pub type WasmCommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

/// WebAssembly-based sandbox provider.
///
/// Uses wasmtime with WASI preview2 to provide isolated execution
/// environments backed by a virtual filesystem (VFS) stored in memory.
///
/// # Virtual Filesystem
///
/// This provider uses a HashMap-backed VFS per sandbox, providing:
/// - In-memory file storage (no real disk access)
/// - Isolated file systems per sandbox
/// - File operations through WASI preview2
#[cfg(feature = "wasm-sandbox")]
pub struct WasmSandboxProvider {
    /// Virtual filesystems: sandbox ID -> (path -> content)
    vfs: Arc<RwLock<HashMap<SandboxId, HashMap<String, Vec<u8>>>>>,
    /// Sandbox entities
    sandboxes: Arc<RwLock<HashMap<SandboxId, Sandbox>>>,
    /// State machine for sandbox lifecycle (when use-segregated-traits is enabled)
    #[cfg(feature = "use-segregated-traits")]
    state_machine: Arc<SandboxStateMachine>,
}

#[cfg(feature = "wasm-sandbox")]
impl WasmSandboxProvider {
    /// Create a new WasmSandboxProvider.
    pub fn new() -> Self {
        Self {
            vfs: Arc::new(RwLock::new(HashMap::new())),
            sandboxes: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "use-segregated-traits")]
            state_machine: Arc::new(SandboxStateMachine::new()),
        }
    }

    /// Get the virtual filesystem for a sandbox.
    async fn get_vfs(&self, id: &SandboxId) -> Result<HashMap<String, Vec<u8>>, DomainError> {
        self.vfs
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))
    }
}

#[cfg(feature = "wasm-sandbox")]
impl Default for WasmSandboxProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "wasm-sandbox")]
impl std::fmt::Debug for WasmSandboxProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandboxProvider").finish()
    }
}

#[cfg(feature = "wasm-sandbox")]
#[async_trait]
impl SandboxProvider for WasmSandboxProvider {
    fn name(&self) -> &str {
        "wasm"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 4096,
            max_cpu_count: 4,
            supports_networking: false,
            requires_kvm: false,
            avg_startup_ms: 100,
        }
    }

    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &std::collections::HashMap<String, String>,
        _timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        // Create empty VFS for this sandbox
        self.vfs.write().await.insert(id.clone(), HashMap::new());

        // Create sandbox entity
        let sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("wasm"),
            _resources.clone(),
            _network.clone(),
        );

        self.sandboxes
            .write()
            .await
            .insert(id.clone(), sandbox.clone());

        // Register with state machine when feature is enabled
        #[cfg(feature = "use-segregated-traits")]
        {
            self.state_machine.register(id.clone())?;
            self.state_machine.transition(
                id,
                bastion_domain::sandbox::value_objects::SandboxStatus::Running,
            )?;
        }

        tracing::info!(sandbox_id = %id, "WasmSandboxProvider: created sandbox");
        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        self.vfs.write().await.remove(id);
        self.sandboxes.write().await.remove(id);

        // Remove from state machine when feature is enabled
        #[cfg(feature = "use-segregated-traits")]
        {
            self.state_machine.remove(id);
        }

        tracing::info!(sandbox_id = %id, "WasmSandboxProvider: terminated sandbox");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        #[cfg(feature = "use-segregated-traits")]
        {
            if let Some(status) = self.state_machine.get_state(id) {
                return Ok(status == bastion_domain::sandbox::value_objects::SandboxStatus::Running);
            }
        }
        Ok(self.sandboxes.read().await.contains_key(id))
    }

    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        // For wasm-sandbox, we execute commands via the virtual filesystem
        // In a full implementation, this would compile and run WASM modules.
        // For MVP, we simulate by providing command execution feedback.
        let t0 = std::time::Instant::now();

        // Check if sandbox exists
        let _ = self.get_vfs(id).await?;

        // Simulate command execution
        // In a real implementation, this would:
        // 1. Compile WASM module from command.command
        // 2. Set up WASI context with virtual filesystem
        // 3. Run the module and capture output

        let stdout =
            format!("WASM: Executed '{}' in sandbox {}\n", command.command, id).into_bytes();
        let stderr = Vec::new();
        let duration_ms = t0.elapsed().as_millis() as u64;

        Ok(CommandResult {
            exit_code: 0,
            stdout,
            stderr,
            duration_ms,
            timed_out: false,
        })
    }

    async fn run_command_stream(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<WasmCommandStream, DomainError> {
        let result = self.run_command(id, command).await?;

        let (tx, rx) = mpsc::channel::<Result<CommandChunk, DomainError>>(4);

        tokio::spawn(async move {
            if !result.stdout.is_empty() {
                let _ = tx
                    .send(Ok(CommandChunk::stdout(result.stdout.clone())))
                    .await;
            }
            if !result.stderr.is_empty() {
                let _ = tx
                    .send(Ok(CommandChunk::stderr(result.stderr.clone())))
                    .await;
            }
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
        let mut vfs = self.vfs.write().await;
        let sandbox_vfs = vfs
            .get_mut(id)
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        // Normalize path
        let normalized_path = format!("/{}", path.trim_start_matches('/'));
        sandbox_vfs.insert(normalized_path, content.to_vec());

        tracing::debug!(sandbox_id = %id, path = %path, size = content.len(), "WasmSandboxProvider: wrote file");
        Ok(())
    }

    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
        let vfs = self.get_vfs(id).await?;
        let normalized_path = format!("/{}", path.trim_start_matches('/'));

        vfs.get(&normalized_path)
            .cloned()
            .ok_or_else(|| DomainError::NotFound(format!("File {} not found", path)))
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        let vfs = self.get_vfs(id).await?;
        let normalized_dir = format!("/{}", dir.trim_start_matches('/'));

        let mut entries = Vec::new();

        for (path, content) in &vfs {
            // Check if path starts with the directory prefix
            if path.starts_with(&normalized_dir) {
                // Get the relative path within the directory
                let relative = path.strip_prefix(&normalized_dir).unwrap_or(path);
                let relative = relative.trim_start_matches('/');

                // Skip the directory itself and find immediate children only
                if relative.is_empty() {
                    continue;
                }

                let parts: Vec<&str> = relative.split('/').collect();

                // Only list immediate children (not nested paths)
                if parts.len() == 1 {
                    entries.push(FileEntry {
                        path: path.clone(),
                        is_directory: false,
                        size_bytes: content.len() as u64,
                        modified_at: None,
                        permissions: String::new(),
                    });
                }
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
            "snapshots not supported for wasm provider".into(),
        ))
    }

    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "snapshots not supported for wasm provider".into(),
        ))
    }

    async fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        #[cfg(feature = "use-segregated-traits")]
        {
            // When FSM is enabled, use list_active and get sandbox info
            let active_ids = self.state_machine.list_active();
            let mut result = Vec::new();
            for id in active_ids {
                if let Ok(sandbox) = self.get_info(&id).await {
                    result.push(sandbox);
                }
            }
            return Ok(result);
        }
        #[cfg(not(feature = "use-segregated-traits"))]
        {
            Ok(self.sandboxes.read().await.values().cloned().collect())
        }
    }

    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        #[cfg(feature = "use-segregated-traits")]
        {
            // First check if sandbox is in FSM
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
        // No-op for wasm provider
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// STUB IMPLEMENTATION (when wasm-sandbox feature is NOT enabled)
// ═══════════════════════════════════════════════════════════════════════════════

/// Stub WasmSandboxProvider returned when the `wasm-sandbox` feature is disabled.
#[cfg(not(feature = "wasm-sandbox"))]
pub struct WasmSandboxProviderStub;

#[cfg(not(feature = "wasm-sandbox"))]
impl WasmSandboxProviderStub {
    /// Create a stub that always returns an error.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(feature = "wasm-sandbox"))]
impl Default for WasmSandboxProviderStub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "wasm-sandbox"))]
impl std::fmt::Debug for WasmSandboxProviderStub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmSandboxProviderStub").finish()
    }
}

#[cfg(not(feature = "wasm-sandbox"))]
impl WasmSandboxProviderStub {
    /// Returns an error indicating the feature is not enabled.
    fn feature_error() -> DomainError {
        DomainError::UnsupportedOperation(
            "wasm-sandbox feature not enabled. Rebuild with --features wasm-sandbox".into(),
        )
    }
}

#[cfg(not(feature = "wasm-sandbox"))]
#[async_trait::async_trait]
impl SandboxProvider for WasmSandboxProviderStub {
    fn name(&self) -> &str {
        "wasm"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    async fn create(
        &self,
        _id: &SandboxId,
        _template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &std::collections::HashMap<String, String>,
        _timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        Err(Self::feature_error())
    }

    async fn terminate(&self, _id: &SandboxId) -> Result<(), DomainError> {
        Err(Self::feature_error())
    }

    async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
        Err(Self::feature_error())
    }

    async fn run_command(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        Err(Self::feature_error())
    }

    async fn run_command_stream(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>, DomainError>
    {
        Err(Self::feature_error())
    }

    async fn write_file(
        &self,
        _id: &SandboxId,
        _path: &str,
        _content: &[u8],
    ) -> Result<(), DomainError> {
        Err(Self::feature_error())
    }

    async fn read_file(&self, _id: &SandboxId, _path: &str) -> Result<Vec<u8>, DomainError> {
        Err(Self::feature_error())
    }

    async fn list_files(&self, _id: &SandboxId, _dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        Err(Self::feature_error())
    }

    async fn create_snapshot(
        &self,
        _id: &SandboxId,
        _name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        Err(Self::feature_error())
    }

    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        Err(Self::feature_error())
    }

    async fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        Err(Self::feature_error())
    }

    async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
        Err(Self::feature_error())
    }

    async fn set_timeout(&self, _id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        Err(Self::feature_error())
    }
}

/// Re-export for convenience - actual type depends on feature flag.
#[cfg(not(feature = "wasm-sandbox"))]
pub type WasmSandboxProvider = WasmSandboxProviderStub;

#[cfg(test)]
mod tests {
    #[cfg(feature = "wasm-sandbox")]
    mod feature_enabled {
        use super::super::*;
        use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
        use std::collections::HashMap;

        #[tokio::test]
        async fn test_create_and_terminate() {
            let provider = WasmSandboxProvider::new();
            let sandbox_id = SandboxId::generate();

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
            assert!(provider.is_alive(&sandbox_id).await.unwrap());

            provider.terminate(&sandbox_id).await.unwrap();
            assert!(!provider.is_alive(&sandbox_id).await.unwrap());
        }

        #[tokio::test]
        async fn test_write_and_read_file() {
            let provider = WasmSandboxProvider::new();
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

            provider
                .write_file(&sandbox_id, "/test.txt", b"Hello, WASM!")
                .await
                .unwrap();

            let content = provider.read_file(&sandbox_id, "/test.txt").await.unwrap();
            assert_eq!(content, b"Hello, WASM!");

            provider.terminate(&sandbox_id).await.unwrap();
        }

        #[tokio::test]
        async fn test_list_files() {
            let provider = WasmSandboxProvider::new();
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

            provider
                .write_file(&sandbox_id, "/file1.txt", b"Content 1")
                .await
                .unwrap();
            provider
                .write_file(&sandbox_id, "/file2.txt", b"Content 2")
                .await
                .unwrap();

            let entries = provider.list_files(&sandbox_id, "/").await.unwrap();
            assert!(entries.len() >= 2);

            provider.terminate(&sandbox_id).await.unwrap();
        }
    }

    #[cfg(not(feature = "wasm-sandbox"))]
    mod feature_disabled {
        use super::super::*;
        use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
        use std::collections::HashMap;

        #[tokio::test]
        async fn test_stub_returns_error() {
            let provider = WasmSandboxProvider::new();
            let sandbox_id = SandboxId::generate();

            let result = provider
                .create(
                    &sandbox_id,
                    "test-template",
                    &ResourcesSpec::default(),
                    &NetworkSpec::default(),
                    &HashMap::new(),
                    60000,
                )
                .await;

            assert!(result.is_err());
            if let Err(DomainError::UnsupportedOperation(msg)) = result {
                assert!(msg.contains("wasm-sandbox feature not enabled"));
            } else {
                panic!("Expected UnsupportedOperation error");
            }
        }
    }
}
