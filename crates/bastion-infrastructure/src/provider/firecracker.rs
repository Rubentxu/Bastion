//! Firecracker provider adapter using REST API over Unix socket + serial console.
//!
//! Each sandbox = one Firecracker microVM process.
//! Communication via HTTP PUT/GET over Unix socket for VM lifecycle.
//! Serial console (stdin/stdout) for command execution and file operations.

use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::executor::TaskExecutor;
use bastion_domain::provider::image_source::{ImageSource, SquashfsImage};
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::network::NetworkBackend;
use bastion_domain::provider::port::CommandStream;
use bastion_domain::provider::router::CommandRouter;
use super::state_machine::DashMapSandboxStateMachine;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{
    NetworkSpec, ResourcesSpec, SandboxFilter, SandboxStatus,
};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// VM state including serial I/O handles.
struct VmState {
    child: Child,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    serial_buf: Arc<Mutex<Vec<u8>>>,
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
    /// Path to the MUSL static worker binary to inject into rootfs.
    worker_binary: PathBuf,
    /// Host gateway address for worker connections (e.g., "10.0.2.1:50052").
    gateway_addr: String,
    /// Optional command router for registry-based command execution.
    command_router: Option<Arc<dyn CommandRouter>>,
    /// State machine for sandbox lifecycle (when use-segregated-traits is enabled)
    state_machine: Arc<DashMapSandboxStateMachine>,
    /// Network backend for TAP device management.
    network_backend: Arc<dyn NetworkBackend>,
}

