//! gVisor (runsc) provider adapter using CLI commands.
//!
//! Creates containers with `runsc run`, injects worker binary into rootfs,
//! and communicates with workers via `runsc exec` (MVP) or registry-based routing.

use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::process::Command;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::executor::TaskExecutor;
use bastion_domain::provider::image_source::{ImageSource, OciImage};
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::port::CommandStream;
use bastion_domain::provider::rootfs::RootfsManager;
use bastion_domain::provider::router::CommandRouter;
use super::state_machine::DashMapSandboxStateMachine;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{
    NetworkSpec, ResourcesSpec, SandboxFilter, SandboxStatus,
};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// State tracked for each gVisor container.
struct ContainerState {
    /// The `runsc run` child process handle (owns the container lifecycle)
    child: tokio::process::Child,
    /// Path to the OCI bundle directory
    bundle_dir: PathBuf,
}

/// gVisor-based sandbox provider using runsc CLI.
///
/// Spawns a `runsc run` process per sandbox, communicating via `runsc exec`.
/// Each sandbox gets its own OCI bundle (rootfs + config.json) with the
/// bastion-worker binary injected.
pub struct GVisorProvider {
    runsc_binary: PathBuf,
    default_image: String,
    rootfs_dir: PathBuf,
    worker_binary: PathBuf,
    rootfs_manager: Arc<dyn RootfsManager>,
    command_router: Option<Arc<dyn CommandRouter>>,
    containers: Arc<DashMap<String, ContainerState>>,
    gateway_addr: String,
    /// State machine for sandbox lifecycle (when use-segregated-traits is enabled)
    state_machine: Arc<DashMapSandboxStateMachine>,
}

impl std::fmt::Debug for GVisorProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GVisorProvider")
            .field("runsc_binary", &self.runsc_binary)
            .field("default_image", &self.default_image)
            .field("rootfs_dir", &self.rootfs_dir)
            .field("worker_binary", &self.worker_binary)
            .field("rootfs_manager", &"...")
            .field("gateway_addr", &self.gateway_addr)
            .field("command_router", &self.command_router.is_some())
            .finish()
    }
}

impl GVisorProvider {
    /// Create a new gVisor provider.
    ///
    /// * `runsc_binary` — path to the `runsc` binary
    /// * `default_image` — name of the default rootfs image directory under `rootfs_dir`
    /// * `rootfs_dir` — directory containing OCI root filesystem images
    /// * `worker_binary` — path to the bastion-worker MUSL static binary
    /// * `gateway_addr` — host gateway address for worker connections
    pub fn new(
        runsc_binary: PathBuf,
        default_image: &str,
        rootfs_dir: PathBuf,
        worker_binary: PathBuf,
        gateway_addr: String,
        rootfs_manager: impl RootfsManager + 'static,
    ) -> Result<Self, DomainError> {
        if !runsc_binary.exists() {
            return Err(DomainError::Config(format!(
                "runsc binary not found: {}",
                runsc_binary.display()
            )));
        }
        if !rootfs_dir.exists() {
            return Err(DomainError::Config(format!(
                "Rootfs directory not found: {}",
                rootfs_dir.display()
            )));
        }

        std::fs::create_dir_all(&rootfs_dir)
            .map_err(|e| DomainError::Config(format!("Cannot create rootfs directory: {e}")))?;

        Ok(Self {
            runsc_binary,
            default_image: default_image.to_string(),
            rootfs_dir,
            worker_binary,
            rootfs_manager: Arc::new(rootfs_manager),
            command_router: None,
            containers: Arc::new(DashMap::new()),
            gateway_addr,
            state_machine: Arc::new(DashMapSandboxStateMachine::new()),
        })
    }

    /// Set the command router for registry-based command execution.
    pub fn set_command_router(&mut self, router: Arc<dyn CommandRouter>) {
        self.command_router = Some(router);
    }

