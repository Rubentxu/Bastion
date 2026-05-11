//! TestTerminal — Process spawning harness with health checking and auto-cleanup.
//!
//! Provides a managed test fixture that:
//! - Spawns bastion-gateway or bastion-worker binaries with isolated ports
//! - Tracks stdout for pattern matching
//! - Automatically cleans up processes and temp directories on Drop

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

/// Configuration for spawning a gateway/worker process.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Path to the binary to run.
    pub binary_path: PathBuf,
    /// Port to listen on (0 = pick random).
    pub port: u16,
    /// Log level (e.g., "debug", "info", "warn").
    pub log_level: String,
    /// Optional metrics database path.
    pub metrics_db: Option<PathBuf>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::new(),
            port: 0,
            log_level: "info".to_string(),
            metrics_db: None,
        }
    }
}

impl GatewayConfig {
    /// Builder-style constructor for the gateway binary.
    pub fn gateway() -> Self {
        let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/debug/bastion-gateway");

        Self {
            binary_path: binary,
            port: 0,
            log_level: "info".to_string(),
            metrics_db: None,
        }
    }

    /// Set the port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the log level.
    pub fn with_log_level(mut self, level: &str) -> Self {
        self.log_level = level.to_string();
        self
    }

    /// Set the metrics database path.
    #[allow(dead_code)]
    pub fn with_metrics_db(mut self, path: PathBuf) -> Self {
        self.metrics_db = Some(path);
        self
    }
}

/// A spawned gateway/worker process handle.
///
/// Owns the child process, stdin/stdout/stderr handles, and provides
/// communication and health-checking methods. Automatically kills the
/// process and cleans up on Drop.
#[derive(Debug)]
pub struct GatewayHandle {
    /// The underlying child process.
    child: Child,
    /// Stdin writer.
    stdin: std::process::ChildStdin,
    /// Buffered stdout reader.
    stdout: BufReader<std::process::ChildStdout>,
    /// Allocated port (populated by portpicker).
    port: u16,
    /// The temp directory (kept alive for the duration of the handle).
    #[allow(dead_code)]
    _temp_dir: TempDir,
}

