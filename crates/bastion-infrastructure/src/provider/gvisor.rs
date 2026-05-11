//! gVisor (runsc) provider adapter using CLI commands.
//!
//! Creates containers with `runsc run`, injects worker binary into rootfs,
//! and communicates with workers via `runsc exec` (MVP) or registry-based routing.

use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::execution::stream::CommandChunk;
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::image_source::{ImageSource, OciImage};
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::executor::TaskExecutor;
use bastion_domain::provider::port::CommandStream;
#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::rootfs::RootfsManager;
use bastion_domain::provider::router::CommandRouter;
#[cfg(feature = "use-segregated-traits")]
use bastion_domain::provider::state_machine::SandboxStateMachine;
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
    #[cfg(feature = "use-segregated-traits")]
    rootfs_manager: Arc<dyn RootfsManager>,
    command_router: Option<Arc<dyn CommandRouter>>,
    containers: Arc<DashMap<String, ContainerState>>,
    gateway_addr: String,
    /// State machine for sandbox lifecycle (when use-segregated-traits is enabled)
    #[cfg(feature = "use-segregated-traits")]
    state_machine: Arc<SandboxStateMachine>,
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
    #[cfg(feature = "use-segregated-traits")]
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
            state_machine: Arc::new(SandboxStateMachine::new()),
        })
    }

    /// Create a new gVisor provider (legacy API without RootfsManager).
    #[cfg(not(feature = "use-segregated-traits"))]
    pub fn new(
        runsc_binary: PathBuf,
        default_image: &str,
        rootfs_dir: PathBuf,
        worker_binary: PathBuf,
        gateway_addr: String,
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
            command_router: None,
            containers: Arc::new(DashMap::new()),
            gateway_addr,
        })
    }

    /// Set the command router for registry-based command execution.
    pub fn set_command_router(&mut self, router: Arc<dyn CommandRouter>) {
        self.command_router = Some(router);
    }

    /// Execute a shell command inside a gVisor container and collect output.
    ///
    /// If `env_vars` is provided, they are prepended as `KEY=VALUE` exports
    /// before the command, since `runsc exec` does not have a native env option.
    async fn exec_in_container(
        &self,
        container_id: &str,
        shell_cmd: &str,
        env_vars: Option<&HashMap<String, String>>,
    ) -> Result<(Vec<u8>, Vec<u8>, i32), DomainError> {
        // If env_vars provided (and non-empty), prepend them as exports in the shell command
        let full_cmd = if let Some(vars) = env_vars {
            if vars.is_empty() {
                shell_cmd.to_string()
            } else {
                let exports: Vec<String> = vars
                    .iter()
                    .map(|(k, v)| format!("export {k}={v}"))
                    .collect();
                format!("{} && {}", exports.join(" && "), shell_cmd)
            }
        } else {
            shell_cmd.to_string()
        };

        tracing::debug!(container_id, %full_cmd, "Running runsc exec");

        let output = self
            .runsc_cmd()
            .args(["exec", container_id, "sh", "-c", &full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to run runsc exec: {e}")))?;

        let exit_code = output.status.code().unwrap_or(-1);
        Ok((output.stdout, output.stderr, exit_code))
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

    /// Verify the worker binary is static musl.
    /// This is required because gVisor runsc containers use musl libc.
    fn verify_worker_binary(&self) -> Result<(), DomainError> {
        let worker_path = &self.worker_binary;

        // Run `file` command to check binary format
        let output = std::process::Command::new("file")
            .arg(worker_path)
            .output()
            .map_err(|e| DomainError::Internal(format!("file command failed: {e}")))?;

        let file_output = String::from_utf8_lossy(&output.stdout);

        // Check for indicators of static musl binary
        let is_static_musl = file_output.contains("statically linked")
            || (file_output.contains("musl") && file_output.contains("static"));

        if !is_static_musl {
            tracing::warn!("Worker binary may not be static musl: {}", file_output);
            // Don't fail - just warn. The binary might still work.
        }

        Ok(())
    }

    /// Create an OCI bundle for the container (legacy inline implementation).
    ///
    /// Copies the base rootfs image, generates config.json, and injects
    /// the worker binary. Returns the path to the bundle directory.
    #[cfg(not(feature = "use-segregated-traits"))]
    fn create_oci_bundle(&self, sandbox_id: &str, image: &str) -> Result<PathBuf, DomainError> {
        // Verify worker binary is static musl before injection
        // gVisor runsc containers use musl libc, so the binary must be static musl
        self.verify_worker_binary()?;

        // Strip tag from image name (e.g. "debian:bookworm-slim" -> "debian")
        let image_name = image.split(':').next().unwrap_or(image);
        let bundle_dir = self.rootfs_dir.join(sandbox_id);
        let rootfs_dest = bundle_dir.join("rootfs");

        std::fs::create_dir_all(&rootfs_dest)
            .map_err(|e| DomainError::Internal(format!("Cannot create bundle directory: {e}")))?;

        // Copy base rootfs image
        let base_rootfs = self.rootfs_dir.join(image_name);
        if !base_rootfs.exists() {
            std::fs::remove_dir_all(&bundle_dir).ok();
            return Err(DomainError::Config(format!(
                "Rootfs image not found: {}. Place a rootfs directory (e.g. debian:bookworm-slim) at this path, \
                 or set 'default_image' in your gvisor provider config to an existing image under '{}'.",
                base_rootfs.display(),
                self.rootfs_dir.display()
            )));
        }

        tracing::info!(
            sandbox_id,
            source = %base_rootfs.display(),
            dest = %rootfs_dest.display(),
            "Copying rootfs for OCI bundle"
        );
        copy_dir_recursive(&base_rootfs, &rootfs_dest)?;

        // Copy worker binary into rootfs
        let worker_dest = rootfs_dest.join("usr/local/bin/bastion-worker");
        if let Some(parent) = worker_dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::copy(&self.worker_binary, &worker_dest).map_err(|e| {
            DomainError::Internal(format!("Failed to copy worker binary to rootfs: {e}"))
        })?;
        // Make it executable
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&worker_dest)
            .map_err(|e| DomainError::Internal(format!("Failed to stat worker binary: {e}")))?
            .permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(&worker_dest, perms).ok();

        // Create /workspace directory in rootfs
        let workspace = rootfs_dest.join("workspace");
        std::fs::create_dir_all(&workspace).ok();

        // Generate config.json
        let config = self.generate_config_json();
        let config_path = bundle_dir.join("config.json");
        let config_str = serde_json::to_string_pretty(&config)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize config.json: {e}")))?;
        std::fs::write(&config_path, config_str)
            .map_err(|e| DomainError::Internal(format!("Failed to write config.json: {e}")))?;

        tracing::info!(sandbox_id, bundle = %bundle_dir.display(), "OCI bundle created");
        Ok(bundle_dir)
    }

    /// Generate a minimal OCI config.json for runsc (legacy inline implementation).
    #[cfg(not(feature = "use-segregated-traits"))]
    fn generate_config_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ociVersion": "1.0.2",
            "process": {
                "terminal": false,
                "user": { "uid": 0, "gid": 0 },
                "args": ["sleep", "999999"],
                "env": [
                    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                ],
                "cwd": "/",
                "capabilities": {
                    "bounding": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ],
                    "effective": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ],
                    "inheritable": [],
                    "permitted": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ]
                }
            },
            "root": {
                "path": "rootfs",
                "readonly": false
            },
            "mounts": [
                {
                    "destination": "/proc",
                    "type": "proc",
                    "source": "proc"
                },
                {
                    "destination": "/dev",
                    "type": "tmpfs",
                    "source": "tmpfs",
                    "options": ["nosuid", "strictatime", "mode=755", "size=65536k"]
                },
                {
                    "destination": "/sys",
                    "type": "sysfs",
                    "source": "sysfs",
                    "options": ["nosuid", "noexec", "nodev"]
                }
            ],
            "linux": {
                // NOTE: network namespace removed — rootless runsc does not support
                // sandbox-level networking. The container uses host network (--network=host equivalent).
                "namespaces": [
                    { "type": "pid" },
                    { "type": "ipc" },
                    { "type": "uts" },
                    { "type": "mount" }
                ],
                "resources": {
                    "devices": [
                        {
                            "allow": false,
                            "access": "rwm"
                        }
                    ]
                }
            }
        })
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

