//! Firecracker provider adapter using REST API over Unix socket.
//!
//! Each sandbox = one Firecracker microVM process.
//! Communication via HTTP PUT/GET over Unix socket.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

/// Firecracker microVM-based sandbox provider.
///
/// Spawns a Firecracker process per sandbox, communicating via its REST API
/// over a per-VM Unix socket. Each microVM boots a Linux kernel with a
/// root filesystem.
pub struct FirecrackerProvider {
    firecracker_binary: PathBuf,
    kernel_path: PathBuf,
    rootfs_path: PathBuf,
    vm_dir: PathBuf,
    /// Whether the rootfs is squashfs (read-only).
    rootfs_readonly: bool,
}

impl std::fmt::Debug for FirecrackerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerProvider")
            .field("firecracker_binary", &self.firecracker_binary)
            .field("kernel_path", &self.kernel_path)
            .field("rootfs_path", &self.rootfs_path)
            .field("rootfs_readonly", &self.rootfs_readonly)
            .finish()
    }
}

impl FirecrackerProvider {
    /// Create a new Firecracker provider.
    ///
    /// * `firecracker_binary` — path to the `firecracker` binary
    /// * `kernel_path` — path to the Linux kernel (vmlinux)
    /// * `rootfs_path` — path to the root filesystem image (ext4 or squashfs)
    /// * `vm_dir` — directory where per-VM sockets and metadata are stored
    pub fn new(
        firecracker_binary: PathBuf,
        kernel_path: PathBuf,
        rootfs_path: PathBuf,
        vm_dir: PathBuf,
    ) -> Result<Self, DomainError> {
        // Validate paths exist
        if !firecracker_binary.exists() {
            return Err(DomainError::Config(format!(
                "Firecracker binary not found: {}",
                firecracker_binary.display()
            )));
        }
        if !kernel_path.exists() {
            return Err(DomainError::Config(format!(
                "Kernel not found: {}",
                kernel_path.display()
            )));
        }
        if !rootfs_path.exists() {
            return Err(DomainError::Config(format!(
                "Rootfs not found: {}",
                rootfs_path.display()
            )));
        }

        std::fs::create_dir_all(&vm_dir).map_err(|e| {
            DomainError::Config(format!("Cannot create VM directory: {e}"))
        })?;

        // Detect read-only filesystems (squashfs)
        let rootfs_readonly = rootfs_path
            .extension()
            .map(|ext| ext == "squashfs")
            .unwrap_or(false);

        Ok(Self {
            firecracker_binary,
            kernel_path,
            rootfs_path,
            vm_dir,
            rootfs_readonly,
        })
    }

    /// Path to the Unix socket for a given sandbox.
    fn socket_path(&self, id: &SandboxId) -> PathBuf {
        self.vm_dir.join(id.to_string()).join("firecracker.sock")
    }

    /// Directory for a sandbox's VM files.
    fn sandbox_dir(&self, id: &SandboxId) -> PathBuf {
        self.vm_dir.join(id.to_string())
    }

    /// Send an HTTP request to the Firecracker API socket.
    async fn api_request(
        socket_path: &Path,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, DomainError> {
        let body_str = body
            .map(|b| serde_json::to_string(b).unwrap_or_default())
            .unwrap_or_default();

        let request = format!(
            "{} {} HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            method,
            path,
            body_str.len(),
            body_str
        );

        let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
            DomainError::Internal(format!(
                "Cannot connect to Firecracker socket {}: {e}",
                socket_path.display()
            ))
        })?;

        stream.write_all(request.as_bytes()).await.map_err(|e| {
            DomainError::Internal(format!("Failed to write to Firecracker socket: {e}"))
        })?;

        // Read response
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.map_err(|e| {
            DomainError::Internal(format!("Failed to read from Firecracker socket: {e}"))
        })?;

        let response = String::from_utf8_lossy(&buf);
        let status_line = response
            .lines()
            .next()
            .unwrap_or("HTTP/1.1 500 ?");
        let status_code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        // Extract JSON body (after double CRLF)
        let json_str = response
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("{}");

        let json: serde_json::Value =
            serde_json::from_str(json_str).unwrap_or(serde_json::json!({}));

