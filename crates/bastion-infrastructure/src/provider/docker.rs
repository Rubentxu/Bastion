//! Docker provider adapter using bollard Docker API client.
//!
//! Creates containers with `sleep infinity`, injects worker binary via bind mount,
//! and communicates with workers via exec (MVP) or future registry-based routing.
//!
//! **Binary Format**: Unlike Firecracker/gVisor (which require static musl binaries
//! because they copy the binary into a musl-based rootfs), Docker uses bind mount
//! which works with ANY binary format (glibc or musl). The binary is mounted
//! directly from the host filesystem read-only.

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::execution::stream::CommandChunk;
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::executor::TaskExecutor;
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::port::CommandStream;
use bastion_domain::provider::router::CommandRouter;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{
    NetworkSpec, ResourcesSpec, SandboxFilter, SandboxStatus,
};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Podman-based sandbox provider using bollard Docker API client.
/// Communicates with containers via bollard exec (MVP) or registry-based routing.
pub struct DockerProvider {
    docker: Docker,
    default_image: String,
    /// Path to the worker binary to inject into containers via bind mount
    worker_binary: PathBuf,
    /// Optional command router for registry-based command execution
    command_router: Option<Arc<dyn CommandRouter>>,
    /// Optional source code path to mount into containers for self-testing
    source_mount: Option<PathBuf>,
}

// Manual Debug impl because Docker doesn't derive Debug
impl std::fmt::Debug for DockerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerProvider")
            .field("default_image", &self.default_image)
            .field("worker_binary", &self.worker_binary)
            .field("command_router", &self.command_router.is_some())
            .field("source_mount", &self.source_mount)
            .finish_non_exhaustive()
    }
}

impl DockerProvider {
    /// Connect to Podman via Unix socket.
    pub fn new(
        socket_path: &str,
        default_image: &str,
        worker_binary: PathBuf,
    ) -> Result<Self, DomainError> {
        let docker = Docker::connect_with_unix(socket_path, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| DomainError::ProviderUnavailable(e.to_string()))?;

        Ok(Self {
            docker,
            default_image: default_image.to_string(),
            worker_binary,
            command_router: None,
            source_mount: None,
        })
    }

    /// Set the command router for registry-based command execution.
    /// When set, commands will be routed through the worker registry instead of exec.
    pub fn set_command_router(&mut self, router: Arc<dyn CommandRouter>) {
        self.command_router = Some(router);
    }

    /// Add a source code mount for self-testing purposes.
    /// The path will be mounted at /workspace/code in the container.
    pub fn with_source_mount(&mut self, path: PathBuf) -> &mut Self {
        self.source_mount = Some(path);
        self
    }

    /// Ping the Podman daemon to verify connectivity.
    pub async fn ping(&self) -> Result<String, DomainError> {
        self.docker
            .ping()
            .await
            .map_err(|e| DomainError::ProviderUnavailable(e.to_string()))
            .map(|pong| format!("{pong:?}"))
    }

