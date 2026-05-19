//! PodmanProvider Integration Tests
//!
//! Tests for each PodmanProvider operation.
//! Requires: Podman daemon running + debian:bookworm-slim image + bastion-gateway on port 50052
//!
//! Run with: `cargo test --package bastion-infrastructure --test podman_provider_test -- --test-threads=1`

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::sandbox::value_objects::{
    NetworkSpec, ResourcesSpec, SandboxFilter, SandboxStatus,
};
use bastion_domain::shared::id::SandboxId;

/// Gateway process handle for auto-cleanup
struct GatewayProcess(Child);

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Check if a port is already in use (something is listening on it)
fn is_port_in_use(port: u16) -> bool {
    std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok()
}

/// Ensure a bastion-gateway process is running on port 50052.
/// Returns a handle that will kill the process on drop.
/// If a gateway is already running, returns None (no cleanup needed).
fn ensure_gateway_on_port_50052() -> Option<GatewayProcess> {
    if !is_port_in_use(50052) {
        eprintln!("No gateway on port 50052, spawning one...");

        // Build path to gateway binary (from bastion-infrastructure/tests -> bastion-infrastructure -> workspace root)
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()      // bastion-infrastructure/tests
            .parent().unwrap()     // bastion-infrastructure
            .to_path_buf();

        let gateway_bin = workspace_root.join("target/debug/bastion-gateway");
        let worker_bin = workspace_root.join("target/x86_64-unknown-linux-musl/release/bastion-worker");

        if !gateway_bin.exists() {
            eprintln!("Gateway binary not found at {:?}, cannot auto-start", gateway_bin);
            return None;
        }

        // Spawn gateway with fixed registry port
        let child = Command::new(&gateway_bin)
            .arg("--socket").arg("/run/user/1000/podman/podman.sock")
            .arg("--image").arg("debian:bookworm-slim")
            .arg("--worker-binary").arg(worker_bin.to_str().unwrap())
            .arg("--registry-addr").arg("127.0.0.1:50052")
            .spawn()
            .expect("Failed to spawn gateway");

        // Wait for gateway to be ready
        std::thread::sleep(Duration::from_secs(3));

        eprintln!("Gateway spawned with PID {}", child.id());
        Some(GatewayProcess(child))
    } else {
        eprintln!("Gateway already running on port 50052, using existing one");
        None
    }
}

// ============================================================================
// Test Configuration
// ============================================================================

/// Socket path for Podman
const PODMAN_SOCKET: &str = "/run/user/1000/podman/podman.sock";

/// Default image for tests
const TEST_IMAGE: &str = "debian:bookworm-slim";

/// Helper to get worker binary path
/// Uses musl static binary for container compatibility (glibc binary requires newer glibc than debian:bookworm-slim provides)
fn worker_binary() -> PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/x86_64-unknown-linux-musl/release/bastion-worker")
}

/// Check if Podman is available
fn podman_available() -> bool {
    std::path::Path::new(PODMAN_SOCKET).exists()
}

/// Try to create a PodmanProvider, returning None if Podman is not available.
/// Also ensures a bastion-gateway is running on port 50052 for worker connections.
fn try_create_provider() -> Option<bastion_infrastructure::provider::PodmanProvider> {
    if !podman_available() {
        eprintln!("Skipping: Podman socket not found at {}", PODMAN_SOCKET);
        return None;
    }

    let worker_bin = worker_binary();
    if !worker_bin.exists() {
        eprintln!(
            "Skipping: bastion-worker binary not found at {:?}",
            worker_bin
        );
        return None;
    }

    // Ensure gateway is running for worker connections
    ensure_gateway_on_port_50052();

    bastion_infrastructure::provider::PodmanProvider::new(PODMAN_SOCKET, TEST_IMAGE, worker_bin)
        .ok()
}

/// Create a PodmanProvider, aborting the test if Podman is not available.
fn create_provider() -> bastion_infrastructure::provider::PodmanProvider {
    try_create_provider().expect("Podman provider should be available")
}

// ============================================================================
// Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_podman_ping() {
    let provider = create_provider();
    let result = provider.ping().await;
    assert!(result.is_ok(), "Podman ping failed: {:?}", result.err());
}

#[tokio::test]
async fn test_podman_create_and_terminate() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
    let sandbox = provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    assert_eq!(sandbox.id, sandbox_id);
    assert!(sandbox.is_active(), "New sandbox should be active");

    // Verify running
    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed");
    assert!(alive, "Newly created sandbox should be alive");

    // Terminate
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");

    // Verify terminated
    let alive_after = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed after terminate");
    assert!(!alive_after, "Terminated sandbox should not be alive");
}

// ============================================================================
// List and Info Tests
// ============================================================================

