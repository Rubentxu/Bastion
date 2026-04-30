//! Podman provider adapter using bollard Docker API client.
//!
//! Creates containers with `sleep infinity`, injects worker binary,
//! and communicates via gRPC.

use async_trait::async_trait;
use bollard::body_full;
use bollard::query_parameters::UploadToContainerOptionsBuilder;
use bollard::Docker;
use bollard::exec::StartExecResults;
use bollard::container::LogOutput;
use prost::bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use crate::sandbox::v1::worker_agent_client::WorkerAgentClient;
use crate::sandbox::v1::{RunCommandRequest, ListFilesRequest, ReadFileRequest, WriteFileRequest};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tonic::transport::Channel;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::execution::stream::CommandChunk;
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Podman-based sandbox provider using bollard Docker API client.
/// Communicates with containers via gRPC WorkerAgent.
pub struct PodmanProvider {
    docker: Docker,
    default_image: String,
    /// Path to the worker binary to inject into containers
    worker_binary: PathBuf,
    /// gRPC clients per container
    clients: DashMap<String, WorkerAgentClient<Channel>>,
}

// Manual Debug impl because Docker doesn't derive Debug
impl std::fmt::Debug for PodmanProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodmanProvider")
            .field("default_image", &self.default_image)
            .field("worker_binary", &self.worker_binary)
            .finish_non_exhaustive()
    }
}

