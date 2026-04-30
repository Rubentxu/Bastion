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
use bastion_domain::provider::port::{CommandStream, SandboxProvider};
use bastion_domain::provider::router::CommandRouter;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

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
    pub fn new(
        firecracker_binary: PathBuf,
        kernel_path: PathBuf,
        rootfs_path: PathBuf,
        vm_dir: PathBuf,
        worker_binary: PathBuf,
        gateway_addr: String,
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
            worker_binary,
            gateway_addr,
            command_router: None,
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
    /// Polls the shared serial buffer for boot completion.
    async fn wait_for_boot(&self, id: &SandboxId, timeout: Duration) -> Result<(), DomainError> {
        let deadline = std::time::Instant::now() + timeout;

        let serial_buf = {
            let vm = self.vms.get(&id.to_string())
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
                if text.contains("root@") && (text.contains(":~#") || text.contains(":~$") || text.contains("login:")) {
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

    /// Create a TAP device for VM networking.
    fn create_tap_device(&self, tap_name: &str) -> Result<(), DomainError> {
        let output = std::process::Command::new("ip")
            .args(["tuntap", "add", "dev", tap_name, "mode", "tap"])
            .output()
            .map_err(|e| DomainError::Internal(format!("Failed to create TAP: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("File exists") {
                tracing::warn!(tap_name, "TAP creation warning: {stderr}");
            }
        }
        let _ = std::process::Command::new("ip")
            .args(["addr", "add", "10.0.2.1/24", "dev", tap_name])
            .output();
        let _ = std::process::Command::new("ip")
            .args(["link", "set", tap_name, "up"])
            .output();
        Ok(())
    }

    /// Destroy a TAP device.
    fn destroy_tap_device(&self, tap_name: &str) {
        let _ = std::process::Command::new("ip")
            .args(["link", "set", tap_name, "down"])
            .output();
        let _ = std::process::Command::new("ip")
            .args(["tuntap", "del", "dev", tap_name, "mode", "tap"])
            .output();
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
            tracing::warn!("Cannot create mount point {:?}, skipping worker injection", mount_point);
            return Ok(());
        }

        let mount_cmd = std::process::Command::new("mount")
            .args(["-o", "loop", &*target.to_string_lossy(), &*mount_point.to_string_lossy()])
            .output();

        match mount_cmd {
            Err(e) => {
                tracing::warn!("Mount failed ({e}), cannot inject worker binary — ensure worker is pre-baked in rootfs");
                let _ = std::fs::remove_dir_all(&mount_point);
                return Ok(());
            }
            Ok(output) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("Mount failed: {stderr}, cannot inject worker binary — ensure worker is pre-baked in rootfs");
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

    /// Sanitize sandbox ID for use as a Linux interface name (max 15 chars, no underscores).
    fn tap_name(id: &SandboxId) -> String {
        format!(
            "tap-{}",
            id.to_string()
                .replace('_', "-")
                .chars()
                .take(12)
                .collect::<String>()
        )
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
impl SandboxProvider for FirecrackerProvider {
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

        std::fs::create_dir_all(&sandbox_dir).map_err(|e| {
            DomainError::Internal(format!("Cannot create sandbox directory: {e}"))
        })?;

        tracing::info!(
            sandbox_id = %id,
            socket = %socket_path.display(),
            kernel = %self.kernel_path.display(),
            "Starting Firecracker microVM"
        );

        // Generate a secret for worker registration
        let secret = format!("secret-{}", uuid::Uuid::new_v4());

        // Prepare per-sandbox rootfs with worker binary injected
        let sandbox_rootfs = sandbox_dir.join("rootfs.img");
        self.prepare_rootfs(&self.rootfs_path, &sandbox_rootfs)?;

        // Create TAP device for networking
        let tap_name = Self::tap_name(id);
        self.create_tap_device(&tap_name)?;

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
        let boot_args = "console=ttyS0 reboot=k panic=1 ip=10.0.2.2::10.0.2.1:255.255.255.0::eth0:off";
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
        let vcpu_count = resources.cpu_count.clamp(1, self.capabilities().max_cpu_count);
        let mem_size_mib = (resources.memory_mb).clamp(128, self.capabilities().max_memory_mb);
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
            resources.clone(),
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

        // Clean up TAP device
        let tap_name = Self::tap_name(id);
        self.destroy_tap_device(&tap_name);

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
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, "Routing command via worker registry");
            let timeout_ms = command.timeout_ms.unwrap_or(30000);
            return router.route_run_command(
                &id.to_string(),
                &command.command,
                &command.args,
                command.working_dir.as_deref().unwrap_or("/workspace"),
                &command.env_vars,
                timeout_ms,
            ).await;
        }

        // Fallback to serial console
        let full_cmd = if command.args.is_empty() {
            command.command.clone()
        } else {
            format!("{} {}", command.command, command.args.join(" "))
        };

        let start = std::time::Instant::now();

        // Get serial buffer and stdin
        let (serial_buf, stdin) = {
            let vm = self.vms.get(&id.to_string())
                .ok_or_else(|| DomainError::NotFound(id.to_string()))?;
            (vm.serial_buf.clone(), vm.stdin.clone())
        };

        // Send newline to get fresh prompt
        {
            let mut stdin_guard = stdin.lock().await;
            stdin_guard.write_all(b"\n").await
                .map_err(|e| DomainError::Internal(format!("Failed to write: {e}")))?;
        }
        sleep(Duration::from_millis(300)).await;

        let start_pos = {
            let guard = serial_buf.lock().await;
            guard.len()
        };

        // Use a unique marker on a SHORT separate line to avoid truncation
        let marker = format!("E{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_micros() % 999999);
        
        // Send commands one line at a time for reliability.
        // Disable local echo and prompt to keep output clean.
        let commands = vec![
            format!("stty -echo 2>/dev/null; PS1=''; echo __{}_S__", marker),   // start marker
            full_cmd,                                                             // actual command
            format!("echo __{}_E__:$?", marker),                                  // end marker with exit code
        ];

        for cmd in &commands {
            let mut stdin_guard = stdin.lock().await;
            stdin_guard.write_all(cmd.as_bytes()).await
                .map_err(|e| DomainError::Internal(format!("Failed to write: {e}")))?;
            stdin_guard.write_all(b"\n").await
                .map_err(|e| DomainError::Internal(format!("Failed to write: {e}")))?;
            drop(stdin_guard);
            sleep(Duration::from_millis(20)).await;
        }

        // Poll for end marker
        let timeout = Duration::from_secs(30);
        let deadline = std::time::Instant::now() + timeout;

        loop {
            if std::time::Instant::now() > deadline {
                let guard = serial_buf.lock().await;
                let tail = String::from_utf8_lossy(&guard[start_pos..]);
                return Err(DomainError::Timeout(format!(
                    "Command timed out. Last output: {}",
                    &tail[tail.len().saturating_sub(200)..]
                )));
            }

            {
                let guard = serial_buf.lock().await;
                if guard.len() > start_pos {
                    let tail = String::from_utf8_lossy(&guard[start_pos..]);
                    if let Some(end_pos) = tail.rfind(&format!("__{}_E__:", marker)) {
                        // Parse exit code
                        let after = &tail[end_pos + marker.len() + 7..];
                        let exit_code: i32 = after
                            .chars()
                            .take_while(|c| c.is_ascii_digit() || *c == '-')
                            .collect::<String>()
                            .parse()
                            .unwrap_or(-1);

                        // Extract output: everything between start marker and end marker
                        let start_marker = format!("__{}_S__", marker);
                        let out_start = tail.find(&start_marker)
                            .map(|p| p + start_marker.len())
                            .unwrap_or(0);
                        let out_end = tail.rfind(&format!("__{}_E__:", marker)).unwrap_or(tail.len());
                        
                        let cmd_output = if out_start < out_end {
                            tail[out_start..out_end]
                                .lines()
                                .filter(|l| {
                                    let trimmed = l.trim();
                                    !trimmed.is_empty()
                                    && !trimmed.contains(&start_marker)
                                    && !trimmed.contains("__E__")
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                                .trim()
                                .to_string()
                        } else {
                            String::new()
                        };

                        let duration_ms = start.elapsed().as_millis() as u64;
                        tracing::info!(sandbox_id = %id, exit_code, duration_ms, "Command completed");
                        
                        return Ok(CommandResult {
                            exit_code,
                            stdout: cmd_output.into_bytes(),
                            stderr: Vec::new(),
                            duration_ms,
                            timed_out: false,
                        });
                    }
                }
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn run_command_stream(
        &self,
        id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        // Try registry-based routing
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, "Streaming command via worker registry");
            let timeout_ms = _command.timeout_ms.unwrap_or(30000);
            return router.route_run_command_stream(
                &id.to_string(),
                &_command.command,
                &_command.args,
                _command.working_dir.as_deref().unwrap_or("/workspace"),
                &_command.env_vars,
                timeout_ms,
            ).await;
        }

        Err(DomainError::UnsupportedOperation(
            "Streaming command execution requires a connected worker via the CommandRouter".to_string(),
        ))
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
            return router.route_write_file(&id.to_string(), path, content).await;
        }

        // Fallback to serial console
        let text = String::from_utf8_lossy(content).replace('\'', "'\\''");
        let command = format!("printf '%s' '{}' > '{}'", text, path);

        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stdout = String::from_utf8_lossy(&result.stdout);
            return Err(DomainError::Internal(format!(
                "Failed to write file (exit {}): {}",
                result.exit_code,
                stdout
            )));
        }

        Ok(())
    }

    async fn read_file(
        &self,
        id: &SandboxId,
        path: &str,
    ) -> Result<Vec<u8>, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, path, "Reading file via worker registry");
            return router.route_read_file(&id.to_string(), path).await;
        }

        // Fallback to serial console
        let command = format!("cat '{}'", path);
        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stdout = String::from_utf8_lossy(&result.stdout);
            return Err(DomainError::Internal(format!(
                "Failed to read file (exit {}): {}",
                result.exit_code, stdout
            )));
        }

        Ok(result.stdout)
    }

    async fn list_files(
        &self,
        id: &SandboxId,
        dir: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        // Try registry-based routing first
        if let Some(ref router) = self.command_router
            && router.is_worker_connected(&id.to_string())
        {
            tracing::info!(sandbox_id = %id, dir, "Listing files via worker registry");
            return router.route_list_files(&id.to_string(), dir).await;
        }

        // Fallback to serial console
        let command = format!("ls -la '{}' 2>/dev/null || ls -la '{}'", dir, dir);
        let cmd = CommandSpec::new(&command);
        let result = self.run_command(id, &cmd).await?;

        if result.exit_code != 0 {
            let stdout = String::from_utf8_lossy(&result.stdout);
            return Err(DomainError::Internal(format!(
                "Failed to list files (exit {}): {}",
                result.exit_code, stdout
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
            PathBuf::from("/nonexistent/bastion-worker"),
            "10.0.2.1:50052".to_string(),
        );
        assert!(result.is_err());
    }
}