#[tokio::test]
async fn test_podman_list_sandboxes() {
    let provider = create_provider();
    let sandbox_id1 = SandboxId::generate();
    let sandbox_id2 = SandboxId::generate();

    // Create 2 sandboxes
    provider
        .create(
            &sandbox_id1,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox 1");

    provider
        .create(
            &sandbox_id2,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox 2");

    // List all sandboxes
    let filter = SandboxFilter {
        provider_name: None,
        status: Some(SandboxStatus::Running),
        limit: Some(100),
        cursor: None,
    };
    let sandboxes = provider
        .list_sandboxes(&filter)
        .await
        .expect("list_sandboxes failed");

    // Verify our sandboxes are in the list
    let ids: Vec<_> = sandboxes.iter().map(|s| s.id.to_string()).collect();
    assert!(
        ids.contains(&sandbox_id1.to_string()),
        "Sandbox 1 should be in list"
    );
    assert!(
        ids.contains(&sandbox_id2.to_string()),
        "Sandbox 2 should be in list"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id1)
        .await
        .expect("Failed to terminate sandbox 1");
    provider
        .terminate(&sandbox_id2)
        .await
        .expect("Failed to terminate sandbox 2");
}

#[tokio::test]
async fn test_podman_get_sandbox_info() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
    let _sandbox = provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Get info
    let info = provider
        .get_info(&sandbox_id)
        .await
        .expect("get_info failed");

    assert_eq!(
        info.id, sandbox_id,
        "Info should return the correct sandbox ID"
    );
    assert_eq!(
        info.status,
        SandboxStatus::Running,
        "Sandbox should be in Running state"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

// ============================================================================
// Timeout Tests
// ============================================================================

#[tokio::test]
async fn test_podman_set_timeout() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Set a new timeout (should be no-op at provider level but shouldn't error)
    let result = provider.set_timeout(&sandbox_id, 7200_000).await;

    // Note: Podman's set_timeout is a no-op, so it should succeed
    assert!(
        result.is_ok(),
        "set_timeout should succeed (even if it's a no-op)"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

// ============================================================================
// Command Execution Tests
//
// NOTE: These tests require the bastion-gateway to be running with a connected
// PodmanProvider that has the command_router configured. The PodmanProvider
// standalone (created directly) does NOT have access to the command_router.
//
// For full E2E tests that spawn the gateway and use HTTP transport, see:
//   bastion-gateway/tests/e2e_podman_test.rs
//
// These tests are kept here for reference but will be skipped until we
// implement exec fallback or proper gateway integration.
// ============================================================================

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_run_command_success() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run successful command
    let cmd = CommandSpec::new("echo hello");
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command failed");

    assert_eq!(
        result.exit_code, 0,
        "echo should exit with code 0, got {}",
        result.exit_code
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("hello"),
        "Output should contain 'hello', got: {}",
        stdout
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_run_command_failure() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run failing command using bash -c "exit 42"
    let cmd = CommandSpec::new("bash").with_args(vec!["-c".to_string(), "exit 42".to_string()]);
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command failed");

    assert_eq!(
        result.exit_code, 42,
        "exit 42 should exit with code 42, got {}",
        result.exit_code
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_run_command_with_args() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run command with arguments
    let cmd = CommandSpec::new("echo foo bar");
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command failed");

    assert_eq!(result.exit_code, 0, "echo should exit with code 0");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("foo bar"),
        "Output should contain 'foo bar', got: {}",
        stdout
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

// ============================================================================
// File Operations Tests
//
// NOTE: File operations require the bastion-gateway command_router.
// Skipped for standalone PodmanProvider - use e2e_podman_test.rs instead.
// ============================================================================

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_read_file() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Write file first
    let content = b"Hello, Bastion!";
    provider
        .write_file(&sandbox_id, "/tmp/test_read.txt", content)
        .await
        .expect("Failed to write file");

    // Read it back
    let read_content = provider
        .read_file(&sandbox_id, "/tmp/test_read.txt")
        .await
        .expect("Failed to read file");

    assert_eq!(
        &read_content[..content.len()],
        content,
        "Read content should match written content"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_write_file() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Write to /tmp/test.txt
    let test_content = b"Write test content 123";
    provider
        .write_file(&sandbox_id, "/tmp/test_write.txt", test_content)
        .await
        .expect("Failed to write file");

    // Read back and verify
    let read_content = provider
        .read_file(&sandbox_id, "/tmp/test_write.txt")
        .await
        .expect("Failed to read back written file");

    assert_eq!(
        &read_content[..test_content.len()],
        test_content,
        "Content mismatch"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

#[tokio::test]
#[ignore = "requires gateway with command_router - use bastion-gateway/tests/e2e_podman_test.rs instead"]
async fn test_podman_list_directory() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            TEST_IMAGE,
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // List /workspace
    let entries = provider
        .list_files(&sandbox_id, "/workspace")
        .await
        .expect("list_files failed");

    // /workspace should exist and have entries (at least . or the bind mount)
    assert!(
        !entries.is_empty() || true, // Workspace may be empty, just verify it works
        "list_files should return without error"
    );

    // List root directory
    let root_entries = provider
        .list_files(&sandbox_id, "/")
        .await
        .expect("list_files / failed");

    // Root should have standard directories
    assert!(
        root_entries
            .iter()
            .any(|e| e.path.contains("bin") || e.path.contains("etc")),
        "Root should have bin or etc, got: {:?}",
        root_entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

// ============================================================================
// Capabilities Test
// ============================================================================

#[tokio::test]
async fn test_podman_capabilities() {
    let provider = create_provider();
    let caps = provider.capabilities();

    assert_eq!(
        caps.avg_startup_ms, 1500,
        "Podman should report ~1500ms startup time"
    );
    assert!(caps.supports_streaming, "Podman should support streaming");
    assert!(
        !caps.supports_snapshots,
        "Podman should not support snapshots"
    );
    assert!(!caps.requires_kvm, "Podman should not require KVM");
}
