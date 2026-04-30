//! Firecracker provider adapter using REST API over Unix socket + serial console.
//!
//! Each sandbox = one Firecracker microVM process.
//! Communication via HTTP PUT/GET over Unix socket for VM lifecycle.
//! Serial console (stdin/stdout) for command execution and file operations.

use async_trait::async_trait;
use base64::Engine;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::broadcast;
use tokio::time::sleep;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

/// VM state including serial I/O handles.
struct VmState {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout_tx: broadcast::Sender<Vec<u8>>,
}

/// Firecracker microVM-based sandbox provider.
///
/// Spawns a Firecracker process per sandbox, communicating via its REST API
/// over a per-VM Unix socket. Each microVM boots a Linux kernel with a
/// root filesystem. Command execution and file operations are done via
/// the serial console (stdin/stdout).
pub struct FirecrackerProvider {
    firecracker_binary: PathBuf,
    kernel_path: PathBuf,
    rootfs_path: PathBuf,
    vm_dir: PathBuf,
    /// Whether the rootfs is squashfs (read-only).
    rootfs_readonly: bool,
    /// Running VM states keyed by sandbox ID.
    vms: Arc<DashMap<String, VmState>>,
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

        // Detect read-only filesystems (squashfs) by magic bytes
        let rootfs_readonly = Self::detect_readonly(&rootfs_path);