        // Firecracker returns 204 No Content for many operations
        if status_code >= 400 {
            let fault = json.get("fault_message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(DomainError::Internal(format!(
                "Firecracker API error {status_code}: {fault}"
            )));
        }

        Ok(json)
    }

    /// Wait for the Firecracker socket to become available (polling).
    async fn wait_for_socket(socket_path: &Path, timeout: Duration) -> Result<(), DomainError> {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if socket_path.exists() {
                // Try connecting
                if UnixStream::connect(socket_path).await.is_ok() {
                    return Ok(());
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
        Err(DomainError::Internal(format!(
            "Firecracker socket did not appear within {}ms: {}",
            timeout.as_millis(),
            socket_path.display()
        )))
    }
}

#[async_trait]
impl SandboxProvider for FirecrackerProvider {
    async fn create(
        &self,
        id: &SandboxId,
        _template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        let sandbox_dir = self.sandbox_dir(id);
        let socket_path = self.socket_path(id);

        std::fs::create_dir_all(&sandbox_dir).map_err(|e| {
            DomainError::Internal(format!("Cannot create sandbox directory: {e}"))
        })?;

        tracing::info!(
            sandbox_id = %id,
            socket = %socket_path.display(),
            kernel = %self.kernel_path.display(),
            "Starting Firecracker microVM"
        );

        // 1. Spawn firecracker process
        let child = Command::new(&self.firecracker_binary)
            .arg("--api-sock")
            .arg(&socket_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                DomainError::Internal(format!("Failed to spawn Firecracker: {e}"))
            })?;

        // Store the PID for later cleanup (we'll rely on kill_on_drop)
        let _pid = child.id();

        // 2. Wait for socket to appear
        Self::wait_for_socket(&socket_path, Duration::from_secs(5)).await?;

        // 3. Configure boot source
        let kernel_path_str = self.kernel_path.to_string_lossy();
        Self::api_request(
            &socket_path,
            "PUT",
            "/boot-source",
            Some(&serde_json::json!({
                "kernel_image_path": kernel_path_str,
                "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
            })),
        )
        .await?;

        // 4. Configure rootfs drive
        let rootfs_path_str = self.rootfs_path.to_string_lossy();
        Self::api_request(
            &socket_path,
            "PUT",
            "/drives/rootfs",
            Some(&serde_json::json!({
                "drive_id": "rootfs",
                "path_on_host": rootfs_path_str,
                "is_root_device": true,
                "is_read_only": self.rootfs_readonly
            })),
        )
        .await?;

        // 5. Configure machine
        Self::api_request(
            &socket_path,
            "PUT",
            "/machine-config",
            Some(&serde_json::json!({
                "vcpu_count": 1,
                "mem_size_mib": 128,
                "smt": false
            })),
        )
        .await?;

        // 6. Start the VM
        Self::api_request(
            &socket_path,
            "PUT",
            "/actions",
            Some(&serde_json::json!({
                "action_type": "InstanceStart"
            })),
        )
        .await?;

        tracing::info!(sandbox_id = %id, "Firecracker microVM started");

        // Build domain entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new("firecracker"),
            bastion_domain::shared::id::ProviderId::new("firecracker"),
            _resources.clone(),
            _network.clone(),
        );
        sandbox.set_timeout(timeout_ms);
        sandbox.mark_running()?;

        Ok(sandbox)
    }

    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
        let socket_path = self.socket_path(id);

        tracing::info!(sandbox_id = %id, "Terminating Firecracker microVM");

        // Try graceful shutdown via API
        if socket_path.exists() {
            let _ = Self::api_request(
                &socket_path,
                "PUT",
                "/actions",
                Some(&serde_json::json!({
                    "action_type": "SendCtrlAltDel"
                })),
            )
            .await;

            // Small delay for graceful shutdown
            sleep(Duration::from_millis(200)).await;
        }

        // Clean up sandbox directory (which also removes the socket)
        let sandbox_dir = self.sandbox_dir(id);
        if sandbox_dir.exists() {
            let _ = std::fs::remove_dir_all(&sandbox_dir);
        }

        tracing::info!(sandbox_id = %id, "Firecracker microVM terminated");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
        let socket_path = self.socket_path(id);
        if !socket_path.exists() {
            return Ok(false);
        }

        // Try to connect to the socket to verify
        match UnixStream::connect(&socket_path).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn run_command(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "Command execution inside Firecracker requires SSH or agent in guest".to_string(),
        ))
    }

    async fn run_command_stream(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "Streaming command execution inside Firecracker requires SSH or agent in guest"
                .to_string(),
        ))
    }

    async fn write_file(
        &self,
        _id: &SandboxId,
        _path: &str,
        _content: &[u8],
    ) -> Result<(), DomainError> {
        Err(DomainError::UnsupportedOperation(
            "File operations inside Firecracker require SSH or agent in guest".to_string(),
        ))
    }

    async fn read_file(
        &self,
        _id: &SandboxId,
        _path: &str,
    ) -> Result<Vec<u8>, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "File operations inside Firecracker require SSH or agent in guest".to_string(),
        ))
    }

    async fn list_files(
        &self,
        _id: &SandboxId,
        _dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        Err(DomainError::UnsupportedOperation(
            "File operations inside Firecracker require SSH or agent in guest".to_string(),
        ))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: true,
            supports_streaming: false,
            supports_pause_resume: false,
            max_timeout_ms: 600_000,
            max_memory_mb: 512,
            max_cpu_count: 4,
            supports_networking: true,
            requires_kvm: true,
            avg_startup_ms: 300,
        }
    }

    fn name(&self) -> &str {
        "firecracker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_validates_paths() {
        let result = FirecrackerProvider::new(
            PathBuf::from("/nonexistent/firecracker"),
            PathBuf::from("/nonexistent/vmlinux"),
            PathBuf::from("/nonexistent/rootfs"),
            PathBuf::from("/tmp/bastion-test"),
        );
        assert!(result.is_err());
    }
}