impl PodmanProvider {
    /// Connect to Podman via Unix socket.
    pub fn new(socket_path: &str, default_image: &str, worker_binary: PathBuf) -> Result<Self, DomainError> {
        let docker = Docker::connect_with_unix(socket_path, 120, bollard::API_DEFAULT_VERSION)
            .map_err(|e| DomainError::ProviderUnavailable(e.to_string()))?;

        Ok(Self {
            docker,
            default_image: default_image.to_string(),
            worker_binary,
            clients: DashMap::new(),
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

    /// Inject the worker binary into the container and connect via gRPC.
    async fn inject_and_start_worker(&self, container_name: &str, grpc_addr: &str) -> Result<(), DomainError> {
        // Read the worker binary
        let worker_bin = tokio::fs::read(&self.worker_binary)
            .await
            .map_err(|e| DomainError::Internal(format!("Cannot read worker binary: {e}")))?;

        // Create a tar archive containing the worker binary
        let mut tar_bytes = Vec::new();
        {
            let mut tar = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(worker_bin.len() as u64);
            header.set_mode(0o755);
            header.set_path("usr/local/bin/bastion-worker").unwrap();
            tar.append_data(&mut header, "usr/local/bin/bastion-worker", &worker_bin[..])
                .unwrap();
            tar.finish().unwrap();
        }

        // Copy to container using upload_to_container
        let options = UploadToContainerOptionsBuilder::default()
            .path("/")
            .build();
        self.docker
            .start_exec(&exec.id, Some(bollard::exec::StartExecOptions {
                detach: true,
                ..Default::default()
            }))
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to start worker: {e}")))?;

        tracing::debug!(container = %container_name, "Worker process started");

        // Wait for worker to start
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Connect to worker via mapped host port
        let channel = Channel::from_shared(grpc_addr.to_string())
            .map_err(|e| DomainError::Internal(format!("Invalid gRPC address: {e}")))?
            .connect()
            .await
            .map_err(|e| DomainError::Internal(format!("Cannot connect to worker at {}: {e}", grpc_addr)))?;

        let client = WorkerAgentClient::new(channel);

        // Store the client
        self.clients.insert(container_name.to_string(), client);

        tracing::info!(container = %container_name, addr = %grpc_addr, "Worker gRPC client connected");
        Ok(())
    }

    /// Execute a command inside a container and collect output via bollard exec (fallback).
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

    /// Get the host port mapped to container port 50051.
    async fn get_mapped_port(&self, container_name: &str) -> Result<u16, DomainError> {
        let info = self
            .docker
            .inspect_container(container_name, None)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to inspect container: {e}")))?;

        let port = info
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .and_then(|ports| ports.get("50051/tcp"))
            .and_then(|bindings| bindings.as_ref())
            .and_then(|bindings| bindings.first())
            .and_then(|binding| binding.host_port.as_ref())
            .and_then(|port_str| port_str.parse::<u16>().ok())
            .unwrap_or(50051);

        Ok(port)
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

        tracing::info!(sandbox_id = %id, image = %image, "Creating Podman container");

        // Build env vars as "KEY=VALUE" strings
        let env: Vec<String> = env_vars.iter().map(|(k, v)| format!("{k}={v}")).collect();

        // Create container with port mapping for worker gRPC
        let mut port_bindings = std::collections::HashMap::new();
        port_bindings.insert(
            "50051/tcp".to_string(),
            Some(vec![bollard::models::PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("0".to_string()), // Random port
            }]),
        );

        let container_config = bollard::models::ContainerCreateBody {
            image: Some(image),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            env: Some(env),
            tty: Some(false),
            attach_stdout: Some(false),
            attach_stderr: Some(false),
            host_config: Some(bollard::models::HostConfig {
                port_bindings: Some(port_bindings),
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

        // Get mapped host port for worker gRPC
        let grpc_port = self.get_mapped_port(&container_name).await?;
        let grpc_addr = format!("http://127.0.0.1:{}", grpc_port);

        // Inject worker binary and start gRPC service
        self.inject_and_start_worker(&container_name, &grpc_addr).await?;

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

        // Remove the gRPC client
        self.clients.remove(&container_name);

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
                    Err(DomainError::Internal(format!("Failed to inspect container: {e}")))
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
            "Running command via WorkerAgent gRPC"
        );

        // Get gRPC client
        let mut client = self
            .clients
            .get(&container_name)
            .ok_or_else(|| DomainError::NotFound(container_name.clone()))?
            .value()
            .clone();

        let req = RunCommandRequest {
            command: command.command.clone(),
            args: command.args.clone(),
            timeout_ms: 30000,
        };

        let mut stream = client
            .run_command(req)
            .await
            .map_err(|e| DomainError::Internal(format!("gRPC error: {e}")))?
            .into_inner();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = -1;

        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|e| DomainError::Internal(format!("Stream error: {e}")))?
        {
            match chunk.r#type {
                0 => stdout.extend_from_slice(&chunk.data), // STDOUT
                1 => stderr.extend_from_slice(&chunk.data), // STDERR
                2 => {
                    // EXIT_CODE
                    if chunk.data.len() >= 4 {
                        exit_code = i32::from_le_bytes(chunk.data[..4].try_into().unwrap());
                    }
                }
                _ => {}
            }
        }

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
        let container_name = id.to_string();

        tracing::info!(
            sandbox_id = %id,
            command = %command.command,
            "Starting streaming command via WorkerAgent gRPC"
        );

        // Get gRPC client
        let mut client = self
            .clients
            .get(&container_name)
            .ok_or_else(|| DomainError::NotFound(container_name.clone()))?
            .value()
            .clone();

        let req = RunCommandRequest {
            command: command.command.clone(),
            args: command.args.clone(),
            timeout_ms: 30000,
        };

        let stream = client
            .run_command(req)
            .await
            .map_err(|e| DomainError::Internal(format!("gRPC error: {e}")))?
            .into_inner();

        // Convert gRPC stream to CommandChunk stream
        let stream = stream.map(|result| {
            match result {
                Ok(chunk) => {
                    match chunk.r#type {
                        0 => Ok(CommandChunk::stdout(chunk.data)), // STDOUT
                        1 => Ok(CommandChunk::stderr(chunk.data)), // STDERR
                        2 => {
                            // EXIT_CODE
                            let code = if chunk.data.len() >= 4 {
                                i32::from_le_bytes(chunk.data[..4].try_into().unwrap())
                            } else {
                                -1
                            };
                            Ok(CommandChunk::exit_code(code))
                        }
                        _ => Ok(CommandChunk::stdout(chunk.data)),
                    }
                }
                Err(e) => Ok(CommandChunk::error(e.to_string())),
            }
        });

        Ok(Box::pin(stream))
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
            "Writing file via WorkerAgent gRPC"
        );

        // Get gRPC client
        let mut client = self
            .clients
            .get(&container_name)
            .ok_or_else(|| DomainError::NotFound(container_name.clone()))?
            .value()
            .clone();

        let req = WriteFileRequest {
            path: path.to_string(),
            content: content.to_vec(),
        };

        client
            .write_file(req)
            .await
            .map_err(|e| DomainError::Internal(format!("gRPC error: {e}")))?;

        tracing::info!(sandbox_id = %id, path, "File written");
        Ok(())
    }

    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, path, "Reading file via WorkerAgent gRPC");

        // Get gRPC client
        let mut client = self
            .clients
            .get(&container_name)
            .ok_or_else(|| DomainError::NotFound(container_name.clone()))?
            .value()
            .clone();

        let req = ReadFileRequest {
            path: path.to_string(),
        };

        let response = client
            .read_file(req)
            .await
            .map_err(|e| DomainError::Internal(format!("gRPC error: {e}")))?;

        Ok(response.into_inner().content)
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        let container_name = id.to_string();

        tracing::info!(sandbox_id = %id, dir, "Listing files via WorkerAgent gRPC");

        // Get gRPC client
        let mut client = self
            .clients
            .get(&container_name)
            .ok_or_else(|| DomainError::NotFound(container_name.clone()))?
            .value()
            .clone();

        let req = ListFilesRequest {
            directory: dir.to_string(),
            recursive: false,
        };

        let response = client
            .list_files(req)
            .await
            .map_err(|e| DomainError::Internal(format!("gRPC error: {e}")))?;

        let entries = response
            .into_inner()
            .entries
            .into_iter()
            .map(|e| FileEntry {
                path: e.path,
                is_directory: e.is_directory,
                size_bytes: e.size_bytes as u64,
                permissions: e.permissions,
                modified_at: None,
            })
            .collect();

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