    /// Execute a command inside a container and collect output via bollard exec.
    ///
    /// If `env_vars` is provided, the environment variables are passed to the
    /// exec session so the command runs with the correct context (PATH, JAVA_HOME, etc.).
    async fn exec_in_container(
        &self,
        container_name: &str,
        command: &str,
        env_vars: Option<&HashMap<String, String>>,
    ) -> Result<(Vec<u8>, Vec<u8>, i32), DomainError> {
        let env = env_vars.map(|vars| {
            vars.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
        });
        let exec_config = bollard::exec::CreateExecOptions {
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                command.to_string(),
            ]),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            env,
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(container_name, exec_config)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to create exec: {e}")))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        match self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to start exec: {e}")))?
        {
            StartExecResults::Attached { output, .. } => {
                let mut stream = output;
                while let Some(log_result) = stream.next().await {
                    match log_result {
                        Ok(LogOutput::StdOut { message }) => stdout.extend_from_slice(&message),
                        Ok(LogOutput::StdErr { message }) => stderr.extend_from_slice(&message),
                        Ok(LogOutput::Console { message }) => stdout.extend_from_slice(&message),
                        Err(e) => {
                            tracing::warn!("Error reading exec output: {e}");
                        }
                        _ => {}
                    }
                }
            }
            StartExecResults::Detached => {
                tracing::warn!("Exec started in detached mode, cannot collect output");
            }
        }

        // Get exit code from exec inspection
        let exec_info = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to inspect exec: {e}")))?;

        let exit_code = exec_info.exit_code.unwrap_or(-1) as i32;
        Ok((stdout, stderr, exit_code))
    }

    /// Start the bastion-worker process in the container via exec.
    /// The worker will connect OUTBOUND to the gateway (JNLP pattern).
    async fn start_worker_in_container(
        &self,
        container_name: &str,
        sandbox_id: &str,
        secret: &str,
    ) -> Result<(), DomainError> {
        let exec_config = CreateExecOptions {
            cmd: Some(vec![
                "/usr/local/bin/bastion-worker".to_string(),
                "--gateway-addr".to_string(),
                "http://host.containers.internal:50052".to_string(),
                "--sandbox-id".to_string(),
                sandbox_id.to_string(),
                "--secret".to_string(),
                secret.to_string(),
                "--workdir".to_string(),
                "/workspace".to_string(),
            ]),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(container_name, exec_config)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to create exec for worker: {e}")))?;

        // Use StartExecOptions with detach: true
        let start_opts = StartExecOptions {
            detach: true,
            ..Default::default()
        };
        self.docker
            .start_exec(&exec.id, Some(start_opts))
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to start worker: {e}")))?;

        tracing::debug!(container = %container_name, "Worker process started");
        Ok(())
    }
}

#[async_trait]
#[async_trait]
impl SandboxLifecycle for DockerProvider {
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        _resources: &ResourcesSpec,
        network: &NetworkSpec,
        env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        let image = if template.is_empty() {
            self.default_image.clone()
        } else {
            template.to_string()
        };
        let container_name = id.to_string();
        let generated_secret = format!("secret-{}", uuid::Uuid::new_v4());

        tracing::info!(sandbox_id = %id, image = %image, "Creating Podman container");

        // Build env vars as "KEY=VALUE" strings
        let env: Vec<String> = env_vars.iter().map(|(k, v)| format!("{k}={v}")).collect();

        // Create container with bind-mounted worker binary
        // NOTE: Unlike Firecracker and gVisor (which copy the binary into rootfs),
        // Podman uses bind mount which works with ANY binary format (glibc or musl).
        // The binary is mounted read-only (:ro) directly from the host.
        // Also mount source code if configured (for self-testing)
        //
        // IMPORTANT: Bind mount sources MUST be absolute paths. Relative paths like
        // "target/debug/bastion-worker" are interpreted as named volumes by Docker,
        // causing "invalid argument" errors. Canonicalize to absolute paths.
        let worker_binary_abs = self.worker_binary.canonicalize().unwrap_or_else(|_| {
            // Fallback: convert to absolute path if canonicalize fails
            if self.worker_binary.is_relative() {
                std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                    .join(&self.worker_binary)
            } else {
                self.worker_binary.clone()
            }
        });
        let mut binds = vec![format!(
            "{}:/usr/local/bin/bastion-worker:ro",
            worker_binary_abs.display()
        )];
        if let Some(ref source_path) = self.source_mount {
            let source_abs = source_path.canonicalize().unwrap_or_else(|_| {
                if source_path.is_relative() {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                        .join(source_path)
                } else {
                    source_path.clone()
                }
            });
            binds.push(format!("{}:/workspace/code:ro", source_abs.display()));
        }

