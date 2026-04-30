//! Integration tests for Firecracker-based sandbox lifecycle.
//!
//! These tests require KVM (/dev/kvm), the firecracker binary,
//! a Linux kernel (vmlinux), and a root filesystem image.
//!
//! Run with: `cargo test --test firecracker_lifecycle -- --test-threads=1`
//!
//! Environment variables:
//!   FIRECRACKER_BIN — path to firecracker (default: ~/.local/bin/firecracker)
//!   KERNEL_PATH      — path to vmlinux (default: ~/.local/share/bastion/vmlinux.bin)
//!   ROOTFS_PATH      — path to rootfs (default: ~/.local/share/bastion/rootfs.ext4)

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
}

use std::sync::atomic::{AtomicU32, Ordering};

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

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
        .unwrap_or_else(|_| PathBuf::from("target/x86_64-unknown-linux-musl/release/bastion-worker"));

    // Unique directory per test to prevent interference
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let data_dir = PathBuf::from(format!("/tmp/bastion-fc-test-{}-{}", std::process::id(), count));
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

async fn create_sandbox(
    provider: &bastion_infrastructure::provider::FirecrackerProvider,
) -> (SandboxId, bastion_domain::sandbox::entity::Sandbox) {
    let sandbox_id = SandboxId::generate();
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
        .expect("Failed to create firecracker sandbox");
    (sandbox_id, sandbox)
}

#[tokio::test]
async fn test_firecracker_create_and_terminate() {
    let provider = provider();
    let (sandbox_id, sandbox) = create_sandbox(&provider).await;

    assert!(sandbox.is_active());
    assert_eq!(sandbox.id, sandbox_id);

    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed");
    assert!(alive, "Sandbox should be alive after creation");

    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");

    // Verify it's gone
    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed");
    assert!(!alive, "Sandbox should not be alive after termination");
}

#[tokio::test]
async fn test_firecracker_run_command() {
    let provider = provider();
    let (sandbox_id, _) = create_sandbox(&provider).await;

    // Run a simple echo command
    let cmd = CommandSpec::new("echo hello_from_firecracker");
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("Failed to run command");

    assert!(
        result.is_success(),
        "Command failed with exit code {}",
        result.exit_code
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("hello_from_firecracker"),
        "Expected 'hello_from_firecracker' in output, got: {}",
        stdout
    );

    // Run a command that should fail
    let cmd = CommandSpec::new("nonexistent_command_xyz");
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("Failed to run command");
    assert!(
        !result.is_success(),
        "Expected non-zero exit code for nonexistent command"
    );

    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn test_firecracker_write_and_read_file() {
    let provider = provider();
    let (sandbox_id, _) = create_sandbox(&provider).await;

    // Write a file
    let content = b"Hello from Bastion Firecracker!";
    provider
        .write_file(&sandbox_id, "/tmp/bastion-test.txt", content)
        .await
        .expect("Failed to write file");

    // Read it back
    let read_content = provider
        .read_file(&sandbox_id, "/tmp/bastion-test.txt")
        .await
        .expect("Failed to read file");

    assert_eq!(
        &read_content[..content.len()],
        content,
        "File content mismatch"
    );

    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn test_firecracker_list_files() {
    let provider = provider();
    let (sandbox_id, _) = create_sandbox(&provider).await;

    let entries = provider
        .list_files(&sandbox_id, "/")
        .await
        .expect("Failed to list files");

    assert!(!entries.is_empty(), "Root directory should have entries");
    assert!(
        entries
            .iter()
            .any(|e| e.path.contains("bin") || e.path.contains("etc") || e.path.contains("usr")),
        "Expected standard Linux directories, got: {:?}",
        entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn test_firecracker_multiple_vms() {
    let provider = provider();
    let (id1, s1) = create_sandbox(&provider).await;
    let (id2, s2) = create_sandbox(&provider).await;

    assert!(s1.is_active());
    assert!(s2.is_active());

    // Both should be independently alive
    assert!(provider.is_alive(&id1).await.unwrap());
    assert!(provider.is_alive(&id2).await.unwrap());

    // Terminate one, verify the other is still alive
    provider
        .terminate(&id1)
        .await
        .expect("Failed to terminate VM 1");
    assert!(!provider.is_alive(&id1).await.unwrap());
    assert!(provider.is_alive(&id2).await.unwrap());

    provider
        .terminate(&id2)
        .await
        .expect("Failed to terminate VM 2");
}