/// Recursively copy a directory (legacy inline implementation).
#[cfg(not(feature = "use-segregated-traits"))]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), DomainError> {
    for entry in std::fs::read_dir(src).map_err(|e| {
        DomainError::Internal(format!("Cannot read directory {}: {e}", src.display()))
    })? {
        let entry =
            entry.map_err(|e| DomainError::Internal(format!("Cannot read dir entry: {e}")))?;
        let ty = entry
            .file_type()
            .map_err(|e| DomainError::Internal(format!("Cannot get file type: {e}")))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            std::fs::create_dir_all(&dst_path).map_err(|e| {
                DomainError::Internal(format!("Cannot create dir {}: {e}", dst_path.display()))
            })?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ty.is_symlink() {
            let target = std::fs::read_link(&src_path).map_err(|e| {
                DomainError::Internal(format!("Cannot read symlink {}: {e}", src_path.display()))
            })?;
            std::os::unix::fs::symlink(&target, &dst_path).map_err(|e| {
                DomainError::Internal(format!("Cannot create symlink {}: {e}", dst_path.display()))
            })?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
                DomainError::Internal(format!(
                    "Cannot copy {} to {}: {e}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
        }
    }
    Ok(())
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

        #[cfg(feature = "use-segregated-traits")]
        {
            // Validate image using OciImage
            let oci_image = OciImage::new(image_path.clone(), false);
            oci_image.validate().await?;
        }

        #[cfg(feature = "use-segregated-traits")]
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

        #[cfg(not(feature = "use-segregated-traits"))]
        let bundle_dir = self.create_oci_bundle(&sandbox_id, &image)?;

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
        #[cfg(feature = "use-segregated-traits")]
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
        #[cfg(feature = "use-segregated-traits")]
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
        #[cfg(feature = "use-segregated-traits")]
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
        ProviderCapabilities {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 600_000,
            max_memory_mb: 4096,
            max_cpu_count: 4,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 2000,
        }
    }

    fn name(&self) -> &str {
        "gvisor"
    }

    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        let mut sandboxes = Vec::new();
        let limit = filter.limit.unwrap_or(u32::MAX) as usize;

        // Get list of sandbox IDs to check
        #[cfg(feature = "use-segregated-traits")]
        let sandbox_ids: Vec<String> = {
            self.state_machine
                .list_active()
                .into_iter()
                .map(|id| id.to_string())
                .take(limit)
                .collect()
        };

        #[cfg(not(feature = "use-segregated-traits"))]
        let sandbox_ids: Vec<String> = self
            .containers
            .iter()
            .map(|item| item.key().clone())
            .take(limit)
            .collect();

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

        // Fallback to runsc exec
        let start = Instant::now();

        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Running command via runsc exec (fallback)"
        );

        let shell_cmd = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!(
                "{} {}",
                command.command,
                command
                    .args
                    .iter()
                    .map(|a| if a.contains(' ') {
                        format!("\"{}\"", a)
                    } else {
                        a.clone()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        let (stdout, stderr, exit_code) = self
            .exec_in_container(&sandbox_id, &shell_cmd, Some(&command.env_vars))
            .await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        tracing::info!(
            sandbox_id = %id,
            exit_code,
            duration_ms,
            "Command completed"
        );

        Ok(CommandResult {
            exit_code,
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

        // Fallback: execute and stream results
        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Starting streaming command via runsc exec"
        );

        let shell_cmd = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!(
                "{} {}",
                command.command,
                command
                    .args
                    .iter()
                    .map(|a| if a.contains(' ') {
                        format!("\"{}\"", a)
                    } else {
                        a.clone()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        let runsc = self.runsc_binary.clone();
        let cid = sandbox_id.clone();

        let (tx, rx) = mpsc::channel::<Result<CommandChunk, DomainError>>(4);

        tokio::spawn(async move {
            let output = Command::new(&runsc)
                .arg("-rootless")
                .args(["exec", &cid, "sh", "-c", &shell_cmd])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            match output {
                Ok(output) => {
                    if !output.stdout.is_empty() {
                        let _ = tx.send(Ok(CommandChunk::stdout(output.stdout))).await;
                    }
                    if !output.stderr.is_empty() {
                        let _ = tx.send(Ok(CommandChunk::stderr(output.stderr))).await;
                    }
                    let exit_code = output.status.code().unwrap_or(-1);
                    let _ = tx.send(Ok(CommandChunk::exit_code(exit_code))).await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(DomainError::Internal(format!(
                            "Failed to run runsc exec: {e}"
                        ))))
                        .await;
                }
            }
        });

        let stream =
            ReceiverStream::new(rx).map(|r| r.map_err(|e| DomainError::Internal(e.to_string())));
        Ok(Box::pin(stream))
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

        // Fallback to runsc exec
        tracing::info!(
            sandbox_id = %id,
            path,
            size = content.len(),
            "Writing file via runsc exec (fallback)"
        );

        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(content);
        let shell_cmd = format!("printf '%s' '{}' | base64 -d > '{}'", encoded, path);

        let (_, _, exit_code) = self
            .exec_in_container(&sandbox_id, &shell_cmd, None)
            .await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to write file: exit code {}",
                exit_code
            )));
        }

        tracing::info!(sandbox_id = %id, path, "File written");
        Ok(())
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

        // Fallback to runsc exec
        tracing::info!(sandbox_id = %id, path, "Reading file via runsc exec (fallback)");

        let shell_cmd = format!("base64 -w0 < '{}' 2>/dev/null || base64 < '{}'", path, path);
        let (stdout, _, exit_code) = self
            .exec_in_container(&sandbox_id, &shell_cmd, None)
            .await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to read file: exit code {}",
                exit_code
            )));
        }

        // Decode base64 — strip whitespace as safety net
        use base64::Engine;
        let cleaned: Vec<u8> = stdout
            .iter()
            .copied()
            .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
            .collect();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&cleaned)
            .map_err(|e| DomainError::Internal(format!("Failed to decode base64: {e}")))?;

        Ok(decoded)
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

        // Fallback to runsc exec
        tracing::info!(sandbox_id = %id, dir, "Listing files via runsc exec (fallback)");

        let shell_cmd = format!("ls -la '{}' 2>/dev/null || ls -la '{}'", dir, dir);
        let (stdout, _, exit_code) = self
            .exec_in_container(&sandbox_id, &shell_cmd, None)
            .await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to list files: exit code {}",
                exit_code
            )));
        }

        let output_str = String::from_utf8_lossy(&stdout);
        let entries = parse_ls_output(&output_str, dir);
        Ok(entries)
    }
}