impl GatewayHandle {
    /// Send a JSON-RPC request and return the response.
    pub fn send_jsonrpc(&mut self, method: &str, params: Value) -> Result<Value, std::io::Error> {
        let id = rand::random::<u64>() % 1_000_000;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;

        // Read response (may have multiple lines for logging etc.)
        let mut response_line = String::new();
        for _ in 0..1000 {
            response_line.clear();
            if let Ok(n) = self.stdout.read_line(&mut response_line) {
                if n == 0 {
                    break;
                }
                if response_line.contains("\"id\":") || response_line.contains("\"jsonrpc\"") {
                    break;
                }
            }
        }

        serde_json::from_str(&response_line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Wait for a pattern to appear in stdout, with timeout.
    pub fn wait_for_output(&mut self, pattern: &str, timeout: Duration) -> Result<(), WaitError> {
        let deadline = std::time::Instant::now() + timeout;
        let mut buf = String::new();

        loop {
            if std::time::Instant::now() >= deadline {
                return Err(WaitError::Timeout {
                    pattern: pattern.to_string(),
                    elapsed: timeout,
                });
            }

            buf.clear();
            match self.stdout.read_line(&mut buf) {
                Ok(0) => return Err(WaitError::Eof),
                Ok(_) => {
                    if buf.contains(pattern) {
                        return Ok(());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => return Err(WaitError::Io(e)),
            }

            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait for the gateway to become healthy (probes health endpoint).
    #[allow(dead_code)]
    pub fn wait_for_health(&mut self, timeout: Duration) -> Result<(), WaitError> {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            if std::time::Instant::now() >= deadline {
                return Err(WaitError::HealthCheckTimeout(timeout));
            }

            // Try to send a health check request
            let response = self.send_jsonrpc(
                "tools/call",
                serde_json::json!({
                    "name": "sandbox_health",
                    "arguments": {}
                }),
            );

            if let Ok(resp) = response {
                if resp.get("result").is_some() {
                    return Ok(());
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Get the port this gateway is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Get a reference to the child process (for killing etc.).
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// Errors for wait operations.
#[derive(Debug)]
pub enum WaitError {
    /// Timed out waiting for a pattern in stdout.
    Timeout { pattern: String, elapsed: Duration },
    /// EOF reached while reading.
    Eof,
    /// Health check timed out.
    HealthCheckTimeout(Duration),
    /// IO error.
    Io(std::io::Error),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaitError::Timeout { pattern, elapsed } => {
                write!(
                    f,
                    "Timeout waiting for pattern '{}' after {:?}",
                    pattern, elapsed
                )
            }
            WaitError::Eof => write!(f, "EOF reached while reading stdout"),
            WaitError::HealthCheckTimeout(d) => {
                write!(f, "Health check timed out after {:?}", d)
            }
            WaitError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for WaitError {}

/// A test terminal that manages spawned processes and temp directories.
///
/// Provides isolated process spawning with automatic cleanup on Drop.
#[derive(Debug)]
pub struct TestTerminal {
    /// Temp directory that is cleaned up on Drop.
    temp_dir: TempDir,
}

impl TestTerminal {
    /// Create a new TestTerminal with an auto-cleanup temp directory.
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        Ok(Self { temp_dir })
    }

    /// Spawn a gateway process with the given config.
    ///
    /// Picks an unused port via `portpicker` and returns a `GatewayHandle`
    /// connected to the process's stdin/stdout.
    pub fn spawn_gateway(&mut self, config: GatewayConfig) -> std::io::Result<GatewayHandle> {
        let port = if config.port == 0 {
            portpicker::pick_unused_port().unwrap_or(0)
        } else {
            config.port
        };

        let mut cmd = Command::new(&config.binary_path);
        cmd.arg("--image").arg("debian:bookworm-slim");

        // Set up the worker binary path
        let worker_binary = config.binary_path.parent().unwrap().join("bastion-worker");
        if worker_binary.exists() {
            cmd.arg("--worker-binary").arg(&worker_binary);
        }

        // Port
        cmd.arg("--port").arg(port.to_string());

        // Log level
        if !config.log_level.is_empty() {
            cmd.arg("--log-level").arg(&config.log_level);
        }

        // Metrics database
        if let Some(ref db) = config.metrics_db {
            cmd.arg("--metrics-db").arg(db);
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        Ok(GatewayHandle {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            port,
            _temp_dir: tempfile::tempdir().expect("Failed to create temp dir for handle"),
        })
    }

    /// Spawn a worker process with the given port.
    #[allow(dead_code)]
    pub fn spawn_worker(&mut self, port: u16) -> std::io::Result<GatewayHandle> {
        let worker_binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/debug/bastion-worker");

        let mut cmd = Command::new(&worker_binary);
        cmd.arg("--port").arg(port.to_string());

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        Ok(GatewayHandle {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            port,
            _temp_dir: tempfile::tempdir().expect("Failed to create temp dir for handle"),
        })
    }

    /// Get the path to the temp directory.
    pub fn temp_path(&self) -> PathBuf {
        self.temp_dir.path().to_path_buf()
    }
}

impl Default for TestTerminal {
    fn default() -> Self {
        Self::new().expect("Failed to create temp directory")
    }
}

impl Drop for GatewayHandle {
    fn drop(&mut self) {
        // Send SIGTERM
        let _ = self.child.kill();
        // Wait for graceful shutdown (with timeout)
        let _ = self.child.wait_timeout(Duration::from_secs(5));
    }
}

impl Drop for TestTerminal {
    fn drop(&mut self) {
        // TempDir automatically cleaned up when dropped
    }
}

/// Extension trait to add timeout-aware wait.
trait WaitTimeout {
    fn wait_timeout(&mut self, timeout: Duration) -> std::io::Result<std::process::ExitStatus>;
}

impl WaitTimeout for std::process::Child {
    fn wait_timeout(&mut self, timeout: Duration) -> std::io::Result<std::process::ExitStatus> {
        use std::thread;
        use std::time::Instant;

        let deadline = Instant::now() + timeout;
        let sleep_step = Duration::from_millis(50);

        loop {
            match self.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => { /* still running */ }
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Process did not exit within timeout",
                ));
            }
            thread::sleep(sleep_step);
        }
    }
}