        let container_config = bollard::models::ContainerCreateBody {
            image: Some(image),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            env: Some(env),
            tty: Some(false),
            attach_stdout: Some(false),
            attach_stderr: Some(false),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(binds),
                ..Default::default()
            }),
            ..Default::default()
        };

        let options = bollard::query_parameters::CreateContainerOptionsBuilder::default()
            .name(&container_name)
            .build();

        self.docker
            .create_container(Some(options), container_config)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to create container: {e}")))?;

        // Start the container
        self.docker
            .start_container(
                &container_name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to start container: {e}")))?;

        tracing::info!(sandbox_id = %id, "Container started successfully");

        // Start the bastion-worker process in the container
        self.start_worker_in_container(&container_name, id.as_str(), &generated_secret)
            .await?;

        // Build domain entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("podman"),
            _resources.clone(),
            network.clone(),
        );
        sandbox.set_timeout(timeout_ms);
        sandbox.mark_running()?;

        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, "Terminating Podman container");

        // Stop the container (best-effort — may already be stopped)
        let stop_options = bollard::query_parameters::StopContainerOptionsBuilder::default()
            .t(10)
            .build();

        if let Err(e) = self
            .docker
            .stop_container(&container_name, Some(stop_options))
            .await
        {
            tracing::warn!(sandbox_id = %id, error = %e, "Stop failed, will force-remove");
        }

        // Remove the container
        let remove_options = bollard::query_parameters::RemoveContainerOptionsBuilder::default()
            .force(true)
            .v(true)
            .build();

        self.docker
            .remove_container(&container_name, Some(remove_options))
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to remove container: {e}")))?;

        tracing::info!(sandbox_id = %id, "Container terminated and removed");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        let container_name = id.to_string();

        match self.docker.inspect_container(&container_name, None).await {
            Ok(info) => {
                let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);
                Ok(running)
            }
            Err(e) => {
                // If container not found, it's not alive
                let err_str = format!("{e}");
                if err_str.contains("404") || err_str.contains("No such container") {
                    Ok(false)
                } else {
                    Err(DomainError::Internal(format!(
                        "Failed to inspect container: {e}"
                    )))
                }
            }
        }
    }

    async fn create_snapshot(
        &self,
        id: &SandboxId,
        name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        crate::template::snapshot_ops::create_snapshot(&self.docker, &id.to_string(), name).await
    }

    async fn restore_snapshot(&self, snapshot_id: &str) -> Result<Sandbox, DomainError> {
        crate::template::snapshot_ops::restore_snapshot(&self.docker, snapshot_id).await
    }

    async fn snapshot_exists(&self, snapshot_id: &str) -> Result<bool, DomainError> {
        let name = crate::template::snapshot_ops::snapshot_name_from_id(snapshot_id);
        crate::template::snapshot_ops::snapshot_exists(&self.docker, &name).await
    }

    async fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), DomainError> {
        crate::template::snapshot_ops::delete_snapshot(&self.docker, snapshot_id).await
    }

    async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
        crate::template::snapshot_ops::list_snapshots(&self.docker).await
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: true,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 16_384,
            max_cpu_count: 16,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 1500,
        }
    }

    fn name(&self) -> &str {
        "podman"
    }

    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        use bollard::query_parameters::ListContainersOptionsBuilder;

        let options = ListContainersOptionsBuilder::default().all(true).build();

        let containers = self
            .docker
            .list_containers(Some(options))
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to list containers: {e}")))?;

        let mut sandboxes = Vec::new();
        let limit = filter.limit.unwrap_or(u32::MAX) as usize;

        for container in containers.iter().take(limit) {
            // Try to get sandbox ID from container name or ID
            let sandbox_id = container
                .names
                .as_ref()
                .and_then(|names| names.first())
                .and_then(|name| name.strip_prefix('/'))
                .map(|s| s.to_string())
                .or_else(|| container.id.as_ref().map(|s| s.to_string()))
                .unwrap_or_default();

            // Filter by status if specified
            let status = match container.state.as_ref().map(|s| s.as_ref()) {
                Some("running") => SandboxStatus::Running,
                Some("exited") | Some("dead") => SandboxStatus::Stopped,
                Some("paused") => SandboxStatus::Paused,
                Some("created") => SandboxStatus::Pending,
                _ => continue,
            };

            if let Some(ref filter_status) = filter.status
                && status != *filter_status
            {
                continue;
            }

            // Build a minimal Sandbox entity from container info
            // Note: This is best-effort since Podman doesn't store full sandbox metadata
            let sandbox = Sandbox::new(
                SandboxId::new(&sandbox_id),
                bastion_domain::shared::id::TemplateId::new(
                    container.image.as_deref().unwrap_or_default(),
                ),
                bastion_domain::shared::id::ProviderId::new("podman"),
                ResourcesSpec::default(),
                NetworkSpec::default(),
            );

            sandboxes.push(sandbox);
        }

        Ok(sandboxes)
    }

    async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
        let container_name = id.to_string();

        let info = self
            .docker
            .inspect_container(&container_name, None)
            .await
            .map_err(|e| {
                if format!("{e}").contains("404") || format!("{e}").contains("No such container") {
                    DomainError::NotFound(id.to_string())
                } else {
                    DomainError::Internal(format!("Failed to inspect container: {e}"))
                }
            })?;

        let state = info
            .state
            .as_ref()
            .ok_or_else(|| DomainError::Internal("Container has no state".to_string()))?;

        let status = match state.status.as_ref().map(|s| s.as_ref()) {
            Some("running") => SandboxStatus::Running,
            Some("exited") | Some("dead") => SandboxStatus::Stopped,
            Some("paused") => SandboxStatus::Paused,
            Some("created") => SandboxStatus::Pending,
            Some("restarting") => SandboxStatus::Pending,
            _ => SandboxStatus::Failed,
        };

        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(
                info.config
                    .as_ref()
                    .and_then(|c| c.image.clone())
                    .unwrap_or_default(),
            ),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        // Note: We lose expires_at, created_at, etc. from the original sandbox
        // since Podman only gives us current state
        if status == SandboxStatus::Running {
            sandbox.mark_running()?;
        } else if status == SandboxStatus::Stopped {
            let _ = sandbox.terminate();
        } else if status == SandboxStatus::Failed {
            sandbox.mark_failed();
        }

        Ok(sandbox)
    }

    async fn set_timeout(&self, id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        // Verify the container exists
        let container_name = id.to_string();
        let _ = self
            .docker
            .inspect_container(&container_name, None)
            .await
            .map_err(|e| {
                if format!("{e}").contains("404") || format!("{e}").contains("No such container") {
                    DomainError::NotFound(id.to_string())
                } else {
                    DomainError::Internal(format!("Failed to inspect container: {e}"))
                }
            })?;

        // Podman containers don't have a native timeout mechanism.
        // The timeout is managed at the Bastion layer (repository/service).
        // This operation is a no-op at the provider level.
        tracing::debug!(sandbox_id = %id, "set_timeout called on DockerProvider (no-op at provider level)");
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for DockerProvider {
    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(id.as_str())
        {
            tracing::info!(sandbox_id = %id, "Routing command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command(
                    id.as_str(),
                    &command.command,
                    &command.args,
                    command.working_dir.as_deref().unwrap_or("/workspace"),
                    &command.env_vars,
                    timeout_ms,
                )
                .await;
        }

        // Fallback to exec
        let container_name = id.to_string();
        let start = Instant::now();

        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Running command via exec (fallback)"
        );

        // Use exec_in_container for MVP
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
            .exec_in_container(&container_name, &shell_cmd, Some(&command.env_vars))
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
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(id.as_str())
        {
            tracing::info!(sandbox_id = %id, "Streaming command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command_stream(
                    id.as_str(),
                    &command.command,
                    &command.args,
                    command.working_dir.as_deref().unwrap_or("/workspace"),
                    &command.env_vars,
                    timeout_ms,
                )
                .await;
        }

        let container_name = id.to_string();

        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Starting streaming command via exec"
        );

        // For MVP, just run the command and return a stream that yields the result
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
            .exec_in_container(&container_name, &shell_cmd, Some(&command.env_vars))
            .await?;

        // Create an mpsc channel-based stream for the result
        let (tx, rx) = mpsc::channel::<Result<CommandChunk, DomainError>>(4);

        // Spawn a task that sends the chunks
        tokio::spawn(async move {
            // Send stdout
            if !stdout.is_empty() {
                let _ = tx.send(Ok(CommandChunk::stdout(stdout.clone()))).await;
            }
            // Send stderr
            if !stderr.is_empty() {
                let _ = tx.send(Ok(CommandChunk::stderr(stderr.clone()))).await;
            }
            // Send exit code
            let _ = tx.send(Ok(CommandChunk::exit_code(exit_code))).await;
        });

        // Convert mpsc to Stream
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
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(id.as_str())
        {
            tracing::info!(sandbox_id = %id, path, "Writing file via worker registry");
            return router.route_write_file(id.as_str(), path, content).await;
        }

        // Fallback to exec
        let container_name = id.to_string();

        tracing::info!(
            sandbox_id = %id,
            path,
            size = content.len(),
            "Writing file via exec (fallback)"
        );

        // Use base64 encoding for binary content in shell
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(content);
        let shell_cmd = format!("echo '{}' | base64 -d > {}", encoded, path);

        let (_, _, exit_code) = self
            .exec_in_container(&container_name, &shell_cmd, None)
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
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(id.as_str())
        {
            tracing::info!(sandbox_id = %id, path, "Reading file via worker registry");
            return router.route_read_file(id.as_str(), path).await;
        }

        // Fallback to exec
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, path, "Reading file via exec (fallback)");

        // Read file and base64 encode it
        // Use -w0 to disable line wrapping (default wraps at 76 chars, causing decode errors)
        let shell_cmd = format!("base64 -w0 {}", path);
        let (stdout, _, exit_code) = self
            .exec_in_container(&container_name, &shell_cmd, None)
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
            .map_err(|e| DomainError::Internal(format!("Failed to decode base64: {}", e)))?;

        Ok(decoded)
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(id.as_str())
        {
            tracing::info!(sandbox_id = %id, dir, "Listing files via worker registry");
            return router.route_list_files(id.as_str(), dir).await;
        }

        // Fallback to exec
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, dir, "Listing files via exec (fallback)");

        // Use ls for simple listing (just names)
        let shell_cmd = format!("ls -la {}", dir);
        let (stdout, _, exit_code) = self
            .exec_in_container(&container_name, &shell_cmd, None)
            .await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to list files: exit code {}",
                exit_code
            )));
        }

        // Parse ls -la output
        let output_str = String::from_utf8_lossy(&stdout);
        let entries = parse_ls_output(&output_str);
        Ok(entries)
    }

    async fn copy_to(
        &self,
        id: &SandboxId,
        host_dir: &std::path::Path,
        target: &str,
    ) -> Result<(), DomainError> {
        let container_name = id.to_string();

        let mut tar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut tar_bytes);
            ar.append_dir_all(".", host_dir)
                .map_err(|e| DomainError::Internal(format!("Failed to create tar: {e}")))?;
            ar.finish()
                .map_err(|e| DomainError::Internal(format!("Failed to finalize tar: {e}")))?;
        }

        use bollard::query_parameters::UploadToContainerOptions;
        let options = UploadToContainerOptions {
            path: target.to_string(),
            ..Default::default()
        };

        self.docker
            .upload_to_container(
                &container_name,
                Some(options),
                bollard::body_full(bytes::Bytes::from(tar_bytes)),
            )
            .await
            .map_err(|e| {
                DomainError::Internal(format!("Failed to copy files to container: {e}"))
            })?;

        Ok(())
    }
}

/// Parse `ls -la` output into FileEntry structs
pub(crate) fn parse_ls_output(output: &str) -> Vec<FileEntry> {
    use chrono::Utc;

    let mut entries = Vec::new();

    for line in output.lines().skip(1) {
        // Skip total line
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 8 {
            let permissions = parts[0].to_string();
            let is_directory = permissions.starts_with('d');
            let path = parts.last().map(|s| s.to_string()).unwrap_or_default();

            // Size is at index 4
            let size_bytes = parts
                .get(4)
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);

            // For modified_at, use current time as placeholder (parsing ls date format is complex)
            let modified_at = Utc::now();

            if !path.is_empty() && path != "." && path != ".." {
                entries.push(FileEntry {
                    path,
                    is_directory,
                    size_bytes: size_bytes as u64,
                    permissions,
                    modified_at: Some(modified_at),
                });
            }
        }
    }

    entries
}