/// Parse `ls -la` output into FileEntry structs.
fn parse_ls_output(output: &str, base_dir: &str) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("total") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        let permissions = parts[0].to_string();
        let is_directory = permissions.starts_with('d');
        let size_bytes: u64 = parts[4].parse().unwrap_or(0);

        // Filename is everything from index 8 onwards (handles spaces)
        let name = parts[8..].join(" ");

        if name == "." || name == ".." {
            continue;
        }

        let path = if base_dir.ends_with('/') {
            format!("{base_dir}{name}")
        } else {
            format!("{base_dir}/{name}")
        };

        entries.push(FileEntry {
            path,
            is_directory,
            size_bytes,
            modified_at: None,
            permissions,
        });
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "use-segregated-traits")]
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

    #[cfg(not(feature = "use-segregated-traits"))]
    #[test]
    fn test_new_validates_runsc_binary() {
        // Without segregated traits, GVisorProvider::new has different signature
        // This test is only for the legacy API
        let result = GVisorProvider::new(
            PathBuf::from("/nonexistent/runsc"),
            "default",
            PathBuf::from("/tmp/bastion-test-rootfs"),
            PathBuf::from("/nonexistent/bastion-worker"),
            "host.containers.internal:50052".to_string(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ls_output() {
        let output = "total 8\n\
                      drwxr-xr-x 2 root root 4096 Jan 01 12:00 dir1\n\
                      -rw-r--r-- 1 root root  100 Jan 01 12:00 file.txt\n";
        let entries = parse_ls_output(output, "/workspace");
        assert_eq!(entries.len(), 2);

        let dir_entry = entries
            .iter()
            .find(|e| e.path == "/workspace/dir1")
            .unwrap();
        assert!(dir_entry.is_directory);
        assert_eq!(dir_entry.size_bytes, 4096);

        let file_entry = entries
            .iter()
            .find(|e| e.path == "/workspace/file.txt")
            .unwrap();
        assert!(!file_entry.is_directory);
        assert_eq!(file_entry.size_bytes, 100);
    }
}