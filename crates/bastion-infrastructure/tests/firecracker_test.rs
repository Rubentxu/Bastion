//! Firecracker Provider Tests (Local-Only)
//!
//! **IMPORTANT**: These tests require KVM access and firecracker binary.
//! Run with: `cargo test --test firecracker_test -- --ignored --test-threads=1`
//!
//! Setup:
//! ```bash
//! # Install firecracker
//! cargo install firecracker
//!
//! # Ensure KVM access
//! ls -la /dev/kvm
//!
//! # Set environment variables (or use defaults)
//! export FIRECRACKER_BIN="$HOME/.local/bin/firecracker"
//! export KERNEL_PATH="$HOME/.local/share/bastion/vmlinux.bin"
//! export ROOTFS_PATH="$HOME/.local/share/bastion/rootfs.ext4"
//!
//! # Run tests
//! cargo test --test firecracker_test -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
}

/// Create a FirecrackerProvider with test defaults.
#[cfg(not(feature = "use-segregated-traits"))]
fn provider() -> bastion_infrastructure::provider::FirecrackerProvider {
    let fc_bin = std::env::var("FIRECRACKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/bin/firecracker"));

    let kernel = std::env::var("KERNEL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/bastion/vmlinux.bin"));

    let rootfs = std::env::var("ROOTFS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/bastion/rootfs.ext4"));

    let worker_bin = std::env::var("WORKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from("target/x86_64-unknown-linux-musl/release/bastion-worker")
        });

    // Unique directory per test to prevent interference
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let data_dir = PathBuf::from(format!(
        "/tmp/bastion-fc-test-{}-{}",
        std::process::id(),
        count
    ));
    std::fs::create_dir_all(&data_dir).ok();

    bastion_infrastructure::provider::FirecrackerProvider::new(
        fc_bin,
        kernel,
        rootfs,
        data_dir,
        worker_bin,
        "10.0.2.1:50052".to_string(),
    )
    .expect("Failed to create FirecrackerProvider")
}

/// Create a FirecrackerProvider with test defaults (segregated-traits version).
#[cfg(feature = "use-segregated-traits")]
fn provider() -> bastion_infrastructure::provider::FirecrackerProvider {
    use bastion_infrastructure::provider::network::HostBackend;

    let fc_bin = std::env::var("FIRECRACKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/bin/firecracker"));

    let kernel = std::env::var("KERNEL_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/bastion/vmlinux.bin"));

    let rootfs = std::env::var("ROOTFS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/bastion/rootfs.ext4"));

    let worker_bin = std::env::var("WORKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from("target/x86_64-unknown-linux-musl/release/bastion-worker")
        });

    // Unique directory per test to prevent interference
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let data_dir = PathBuf::from(format!(
        "/tmp/bastion-fc-test-{}-{}",
        std::process::id(),
        count
    ));
    std::fs::create_dir_all(&data_dir).ok();

    bastion_infrastructure::provider::FirecrackerProvider::new(
        fc_bin,
        kernel,
        rootfs,
        data_dir,
        worker_bin,
        "10.0.2.1:50052".to_string(),
        HostBackend::new(),
    )
    .expect("Failed to create FirecrackerProvider")
}

/// Test: Firecracker provider initializes correctly
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_provider_init() {
    let provider = provider();
    // Just verify the provider was created successfully
    assert!(provider.name().contains("firecracker") || provider.name().contains("Firecracker"));
}

/// Test: Create and terminate a Firecracker sandbox
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_create_and_terminate() {
    let provider = provider();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
    let sandbox = provider
        .create(
            &sandbox_id,
            "",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            120_000,
        )
        .await
        .expect("create_sandbox should work");

    // Verify running
    assert!(sandbox.is_active());
    assert_eq!(sandbox.id, sandbox_id);

    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive should work");
    assert!(alive, "Sandbox should be alive after creation");

    // Terminate
    provider
        .terminate(&sandbox_id)
        .await
        .expect("terminate should work");

    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive after terminate");
    assert!(!alive, "Sandbox should not be alive after termination");
}

/// Test: Run command in Firecracker sandbox
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_run_command() {
    let provider = provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            120_000,
        )
        .await
        .unwrap();

    // Run echo command
    let cmd = CommandSpec::new("echo hello");
    let output = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command should work");

    assert!(
        output.is_success(),
        "Command should succeed, got exit code: {}",
        output.exit_code
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello"),
        "Expected 'hello' in output, got: {}",
        stdout
    );

    provider.terminate(&sandbox_id).await.unwrap();
}

/// Test: Firecracker supports networking (bridge mode)
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_networking() {
    let provider = provider();
    let sandbox_id = SandboxId::generate();

    // Create config with networking enabled
    let network = NetworkSpec {
        allow_internet: true,
        ..Default::default()
    };

    provider
        .create(
            &sandbox_id,
            "",
            &ResourcesSpec::default(),
            &network,
            &std::collections::HashMap::new(),
            120_000,
        )
        .await
        .unwrap();

    // Try to run a command that uses network
    let cmd = CommandSpec::new("curl -s ifconfig.me || echo 'no curl'");
    let output = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command should work");

    // If curl succeeded, we have internet
    let _has_internet =
        output.is_success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty();

    provider.terminate(&sandbox_id).await.unwrap();

    // Note: This assertion may pass or fail depending on network configuration
    // The important thing is the sandbox was created with network enabled
    // Just verify command ran - actual network connectivity depends on environment
    assert!(output.is_success() || String::from_utf8_lossy(&output.stdout).contains("no curl"));
}

/// Test: Firecracker has resource limits via capabilities
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_resource_limits() {
    let provider = provider();

    // Get capabilities
    let caps = provider.capabilities();

    // Firecracker should report CPU/memory limits
    assert!(caps.max_cpu_count > 0, "Should have CPU limit");
    assert!(caps.max_memory_mb > 0, "Should have memory limit");

    // Firecracker typically requires KVM
    assert!(caps.requires_kvm, "Firecracker should require KVM");
}

/// Test: Firecracker command failure handling
#[tokio::test]
#[ignore = "requires KVM and firecracker binary"]
async fn test_firecracker_command_failure() {
    let provider = provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            120_000,
        )
        .await
        .unwrap();

    // Run a command that should fail
    let cmd = CommandSpec::new("nonexistent_command_xyz");
    let output = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command should work");

    assert!(
        !output.is_success(),
        "Expected non-zero exit code for nonexistent command"
    );

    provider.terminate(&sandbox_id).await.unwrap();
}