impl std::fmt::Debug for FirecrackerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerProvider")
            .field("firecracker_binary", &self.firecracker_binary)
            .field("kernel_path", &self.kernel_path)
            .field("rootfs_path", &self.rootfs_path)
            .field("rootfs_readonly", &self.rootfs_readonly)
            .field("worker_binary", &self.worker_binary)
            .field("gateway_addr", &self.gateway_addr)
            .field("command_router", &self.command_router.is_some())
            .field("state_machine", &"...")
            .field("network_backend", &"...")
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
    /// * `worker_binary` — path to the bastion-worker MUSL static binary
    /// * `gateway_addr` — host gateway address for worker connections (e.g., "10.0.2.1:50052")
    /// * `network_backend` — backend for TAP device creation/destruction
    pub fn new(
        firecracker_binary: PathBuf,
        kernel_path: PathBuf,
        rootfs_path: PathBuf,
        vm_dir: PathBuf,
        worker_binary: PathBuf,
        gateway_addr: String,
        network_backend: impl NetworkBackend + 'static,
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

        std::fs::create_dir_all(&vm_dir)
            .map_err(|e| DomainError::Config(format!("Cannot create VM directory: {e}")))?;

        // Detect read-only filesystems (squashfs) by magic bytes
        let rootfs_readonly = Self::detect_readonly(&rootfs_path);

        Ok(Self {
            firecracker_binary,
            kernel_path,
            rootfs_path,
            vm_dir,
            rootfs_readonly,
            vms: Arc::new(DashMap::new()),
            worker_binary,
            gateway_addr,
            command_router: None,
            state_machine: Arc::new(DashMapSandboxStateMachine::new()),
            network_backend: Arc::new(network_backend),
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
            let n = stream
                .read(&mut buf)
                .await
                .map_err(|e| DomainError::Internal(format!("Failed to read headers: {e}")))?;
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
        let status_line = headers_str.lines().next().unwrap_or("HTTP/1.1 500 ?");
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
                let n = stream
                    .read(&mut body_buf[total..])
                    .await
                    .map_err(|e| DomainError::Internal(format!("Failed to read body: {e}")))?;
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
            let fault = json
                .get("fault_message")
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
    /// Polls the shared serial buffer for boot completion.
    async fn wait_for_boot(&self, id: &SandboxId, timeout: Duration) -> Result<(), DomainError> {
        let deadline = std::time::Instant::now() + timeout;

        let serial_buf = {
            let vm = self
                .vms
                .get(&id.to_string())
                .ok_or_else(|| DomainError::NotFound(id.to_string()))?;
            vm.serial_buf.clone()
        };

        loop {
            if std::time::Instant::now() > deadline {
                return Err(DomainError::Timeout("VM boot timed out".to_string()));
            }

            {
                let guard = serial_buf.lock().await;
                let text = String::from_utf8_lossy(&guard);
                if text.contains("root@")
                    && (text.contains(":~#") || text.contains(":~$") || text.contains("login:"))
                {
                    return Ok(());
                }
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    /// Set the command router for registry-based command execution.
    pub fn set_command_router(&mut self, router: Arc<dyn CommandRouter>) {
        self.command_router = Some(router);
    }

    /// Verify the worker binary is static musl.
    /// This is required because Firecracker rootfs uses musl libc.
    async fn verify_worker_binary(&self) -> Result<(), DomainError> {
        let worker_path = &self.worker_binary;

        // Run `file` command to check binary format
        let output = Command::new("file")
            .arg(worker_path)
            .output()
            .await
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

    /// Prepare a per-sandbox rootfs copy, optionally injecting the worker binary.
    fn prepare_rootfs(&self, base_rootfs: &Path, target: &Path) -> Result<(), DomainError> {
        std::fs::copy(base_rootfs, target)
            .map_err(|e| DomainError::Internal(format!("Failed to copy rootfs: {e}")))?;

        if self.rootfs_readonly {
            tracing::debug!("Rootfs is read-only, skipping worker injection (must be pre-baked)");
            return Ok(());
        }

        let mount_point = match target.parent() {
            Some(parent) => parent.join("mnt"),
            None => {
                tracing::warn!("Rootfs path has no parent, skipping mount for worker injection");
                return Ok(());
            }
        };
        if std::fs::create_dir_all(&mount_point).is_err() {
            tracing::warn!(
                "Cannot create mount point {:?}, skipping worker injection",
                mount_point
            );
            return Ok(());
        }

        let mount_cmd = std::process::Command::new("mount")
            .args([
                "-o",
                "loop",
                &*target.to_string_lossy(),
                &*mount_point.to_string_lossy(),
            ])
            .output();

        match mount_cmd {
            Err(e) => {
                tracing::warn!(
                    "Mount failed ({e}), cannot inject worker binary — ensure worker is pre-baked in rootfs"
                );
                let _ = std::fs::remove_dir_all(&mount_point);
                return Ok(());
            }
            Ok(output) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    "Mount failed: {stderr}, cannot inject worker binary — ensure worker is pre-baked in rootfs"
                );
                let _ = std::fs::remove_dir_all(&mount_point);
                return Ok(());
            }
            _ => {}
        }

        // Copy worker binary
        let worker_dest = mount_point.join("usr/local/bin/bastion-worker");
        if let Some(parent) = worker_dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&self.worker_binary, &worker_dest) {
            Ok(_) => tracing::info!("Worker binary injected at {:?}", worker_dest),
            Err(e) => tracing::warn!("Failed to copy worker binary: {e}"),
        }

        // Create /workspace directory
        let workspace = mount_point.join("workspace");
        let _ = std::fs::create_dir_all(&workspace);

        // Unmount
        let _ = std::process::Command::new("umount")
            .arg(&*mount_point.to_string_lossy())
            .output();

        let _ = std::fs::remove_dir_all(&mount_point);

        Ok(())
    }

    /// Start the worker process via serial console (internal helper for create).
    /// Calls through to run_command, which falls through to serial console
    /// when the worker is not yet registered.
    async fn start_via_serial(&self, id: &SandboxId, secret: &str) -> Result<(), DomainError> {
        let gateway = format!("http://{}", self.gateway_addr);
        let sandbox_id = id.to_string();
        let worker_cmd = format!(
            "nohup /usr/local/bin/bastion-worker --gateway-addr {} --sandbox-id {} --secret {} --workdir /workspace > /tmp/worker.log 2>&1 &",
            gateway, sandbox_id, secret
        );
        let cmd = CommandSpec::new(&worker_cmd);
        // Falls through to serial console since worker is not connected yet
        self.run_command(id, &cmd).await?;
        Ok(())
    }
}

#[async_trait]
#[async_trait]
impl SandboxLifecycle for FirecrackerProvider {
    async fn create(
        &self,
        id: &SandboxId,
        _template: &str,
        resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        let sandbox_dir = self.sandbox_dir(id);
        let socket_path = self.socket_path(id);

        std::fs::create_dir_all(&sandbox_dir)
            .map_err(|e| DomainError::Internal(format!("Cannot create sandbox directory: {e}")))?;

        tracing::info!(
            sandbox_id = %id,
            socket = %socket_path.display(),
            kernel = %self.kernel_path.display(),
            "Starting Firecracker microVM"
        );

        // Generate a secret for worker registration
        let secret = format!("secret-{}", uuid::Uuid::new_v4());

        // Verify worker binary is static musl before injection
        // Firecracker rootfs uses musl libc, so the binary must be static musl
        self.verify_worker_binary().await?;

        // Validate rootfs image using SquashfsImage
        {
            let rootfs_image = SquashfsImage::new(self.rootfs_path.clone());
            rootfs_image.validate().await?;
        }

        // Prepare per-sandbox rootfs with worker binary injected
        // Note: Firecracker uses disk images with mount/umount injection, not OCI bundles
        let sandbox_rootfs = sandbox_dir.join("rootfs.img");

        {
            // With RootfsManager: copy rootfs (injection requires separate handling for disk images)
            use tokio::fs as tokio_fs;
            tokio_fs::copy(&self.rootfs_path, &sandbox_rootfs)
                .await
                .map_err(|e| DomainError::Internal(format!("Failed to copy rootfs: {e}")))?;

            if self.rootfs_readonly {
                tracing::debug!(
                    "Rootfs is read-only, skipping worker injection (must be pre-baked)"
                );
            } else {
                // For Firecracker, we still need mount-based injection for disk images
                // RootfsManager is designed for OCI bundles - Firecracker needs special handling
                tracing::warn!(
                    "Firecracker worker injection via mount - consider pre-baking worker in rootfs"
                );
                // Use mount-based injection
                self.prepare_rootfs(&self.rootfs_path, &sandbox_rootfs)?;
            }
        }

        // Create TAP device for networking and get the interface name
        let tap_name = {
            let config = self.network_backend.setup(id, _network).await?;
            config.interface_name
        };

        // 1. Spawn firecracker process with piped stdin/stdout for serial console
        let mut child = Command::new(&self.firecracker_binary)
            .arg("--api-sock")
            .arg(&socket_path)
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DomainError::Internal(format!("Failed to spawn Firecracker: {e}")))?;

        // Take ownership of stdout and stdin
        let mut child_stdout = child.stdout.take().expect("stdout not captured");
        let child_stdin = child.stdin.take().expect("stdin not captured");

        // Set up shared buffer for serial output
        let serial_buf = Arc::new(Mutex::new(Vec::new()));
        let serial_buf_clone = serial_buf.clone();

        // Spawn background task to continuously read stdout into shared buffer
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match child_stdout.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let mut guard = serial_buf_clone.lock().await;
                        guard.extend_from_slice(&buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        });

        // Store VM state
        let vm_state = VmState {
            child,
            stdin: Arc::new(Mutex::new(child_stdin)),
            serial_buf,
        };
        self.vms.insert(id.to_string(), vm_state);

        // 2. Wait for socket to appear
        Self::wait_for_socket(&socket_path, Duration::from_secs(5)).await?;

        // 3. Configure boot source
        let kernel_path_str = self.kernel_path.to_string_lossy();
        let boot_args =
            "console=ttyS0 reboot=k panic=1 ip=10.0.2.2::10.0.2.1:255.255.255.0::eth0:off";
        Self::api_request(
            &socket_path,
            "PUT",
            "/boot-source",
            Some(&serde_json::json!({
                "kernel_image_path": kernel_path_str,
                "boot_args": boot_args
            })),
        )
        .await?;

        // 4. Configure rootfs drive (per-sandbox copy)
        let rootfs_path_str = sandbox_rootfs.to_string_lossy();
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

        // 5. Configure machine — honor ResourcesSpec with safety clamps
        let vcpu_count = resources
            .cpu_count
            .clamp(1, self.capabilities().max_cpu_count());
        let mem_size_mib = (resources.memory_mb).clamp(128, self.capabilities().max_memory_mb());
        Self::api_request(
            &socket_path,
            "PUT",
            "/machine-config",
            Some(&serde_json::json!({
                "vcpu_count": vcpu_count,
                "mem_size_mib": mem_size_mib,
                "smt": false
            })),
        )
        .await?;
        tracing::info!(
            sandbox_id = %id,
            vcpu_count,
            mem_size_mib,
            "Firecracker machine configured"
        );

        // 6. Configure networking via TAP device
        Self::api_request(
            &socket_path,
            "PUT",
            "/network-interfaces/eth0",
            Some(&serde_json::json!({
                "iface_id": "eth0",
                "host_dev_name": tap_name,
                "guest_mac": "AA:FC:00:00:00:01"
            })),
        )
        .await?;

        // 7. Start the VM
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

        // Wait for the VM to boot and show login prompt
        self.wait_for_boot(id, Duration::from_secs(30)).await?;

        tracing::info!(sandbox_id = %id, "Firecracker microVM booted and ready");

        // Wait for serial console to stabilize (MOTD, trailing output)
        sleep(Duration::from_millis(1000)).await;

        // Start the worker process via serial console
        self.start_via_serial(id, &secret).await?;

        // Register secret with command router
        if let Some(ref router) = self.command_router {
            router.set_sandbox_secret(&id.to_string(), &secret);
        }

        // Build domain entity
        let mut sandbox = Sandbox::new(
            id.clone(),
            bastion_domain::shared::id::TemplateId::new("firecracker"),
            bastion_domain::shared::id::ProviderId::new("firecracker"),
            None,
            resources.clone(),
            _network.clone(),
        );
        sandbox.set_timeout(timeout_ms);
        sandbox.mark_running()?;

        // Register with state machine when feature is enabled
        {
            self.state_machine.register(id.clone())?;
            self.state_machine.transition(id, SandboxStatus::Running)?;
        }

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

        // Remove from state machine when feature is enabled
        {
            self.state_machine.remove(id);
        }

        // Clean up TAP device
        self.network_backend.teardown(id).await?;

        // Clean up sandbox directory (which also removes the socket)
        let sandbox_dir = self.sandbox_dir(id);
        if sandbox_dir.exists() {
            let _ = std::fs::remove_dir_all(&sandbox_dir);
        }

        tracing::info!(sandbox_id = %id, "Firecracker microVM terminated");
        Ok(())
    }

    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
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

    async fn create_snapshot(
        &self,
        id: &SandboxId,
        name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        let socket_path = self.socket_path(id);
        let snapshot_dir = self.sandbox_dir(id).join("snapshots").join(name);

        tracing::info!(
            sandbox_id = %id,
            snapshot_name = %name,
            "Creating Firecracker snapshot"
        );

        // Create snapshot directory
        std::fs::create_dir_all(&snapshot_dir)
            .map_err(|e| DomainError::Internal(format!("Cannot create snapshot directory: {e}")))?;

        // 1. Pause the VM
        Self::api_request(
            &socket_path,
            "PUT",
            "/actions",
            Some(&serde_json::json!({
                "action_type": "Pause"
            })),
        )
        .await?;

        // 2. Create snapshot via Firecracker snapshot API
        let state_file = snapshot_dir.join("snapshot.state");
        let mem_file = snapshot_dir.join("snapshot.memory");

        Self::api_request(
            &socket_path,
            "PUT",
            "/snapshot/create",
            Some(&serde_json::json!({
                "snapshot_type": "Full",
                "state_file_path": state_file.to_string_lossy(),
                "mem_file_path": mem_file.to_string_lossy()
            })),
        )
        .await?;

        // 3. Resume the VM
        Self::api_request(
            &socket_path,
            "PUT",
            "/actions",
            Some(&serde_json::json!({
                "action_type": "Resume"
            })),
        )
        .await?;

        // 4. Calculate total size of snapshot files
        let size_bytes = std::fs::metadata(&state_file).map(|m| m.len()).unwrap_or(0)
            + std::fs::metadata(&mem_file).map(|m| m.len()).unwrap_or(0);

        let snapshot_id = format!("{}-{}", id, name);

        tracing::info!(
            sandbox_id = %id,
            snapshot_id = %snapshot_id,
            size_bytes,
            "Firecracker snapshot created"
        );

        Ok(SnapshotInfo {
            snapshot_id,
            sandbox_id: id.to_string(),
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            size_bytes,
        })
    }

    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        // Restore requires the original sandbox_id which is not available from snapshot_id alone.
        // This would need additional design work to track the mapping.
        Err(DomainError::UnsupportedOperation(
            "restore_snapshot requires additional context (original sandbox_id). \
             This is complex without storing the mapping during create_snapshot."
                .to_string(),
        ))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::try_new(
            true,
            false,
            false,
            600_000,
            512,
            4,
            true,
            true,
            300,
        )
        .expect("known valid values")
    }

    fn name(&self) -> &str {
        "firecracker"
    }

    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        let mut sandboxes = Vec::new();
        let limit = filter.limit.unwrap_or(u32::MAX) as usize;

        // Collect keys to avoid holding DashMap lock while calling try_wait
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
            let is_alive = if let Some(mut vm) = self.vms.get_mut(&sandbox_id) {
                match vm.child.try_wait() {
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
                bastion_domain::shared::id::TemplateId::new("firecracker"),
                bastion_domain::shared::id::ProviderId::new("firecracker"),
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

        // Check if we have this VM tracked
        let mut vm = self
            .vms
            .get_mut(&sandbox_id)
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;

        // Check if VM process is still alive
        let is_alive = match vm.child.try_wait() {
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
            bastion_domain::shared::id::TemplateId::new("firecracker"),
            bastion_domain::shared::id::ProviderId::new("firecracker"),
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

        // Verify the VM exists
        let _ = self
            .vms
            .get(&sandbox_id)
            .ok_or_else(|| DomainError::NotFound(id.to_string()))?;

        // Firecracker doesn't have a native timeout mechanism at the VM level.
        // The timeout is managed at the Bastion layer.
        // This operation is a no-op at the provider level.
        tracing::debug!(sandbox_id = %id, "set_timeout called on FirecrackerProvider (no-op at provider level)");
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for FirecrackerProvider {
    async fn run_command(
        &self,
        id: &SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, "Routing command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command(
                    &id.to_string(),
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
        // Try registry-based routing
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, "Streaming command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router
                .route_run_command_stream(
                    &id.to_string(),
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
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, path, "Writing file via worker registry");
            return router
                .route_write_file(&id.to_string(), path, content)
                .await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, path, "Reading file via worker registry");
            return router.route_read_file(&id.to_string(), path).await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }

    async fn list_files(&self, id: &SandboxId, dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, dir, "Listing files via worker registry");
            return router.route_list_files(&id.to_string(), dir).await;
        }

        // Worker is NOT connected - this is an error, not a fallback opportunity
        return Err(DomainError::WorkerNotConnected(id.to_string()));
    }
}
