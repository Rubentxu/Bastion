//! Integration tests for Podman-based sandbox lifecycle.
//!
//! These tests require a running Podman daemon.
//! Run with: `cargo test --test podman_lifecycle -- --test-threads=1`

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;

/// Helper to create a PodmanProvider connected to the local daemon.
fn create_provider() -> bastion_infrastructure::provider::PodmanProvider {
    // Find worker binary relative to workspace root
    let worker_bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-worker");

    bastion_infrastructure::provider::PodmanProvider::new(
        "/run/user/1000/podman/podman.sock",
        "debian:bookworm-slim",
        worker_bin,
    )
    .expect("Failed to connect to Podman. Is Podman running?")
}

#[tokio::test]
async fn test_podman_ping() {
    let provider = create_provider();
    let result = provider.ping().await;
    assert!(result.is_ok(), "Podman ping failed: {:?}", result.err());
}

#[tokio::test]
async fn test_sandbox_create_and_terminate() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    let sandbox = provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    assert_eq!(sandbox.id, sandbox_id);
    assert!(sandbox.is_active());

    // Verify it's alive
    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed");
    assert!(alive);

    // Terminate
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");
}

#[tokio::test]
async fn test_sandbox_run_command() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
    provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run a simple command
    let cmd = CommandSpec::new("echo hello world");
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
        stdout.contains("hello world"),
        "Expected 'hello world' in output, got: {}",
        stdout
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn test_sandbox_write_and_read_file() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Write a file
    let content = b"Hello from Bastion!";
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

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn test_sandbox_list_files() {
    let provider = create_provider();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    let entries = provider
        .list_files(&sandbox_id, "/")
        .await
        .expect("Failed to list files");

    assert!(!entries.is_empty(), "Root directory should have entries");
    assert!(
        entries
            .iter()
            .any(|e| e.path.contains("bin") || e.path.contains("etc")),
        "Expected standard Linux directories, got: {:?}",
        entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}