    /// Start the bastion-worker process in the container via exec.
    /// The worker connects OUTBOUND to the gateway (JNLP pattern).
    fn start_worker_in_container(&self, container_id: &str, sandbox_id: &str, secret: &str) {
        let runsc = self.runsc_binary.clone();
        let cid = container_id.to_string();
        let sid = sandbox_id.to_string();
        let sec = secret.to_string();
        let gateway = format!("http://{}", self.gateway_addr);

        tokio::spawn(async move {
            let result = Self::runsc_cmd_static(&runsc)
                .args([
                    "exec",
                    &cid,
                    "/usr/local/bin/bastion-worker",
                    "--gateway-addr",
                    &gateway,
                    "--sandbox-id",
                    &sid,
                    "--secret",
                    &sec,
                    "--workdir",
                    "/workspace",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;

            match result {
                Ok(status) => {
                    tracing::debug!(
                        container_id = %cid,
                        exit_code = ?status.code(),
                        "Worker process exited"
                    );
                }
                Err(e) => {
                    tracing::warn!(container_id = %cid, error = %e, "Failed to start worker");
                }
            }
        });

        tracing::debug!(container_id, "Worker process spawned");
    }

    /// Wait for the worker to connect to the gateway.
    /// Polls the command router to check if the worker has registered.
    /// If no command router is configured, skip the wait (fallback mode — worker is not used).
    async fn wait_for_worker_connection(
        &self,
        sandbox_id: &str,
        timeout: std::time::Duration,
    ) -> Result<(), DomainError> {
        // If no router is configured, we're in fallback mode — skip wait
        let Some(ref router) = self.command_router else {
            tracing::debug!(
                sandbox_id,
                "No command router configured — skipping worker connection wait"
            );
            return Ok(());
        };

        let deadline = std::time::Instant::now() + timeout;

        loop {
            if std::time::Instant::now() > deadline {
                return Err(DomainError::Timeout(format!(
                    "Worker in sandbox {} did not connect within timeout",
                    sandbox_id
                )));
            }

            if router.is_worker_connected(sandbox_id) {
                tracing::info!(sandbox_id, "Worker connected to gateway");
                return Ok(());
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Check if a container is running via runsc list.
    async fn container_is_running(&self, container_id: &str) -> Result<bool, DomainError> {
        let output = self
            .runsc_cmd()
            .args(["list"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to run runsc list: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // runsc list output: ID\tPID\tSTATUS\tBUNDLE\tCREATED\tOWNER
        Ok(stdout.lines().any(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.first().is_some_and(|id| *id == container_id)
                && parts.get(2).is_some_and(|status| *status == "running")
        }))
    }

    /// Create a `Command` with rootless runsc flags (for rootless gVisor).
    fn runsc_cmd(&self) -> Command {
        Self::runsc_cmd_static(&self.runsc_binary)
    }

    /// Static version that takes a path (for use in spawned tasks without self).
    fn runsc_cmd_static(runsc: &Path) -> Command {
        let mut cmd = Command::new(runsc);
        cmd.arg("-rootless");
        // NOTE: rootless runsc does not support sandbox networking.
        // Use host network to avoid "sandbox network isn't supported with --rootless" error.
        cmd.arg("-network=host");
        cmd
    }

    /// Return the rootfs directory (used by tests for cleanup).
    pub fn rootfs_dir(&self) -> &PathBuf {
        &self.rootfs_dir
    }
}

#[async_trait]
#[async_trait]
impl SandboxLifecycle for GVisorProvider {
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        _resources: &ResourcesSpec,
        network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        // For gVisor, map template names like "debian:bookworm-slim" to directory names.
        // Strip the tag suffix since rootfs directories don't use tags (e.g. "debian" not "debian:bookworm-slim").
        let image = if template.is_empty() {
            self.default_image.clone()
        } else {
            template.split(':').next().unwrap_or(&template).to_string()
        };
        let sandbox_id = id.to_string();
        let secret = format!("secret-{}", uuid::Uuid::new_v4());

        tracing::info!(sandbox_id = %id, image = %image, "Creating gVisor container");

        // Validate image exists
        let image_path = self.rootfs_dir.join(&image);
        if !image_path.exists() {
            return Err(DomainError::Config(format!(
                "Rootfs image not found: {}. Place a rootfs directory (e.g. debian:bookworm-slim) at this path, \
                 or set 'default_image' in your gvisor provider config to an existing image under '{}'.",
                image_path.display(),
                self.rootfs_dir.display()
            )));
        }

        {
            // Validate image using OciImage
            let oci_image = OciImage::new(image_path.clone(), false);
            oci_image.validate().await?;
        }

        let bundle_dir = self
            .rootfs_manager
            .prepare_oci_bundle(
                id,
                &self.rootfs_dir.join(&sandbox_id),
                &image_path,
                &self.worker_binary,
                _env_vars,
                &["/bin/sleep".to_string(), "infinity".to_string()],
            )
            .await?;

        // Spawn runsc run (this process owns the container's lifetime)
        // Network config is set in the OCI config.json via linux.namespaces.
        // --network flag is not supported in modern runsc (>=2025).
        let mut child = self
            .runsc_cmd()
            .args(["run", "-bundle", &bundle_dir.to_string_lossy(), &sandbox_id])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DomainError::Internal(format!("Failed to spawn runsc: {e}")))?;

        // Read stderr in background for debugging
        if let Some(stderr) = child.stderr.take() {
            let sid = sandbox_id.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut reader = tokio::io::BufReader::new(stderr);
                let mut buf = vec![0u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&buf[..n]);
                            tracing::debug!(sandbox_id = %sid, "runsc stderr: {}", text.trim());
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Store container state
        let state = ContainerState { child, bundle_dir };
        self.containers.insert(sandbox_id.clone(), state);

        // Register with state machine when feature is enabled
        {
            self.state_machine.register(id.clone())?;
            self.state_machine.transition(id, SandboxStatus::Running)?;
        }

        // Wait briefly for container to initialize
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Verify container is running
        let mut attempts = 0;
        loop {
            if self
                .container_is_running(&sandbox_id)
                .await
                .unwrap_or(false)
            {
                break;
            }
            attempts += 1;
            if attempts >= 10 {
                return Err(DomainError::Timeout(format!(
                    "Container {} did not start within timeout",
                    sandbox_id
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        tracing::info!(sandbox_id = %id, "gVisor container started successfully");

        // Start the bastion-worker process
        self.start_worker_in_container(&sandbox_id, &sandbox_id, &secret);

        // Register secret with command router
        if let Some(ref router) = self.command_router {
            router.set_sandbox_secret(&sandbox_id, &secret);
        }

        // Wait for worker to connect to gateway (with generous timeout for slow startup)
        self.wait_for_worker_connection(&sandbox_id, std::time::Duration::from_secs(30))
            .await?;

        // Build domain entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("gvisor"),
            None,
            _resources.clone(),
            network.clone(),
        );
        sandbox.set_timeout(timeout_ms);
        sandbox.mark_running()?;

        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        let sandbox_id = id.to_string();

        tracing::info!(sandbox_id = %id, "Terminating gVisor container");

        // Kill the runsc process and remove state
        let bundle_dir = if let Some((_, mut state)) = self.containers.remove(&sandbox_id) {
            let _ = state.child.kill().await;
            Some(state.bundle_dir)
        } else {
            None
        };

        // Remove from state machine when feature is enabled
        {
            self.state_machine.remove(id);
        }

        // Force-delete the container via runsc (best-effort)
        let _ = self
            .runsc_cmd()
            .args(["delete", "-force", &sandbox_id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        // Clean up the bundle directory
        if let Some(dir) = bundle_dir
            && dir.exists()
            && let Err(e) = std::fs::remove_dir_all(&dir)
        {
            tracing::warn!(sandbox_id = %id, error = %e, "Failed to clean up bundle directory");
        }

        tracing::info!(sandbox_id = %id, "gVisor container terminated");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        let sandbox_id = id.to_string();

        // Check FSM state first when feature is enabled
        {
            if let Some(status) = self.state_machine.get_state(id) {
                // If FSM says Stopped or Failed, return false immediately
                if status == SandboxStatus::Stopped || status == SandboxStatus::Failed {
                    return Ok(false);
                }
                // If Running, still verify with process check below
            }
        }

        // First check our tracked process
        match self.containers.get_mut(&sandbox_id) {
            Some(mut state) => {
                match state.child.try_wait() {
                    Ok(Some(_)) => Ok(false), // Process exited
                    Ok(None) => Ok(true),     // Still running
                    Err(_) => Ok(false),
                }
            }
            None => {
                // Not in our map, check runsc list as fallback
                self.container_is_running(&sandbox_id).await
            }
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::try_new(
            false,
            true,
            false,
            600_000,
            4096,
            4,
            true,
            false,
            2000,
        )
        .expect("known valid values")
    }

    fn name(&self) -> &str {
        "gvisor"
    }

    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        let mut sandboxes = Vec::new();
        let limit = filter.limit.unwrap_or(u32::MAX) as usize;

        // Get list of sandbox IDs to check
        let sandbox_ids: Vec<String> = {
            self.state_machine
                .list_active()
                .into_iter()
                .map(|id| id.to_string())
                .take(limit)
                .collect()
        };

        for sandbox_id in sandbox_ids {
            // Use get_mut to allow calling try_wait()
            let is_alive = if let Some(mut state) = self.containers.get_mut(&sandbox_id) {
                match state.child.try_wait() {
                    Ok(Some(_)) => false,
                    Ok(None) => true,
                    Err(_) => false,
                }
            } else {
                continue;
            };

            let status = if is_alive {
                SandboxStatus::Running
            } else {
                SandboxStatus::Stopped
            };

            // Apply status filter
            if let Some(ref filter_status) = filter.status
                && status != *filter_status
            {
                continue;
            }

            // Build a minimal Sandbox entity
            let sandbox = Sandbox::new(
                SandboxId::new(&sandbox_id),
                bastion_domain::shared::id::TemplateId::new("gvisor"),
                bastion_domain::shared::id::ProviderId::new("gvisor"),
                None,
                ResourcesSpec::default(),
                NetworkSpec::default(),
            );

            sandboxes.push(sandbox);
        }

        Ok(sandboxes)
    }

    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        let sandbox_id = id.to_string();

        // Check if we have this container tracked
        let mut container = self
            .containers
            .get_mut(&sandbox_id)
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;

        // Check if container process is still alive
        let is_alive = match container.child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        };

        let status = if is_alive {
            SandboxStatus::Running
        } else {
            SandboxStatus::Stopped
        };

        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new("gvisor"),
            bastion_domain::shared::id::ProviderId::new("gvisor"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        if status == SandboxStatus::Running {
            sandbox.mark_running()?;
        } else {
            let _ = sandbox.terminate();
        }

        Ok(sandbox)
    }

    async fn set_timeout(&self, id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        let sandbox_id = id.to_string();

        // Verify the container exists
        let _ = self
            .containers
            .get(&sandbox_id)
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;

        // gVisor containers don't have a native timeout mechanism.
        // The timeout is managed at the Bastion layer.
        // This operation is a no-op at the provider level.
        tracing::debug!(sandbox_id = %id, "set_timeout called on GVisorProvider (no-op at provider level)");
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for GVisorProvider {
    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        let sandbox_id = id.to_string();

        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&sandbox_id)
        {
            tracing::info!(sandbox_id = %id, "Routing command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command(
                    &sandbox_id,
                    &command.command,
                    &command.args,
                    command.working_dir.as_deref().unwrap_or("/workspace"),
                    &command.env_vars,
                    timeout_ms,
                )
                .await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn run_command_stream(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        let sandbox_id = id.to_string();

        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&sandbox_id)
        {
            tracing::info!(sandbox_id = %id, "Streaming command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command_stream(
                    &sandbox_id,
                    &command.command,
                    &command.args,
                    command.working_dir.as_deref().unwrap_or("/workspace"),
                    &command.env_vars,
                    timeout_ms,
                )
                .await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        let sandbox_id = id.to_string();

        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&sandbox_id)
        {
            tracing::info!(sandbox_id = %id, path, "Writing file via worker registry");
            return router.route_write_file(&sandbox_id, path, content).await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
        let sandbox_id = id.to_string();

        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&sandbox_id)
        {
            tracing::info!(sandbox_id = %id, path, "Reading file via worker registry");
            return router.route_read_file(&sandbox_id, path).await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        let sandbox_id = id.to_string();

        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&sandbox_id)
        {
            tracing::info!(sandbox_id = %id, dir, "Listing files via worker registry");
            return router.route_list_files(&sandbox_id, dir).await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_validates_runsc_binary() {
        use crate::provider::DefaultRootfsManager;
        let result = GVisorProvider::new(
            PathBuf::from("/nonexistent/runsc"),
            "default",
            PathBuf::from("/tmp/bastion-test-rootfs"),
            PathBuf::from("/nonexistent/bastion-worker"),
            "host.containers.internal:50052".to_string(),
            DefaultRootfsManager::new(),
        );
        assert!(result.is_err());
    }
}
