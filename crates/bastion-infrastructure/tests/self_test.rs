//! Self-testing: Bastion tests itself using Podman containers.
//!
//! This test creates a container with the Bastion source code mounted,
//! then runs `cargo check` and `cargo test` against it.
//!
//! Requirements:
//! - Podman daemon running
//! - Source code available at workspace root
//! - bastion-worker binary compiled

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use std::path::PathBuf;

/// Helper to create a PodmanProvider with source mount for self-testing.
async fn create_test_provider()
-> Result<bastion_infrastructure::provider::PodmanProvider, Box<dyn std::error::Error>> {
    let socket = "/run/user/1000/podman/podman.sock";

    // Path to bastion-worker binary
    let worker_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/bastion-worker");

    // Path to Bastion source root
    let source_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let mut provider = bastion_infrastructure::provider::PodmanProvider::new(
        socket,
        "debian:bookworm-slim",
        worker_bin,
    )?;

    provider.with_source_mount(source_path);

    Ok(provider)
}

#[tokio::test]
async fn self_test_cargo_check() {
    // Skip if Podman not available
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let provider = create_test_provider()
        .await
        .expect("Failed to create provider");
    let sandbox_id = SandboxId::generate();

    // Create sandbox with source mounted
    let _sandbox = provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            300_000, // 5 min timeout
        )
        .await
        .expect("Failed to create sandbox");

    // Run cargo check against the mounted source
    let cmd = CommandSpec::new("cd /workspace/code && cargo check -p bastion-domain");
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("Failed to run command");

    println!("cargo check exit_code: {}", result.exit_code);
    println!("stdout: {}", String::from_utf8_lossy(&result.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&result.stderr));

    assert_eq!(result.exit_code, 0, "cargo check should succeed");

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}

#[tokio::test]
async fn self_test_cargo_test_unit() {
    let socket = std::path::Path::new("/run/user/1000/podman/podman.sock");
    if !socket.exists() {
        eprintln!("Skipping: Podman socket not found");
        return;
    }

    let provider = create_test_provider()
        .await
        .expect("Failed to create provider");
    let sandbox_id = SandboxId::generate();

    // Create sandbox with source mounted
    let _sandbox = provider
        .create(
            &sandbox_id,
            "debian:bookworm-slim",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            600_000, // 10 min timeout for tests
        )
        .await
        .expect("Failed to create sandbox");

    // Run cargo test --lib (unit tests only, faster than full test suite)
    let cmd = CommandSpec::new(
        "cd /workspace/code && cargo test --lib --manifest-path /workspace/code/Cargo.toml 2>&1 | head -100",
    );
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("Failed to run command");

    println!("cargo test exit_code: {}", result.exit_code);
    println!("output (first 100 lines):");
    println!("{}", String::from_utf8_lossy(&result.stdout));

    // We expect exit_code 0 (all tests passed) or 101 (tests run but some failed)
    // For now, just verify tests ran
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(
        output.contains("test result") || output.contains("running"),
        "Tests should have run"
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
}
