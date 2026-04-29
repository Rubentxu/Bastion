//! Podman provider adapter using bollard Docker API client.
//!
//! Creates containers with `sleep infinity`, runs commands via exec API.

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use futures::StreamExt;
use base64::Engine;
use std::collections::HashMap;
use std::time::Instant;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

/// Podman-based sandbox provider using bollard Docker API client.
pub struct PodmanProvider {
    docker: Docker,
    default_image: String,
}

// Manual Debug impl because Docker doesn't derive Debug
impl std::fmt::Debug for PodmanProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodmanProvider")
            .field("default_image", &self.default_image)
            .finish_non_exhaustive()
    }
}

impl PodmanProvider {
    /// Connect to Podman via Unix socket.
    pub fn new(socket_path: &str, default_image: &str) -> Result<Self, DomainError> {
        let docker = Docker::connect_with_unix(
            socket_path,
            120,
            bollard::API_DEFAULT_VERSION,
        )
        .map_err(|e| DomainError::ProviderUnavailable(e.to_string()))?;

        Ok(Self {
            docker,
            default_image: default_image.to_string(),
        })
    }

    /// Ping the Podman daemon to verify connectivity.
    pub async fn ping(&self) -> Result<String, DomainError> {
        self.docker
            .ping()
            .await
            .map_err(|e| DomainError::ProviderUnavailable(e.to_string()))
            .map(|pong| format!("{pong:?}"))
    }

    /// Execute a command inside a container and collect output.
    async fn exec_in_container(
        &self,
        container_name: &str,
        command: &str,
    ) -> Result<(Vec<u8>, Vec<u8>, i32), DomainError> {
        let exec_config = bollard::exec::CreateExecOptions {
            cmd: Some(vec!["sh".to_string(), "-c".to_string(), command.to_string()]),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
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
}

#[async_trait]
impl SandboxProvider for PodmanProvider {
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        let image = if template.is_empty() {
            self.default_image.clone()
        } else {
            template.to_string()
        };
        let container_name = id.to_string();

        tracing::info!(
            sandbox_id = %id,
            image = %image,
            "Creating Podman container"
        );

        // Build env vars as "KEY=VALUE" strings
        let env: Vec<String> = env_vars
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        // Create container running `sleep infinity` to keep it alive
        let container_config = bollard::models::ContainerCreateBody {
            image: Some(image),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            env: Some(env),
            tty: Some(false),
            attach_stdout: Some(false),
            attach_stderr: Some(false),
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
            .start_container(&container_name, None::<bollard::query_parameters::StartContainerOptions>)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to start container: {e}")))?;

        tracing::info!(sandbox_id = %id, "Container started successfully");

        // Build domain entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new(template),
            bastion_domain::shared::id::ProviderId::new("podman"),
            _resources.clone(),
            _network.clone(),
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

        if let Err(e) = self.docker.stop_container(&container_name, Some(stop_options)).await {
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
                let running = info
                    .state
                    .as_ref()
                    .and_then(|s| s.running)
                    .unwrap_or(false);
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

    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        let container_name = id.to_string();
        let start = Instant::now();

        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Running command via Podman exec"
        );

        // Build full command string
        let full_command = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!("{} {}", command.command, command.args.join(" "))
        };

        let (stdout, stderr, exit_code) =
            self.exec_in_container(&container_name, &full_command).await?;

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
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        // TODO: Implement streaming via exec with streaming output
        Err(DomainError::UnsupportedOperation(
            "streaming not yet implemented".to_string(),
        ))
    }

    async fn write_file(
        &self,
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        let container_name = id.to_string();

        tracing::info!(
            sandbox_id = %id,
            path,
            size = content.len(),
            "Writing file via Podman exec"
        );

        // Use base64 to avoid shell escaping issues
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(content);
        let command = format!("printf '%s' '{encoded}' | base64 -d > '{path}'");

        let (_, stderr, exit_code) =
            self.exec_in_container(&container_name, &command).await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to write file: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }

        tracing::info!(sandbox_id = %id, path, "File written");
        Ok(())
    }

    async fn read_file(
        &self,
        id: &SandboxId,
        path: &str,
    ) -> Result<Vec<u8>, DomainError> {
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, path, "Reading file via Podman exec");

        let (stdout, stderr, exit_code) = self
            .exec_in_container(&container_name, &format!("cat '{path}'"))
            .await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to read file: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }

        Ok(stdout)
    }

    async fn list_files(
        &self,
        id: &SandboxId,
        dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, dir, "Listing files via Podman exec");

        let command = format!("ls -la '{dir}' 2>/dev/null || ls -la '{dir}'");
        let (stdout, stderr, exit_code) =
            self.exec_in_container(&container_name, &command).await?;

        if exit_code != 0 {
            return Err(DomainError::Internal(format!(
                "Failed to list files: {}",
                String::from_utf8_lossy(&stderr)
            )));
        }

        // Parse ls -la output into FileEntry structs
        let output = String::from_utf8_lossy(&stdout);
        let mut entries = Vec::new();

        for line in output.lines().skip(1) {
            // Skip "total N" header
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

            // Skip . and .. entries
            if name == "." || name == ".." {
                continue;
            }

            let path = if dir.ends_with('/') {
                format!("{dir}{name}")
            } else {
                format!("{dir}/{name}")
            };

            entries.push(FileEntry {
                path,
                is_directory,
                size_bytes,
                modified_at: None,
                permissions,
            });
        }

        Ok(entries)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: false,
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
}