        Ok(Self {
            firecracker_binary,
            kernel_path,
            rootfs_path,
            vm_dir,
            rootfs_readonly,
            vms: Arc::new(DashMap::new()),
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

        // Read response headers (until \r\n\r\n)
        let mut header_buf = Vec::new();
        let mut buf = [0u8; 1];
        loop {
            let n = stream.read(&mut buf).await.map_err(|e| {
                DomainError::Internal(format!("Failed to read headers: {e}"))
            })?;
            if n == 0 {
                break;
            }
            header_buf.push(buf[0]);
            // Check for \r\n\r\n terminator
            let len = header_buf.len();
            if len >= 4
                && header_buf[len - 4] == b'\r'
                && header_buf[len - 3] == b'\n'
                && header_buf[len - 2] == b'\r'
                && header_buf[len - 1] == b'\n'
            {
                break;
            }
        }

        let headers_str = String::from_utf8_lossy(&header_buf);
        let status_line = headers_str
            .lines()
            .next()
            .unwrap_or("HTTP/1.1 500 ?");
        let status_code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);

        // Parse Content-Length
        let content_length: usize = headers_str
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        // Read body if present
        let mut body_buf = vec![0u8; content_length];
        if content_length > 0 {
            let mut total = 0;
            while total < content_length {
                let n = stream.read(&mut body_buf[total..]).await.map_err(|e| {
                    DomainError::Internal(format!("Failed to read body: {e}"))
                })?;
                if n == 0 {
                    break;
                }
                total += n;
            }
        }

        let json_str = if content_length > 0 {
            String::from_utf8_lossy(&body_buf[..content_length]).to_string()
        } else {
            "{}".to_string()
        };

        let json: serde_json::Value =
            serde_json::from_str(&json_str).unwrap_or(serde_json::json!({}));

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

    /// Detect if a filesystem image is read-only by checking magic bytes.
    fn detect_readonly(path: &Path) -> bool {
        use std::io::Read;
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        let mut magic = [0u8; 4];
        if file.read_exact(&mut magic).is_err() {
            return false;
        }
        // squashfs magic: 0x68737173 = "hsqs"
        magic == [0x68, 0x73, 0x71, 0x73]
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

    /// Wait for the VM to boot and show a login prompt.
    /// This consumes any boot messages from the serial console.
    async fn wait_for_boot(&self, id: &SandboxId, timeout: Duration) -> Result<(), DomainError> {
        let start = std::time::Instant::now();
        let deadline = start + timeout;

        // Subscribe to the broadcast channel to receive serial output
        let mut rx = {
            let vm = self.vms.get(&id.to_string())
                .ok_or_else(|| DomainError::NotFound(id.to_string()))?;
            vm.stdout_tx.subscribe()
        };

        let mut buf = Vec::new();

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(DomainError::Timeout("VM boot timed out".to_string()));
            }

            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(data)) => {
                    buf.extend_from_slice(&data);
                    let text = String::from_utf8_lossy(&buf);
                    // Look for login prompt or root@ prompt
                    if text.contains("login:") || text.contains("root@") {
                        tracing::debug!(sandbox_id = %id, "VM boot complete, login prompt detected");
                        return Ok(());
                    }
                }
                Ok(Err(_)) => {
                    return Err(DomainError::Internal("Serial console closed during boot".to_string()));
                }
                Err(_) => {
                    return Err(DomainError::Timeout("VM boot timed out waiting for login prompt".to_string()));
                }
            }
        }
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

        // 1. Spawn firecracker process with piped stdin/stdout for serial console
        let mut child = Command::new(&self.firecracker_binary)
            .arg("--api-sock")
            .arg(&socket_path)
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                DomainError::Internal(format!("Failed to spawn Firecracker: {e}"))
            })?;

        // Take ownership of stdout and stdin
        let mut child_stdout = child.stdout.take().expect("stdout not captured");
        let child_stdin = child.stdin.take().expect("stdin not captured");

        // Set up broadcast channel for serial output
        let (stdout_tx, _) = broadcast::channel::<Vec<u8>>(256);
        let stdout_tx_clone = stdout_tx.clone();

        // Spawn background task to continuously read stdout and broadcast
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match child_stdout.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let _ = stdout_tx_clone.send(buf[..n].to_vec());
                    }
                    Err(_) => break,
                }
            }
        });

        // Store VM state
        let vm_state = VmState {
            child,
            stdin: child_stdin,
            stdout_tx,
        };
        self.vms.insert(id.to_string(), vm_state);

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

        // 7. Wait for the VM to boot and show login prompt
        self.wait_for_boot(id, Duration::from_secs(30)).await?;

        tracing::info!(sandbox_id = %id, "Firecracker microVM booted and ready");

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

        // Kill the firecracker process and remove VM state
        if let Some((_, mut vm)) = self.vms.remove(&id.to_string()) {
            let _ = vm.child.kill().await;
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

        // Check if VM exists and child process is still running
        // Use get_mut to allow calling try_wait()
        match self.vms.get_mut(&id.to_string()) {
            Some(mut vm) => {
                // Try to poll the child to see if it's still alive
                match vm.child.try_wait() {
                    Ok(Some(_)) => Ok(false), // Process has exited
                    Ok(None) => Ok(true),     // Still running
                    Err(_) => Ok(false),      // Error checking status
                }
            }
            None => Ok(false),
        }
    }

    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        let mut vm = self.vms.get_mut(&id.to_string())
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;

        let full_cmd = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!("{} {}", command.command, command.args.join(" "))
        };

        let start = std::time::Instant::now();

        // Subscribe to broadcast AFTER clearing any pending output
        let mut rx = vm.stdout_tx.subscribe();

        // Write command to serial console via stdin
        let cmd_line = format!("echo __BASTION_CMD__; {}; echo __BASTION_EXIT__:$?\n", full_cmd);
        tracing::debug!(sandbox_id = %id, cmd = %cmd_line, "Sending command to serial console");
        vm.stdin.write_all(cmd_line.as_bytes()).await
            .map_err(|e| DomainError::Internal(format!("Failed to write to serial console: {e}")))?;

        // Collect output
        let mut all_output = Vec::new();
        let timeout = Duration::from_secs(30);
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                tracing::warn!(sandbox_id = %id, output = ?String::from_utf8_lossy(&all_output), "Command timed out");
                return Err(DomainError::Timeout("Command timed out".to_string()));
            }

            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(data)) => {
                    all_output.extend_from_slice(&data);
                    let text = String::from_utf8_lossy(&all_output);
                    if text.contains("__BASTION_EXIT__:") {
                        break;
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!(n, "Broadcast lagged, some output lost");
                    continue;
                }
                _ => break,
            }
        }

        let output = String::from_utf8_lossy(&all_output).to_string();

        tracing::debug!(sandbox_id = %id, output = %output, "Raw command output");

        // Parse exit code: "__BASTION_EXIT__:0" or "__BASTION_EXIT__:127"
        let exit_code = output
            .split("__BASTION_EXIT__:")
            .nth(1)
            .and_then(|s| s.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect::<String>().parse().ok())
            .unwrap_or(-1);

        tracing::debug!(sandbox_id = %id, exit_code, "Parsed exit code");

        // Extract command output between markers
        let stdout_start = output.find("__BASTION_CMD__").unwrap_or(0) + "__BASTION_CMD__".len();
        let stdout_end = output.find("__BASTION_EXIT__:").unwrap_or(output.len());
        let cmd_output = if stdout_start < stdout_end {
            output[stdout_start..stdout_end].to_string()
        } else {
            String::new()
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        tracing::info!(sandbox_id = %id, exit_code, duration_ms, "Command completed");

        Ok(CommandResult {
            exit_code,
            stdout: cmd_output.into_bytes(),
            stderr: Vec::new(),
            duration_ms,
            timed_out: false,
        })
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
        id: &SandboxId,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        // Use base64 encoding to avoid shell escaping issues
        let encoded = base64::engine::general_purpose::STANDARD.encode(content);
        let command = format!("printf '%s' '{}' | base64 -d > '{}'", encoded, path);

        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to write file: {}",
                stderr
            )));
        }

        Ok(())
    }

    async fn read_file(
        &self,
        id: &SandboxId,
        path: &str,
    ) -> Result<Vec<u8>, DomainError> {
        let command = format!("cat '{}'", path);
        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to read file: {}",
                stderr
            )));
        }

        Ok(result.stdout)
    }

    async fn list_files(
        &self,
        id: &SandboxId,
        dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        let command = format!("ls -la '{}' 2>/dev/null || ls -la '{}'", dir, dir);
        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to list files: {}",
                stderr
            )));
        }

        // Parse ls -la output into FileEntry structs
        let output = String::from_utf8_lossy(&result.stdout);
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
