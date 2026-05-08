//! Integration tests for gVisor (runsc)-based sandbox lifecycle.
//!
//! These tests require `runsc` installed and available in PATH.
//! Run with: `cargo test --test gvisor_lifecycle -- --test-threads=1`
//!
//! If runsc is not installed, tests are skipped automatically.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bastion_domain::execution::command::CommandSpec;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Find runsc binary in PATH.
fn find_runsc() -> Option<PathBuf> {
    std::env::var_os("RUNSC_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            // Search PATH
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths).find_map(|dir| {
                    let candidate = dir.join("runsc");
                    if candidate.exists() {
                        Some(candidate)
                    } else {
                        None
                    }
                })
            })
        })
}

/// Create a minimal OCI rootfs directory with busybox.
fn create_rootfs(dir: &std::path::Path) {
    let bins = [
        "bin",
        "usr/bin",
        "usr/local/bin",
        "tmp",
        "workspace",
        "etc",
        "dev",
        "proc",
        "root",
    ];
    for b in &bins {
        std::fs::create_dir_all(dir.join(b)).expect("Cannot create rootfs dir");
    }

    // Try to use busybox if available, otherwise copy a few binaries
    let busybox = std::process::Command::new("which")
        .arg("busybox")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !path.is_empty() {
                Some(std::path::PathBuf::from(path))
            } else {
                None
            }
        });
    if let Some(busybox_path) = busybox {
        std::fs::copy(&busybox_path, dir.join("bin/busybox")).expect("Cannot copy busybox");
        for cmd in &[
            "sh", "ls", "cat", "echo", "printf", "sleep", "mkdir", "chmod", "cp", "mv", "rm", "id",
            "pwd", "uname", "whoami",
        ] {
            let _ = std::os::unix::fs::symlink("/bin/busybox", dir.join("bin").join(cmd));
        }
    } else {
        // Fallback: copy key binaries from host
        for cmd in &[
            "sh", "ls", "cat", "echo", "printf", "sleep", "mkdir", "id", "pwd", "uname",
        ] {
            let found = std::process::Command::new("which")
                .arg(cmd)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !path.is_empty() {
                        Some(std::path::PathBuf::from(path))
                    } else {
                        None
                    }
                });
            if let Some(src) = found {
                let _ = std::fs::copy(&src, dir.join("bin").join(cmd));
            }
        }
    }
}

/// Helper to create a GVisorProvider for testing.
fn create_provider() -> bastion_infrastructure::provider::GVisorProvider {
    let runsc = find_runsc().expect("runsc not found in PATH. Install gVisor first.");

    // Create a unique temp rootfs for this test run
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let rootfs_dir = std::env::temp_dir().join(format!("bastion-gvisor-test-{}", count));
    if rootfs_dir.exists() {
        std::fs::remove_dir_all(&rootfs_dir).ok();
    }
    std::fs::create_dir_all(&rootfs_dir).expect("Cannot create test rootfs dir");

    // Create a minimal rootfs image (subdirectory under rootfs_dir)
    let image_dir = rootfs_dir.join("default");
    create_rootfs(&image_dir);

    // Create a dummy worker binary (provider validates it exists)
    let worker_bin = rootfs_dir.join("bastion-worker");
    std::fs::write(&worker_bin, b"#!/bin/sh\necho 'mock worker'\n").expect("Cannot write worker");
    std::fs::set_permissions(
        &worker_bin,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("Cannot set worker permissions");

    bastion_infrastructure::provider::GVisorProvider::new(
        runsc,
        "default",
        rootfs_dir,
        worker_bin,
        "10.0.2.1:50052".to_string(),
    )
    .expect("Failed to create GVisorProvider")
}

/// Clean up the rootfs directory after a test.
fn cleanup(rootfs_dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(rootfs_dir);
}

#[tokio::test]
async fn test_gvisor_create_and_terminate() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    let sandbox = provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create gVisor sandbox");

    assert_eq!(sandbox.id, sandbox_id);
    assert!(sandbox.is_active(), "Sandbox should be active after create");

    // Verify it's alive
    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive failed");
    assert!(alive, "Sandbox should be alive");

    // Terminate
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate sandbox");

    // Verify it's no longer alive
    let alive = provider
        .is_alive(&sandbox_id)
        .await
        .expect("is_alive after terminate failed");
    assert!(!alive, "Sandbox should not be alive after terminate");

    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_run_command() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run a simple command
    let cmd = CommandSpec::new("echo hello from gvisor");
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
        stdout.contains("hello from gvisor"),
        "Expected 'hello from gvisor' in output, got: '{}'",
        stdout.trim()
    );

    // Run another command to verify state
    let cmd2 = CommandSpec::new("id -u");
    let result2 = provider
        .run_command(&sandbox_id, &cmd2)
        .await
        .expect("Failed to run second command");
    assert!(result2.is_success(), "id command failed");
    let stdout2 = String::from_utf8_lossy(&result2.stdout);
    assert!(
        stdout2.contains("0"),
        "Expected root user (uid 0), got: '{}'",
        stdout2.trim()
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_write_and_read_file() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Write a file
    let content = b"Hello from Bastion on gVisor!";
    provider
        .write_file(&sandbox_id, "/tmp/bastion-test.txt", content)
        .await
        .expect("Failed to write file");

    // Read it back
    let read_content = provider
        .read_file(&sandbox_id, "/tmp/bastion-test.txt")
        .await
        .expect("Failed to read file");

    let expected = &content[..];
    let actual = &read_content[..read_content.len().min(content.len())];
    assert_eq!(
        actual,
        expected,
        "File content mismatch. Expected: {:?}, Got: {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(actual)
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_list_files() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // List root directory
    let entries = provider
        .list_files(&sandbox_id, "/")
        .await
        .expect("Failed to list files");

    assert!(!entries.is_empty(), "Root directory should have entries");
    assert!(
        entries.iter().any(|e| e.path.contains("bin")),
        "Expected 'bin' directory in root listing, got: {:?}",
        entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    // Verify workspace exists
    let ws_entries = provider
        .list_files(&sandbox_id, "/workspace")
        .await
        .expect("Failed to list /workspace");

    // Workspace should be empty (just created)
    assert!(
        ws_entries.is_empty(),
        "Workspace should be empty, got {} entries",
        ws_entries.len()
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_command_with_timeout() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Run a command that sleeps briefly and then completes
    let mut cmd = CommandSpec::new("sleep 1 && echo done");
    cmd.timeout_ms = Some(5000);
    let result = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("Command with sleep failed");

    assert!(result.is_success(), "Command should succeed");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("done"),
        "Expected 'done' in output: {}",
        stdout
    );

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_multiple_vms() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();

    // Create two independent sandboxes
    let id1 = SandboxId::generate();
    let id2 = SandboxId::generate();

    provider
        .create(
            &id1,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox 1");

    provider
        .create(
            &id2,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox 2");

    // Both should be alive
    assert!(
        provider.is_alive(&id1).await.unwrap_or(false),
        "Sandbox 1 should be alive"
    );
    assert!(
        provider.is_alive(&id2).await.unwrap_or(false),
        "Sandbox 2 should be alive"
    );

    // Run commands in both
    let cmd = CommandSpec::new("echo sandbox1");
    let r1 = provider
        .run_command(&id1, &cmd)
        .await
        .expect("Command in 1 failed");
    let stdout1 = String::from_utf8_lossy(&r1.stdout);
    assert!(
        stdout1.contains("sandbox1"),
        "Sandbox 1 output mismatch: {}",
        stdout1
    );

    let cmd2 = CommandSpec::new("echo sandbox2");
    let r2 = provider
        .run_command(&id2, &cmd2)
        .await
        .expect("Command in 2 failed");
    let stdout2 = String::from_utf8_lossy(&r2.stdout);
    assert!(
        stdout2.contains("sandbox2"),
        "Sandbox 2 output mismatch: {}",
        stdout2
    );

    // Terminate both
    provider
        .terminate(&id1)
        .await
        .expect("Failed to terminate 1");
    provider
        .terminate(&id2)
        .await
        .expect("Failed to terminate 2");

    // Both should be dead
    assert!(
        !provider.is_alive(&id1).await.unwrap_or(true),
        "Sandbox 1 should be dead"
    );
    assert!(
        !provider.is_alive(&id2).await.unwrap_or(true),
        "Sandbox 2 should be dead"
    );

    cleanup(&rootfs);
}

#[tokio::test]
async fn test_gvisor_streaming_command() {
    let provider = create_provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .expect("Failed to create sandbox");

    // Stream a command (falls back to non-streaming since no registry)
    let cmd = CommandSpec::new("echo streaming test");
    let result = provider.run_command_stream(&sandbox_id, &cmd).await;

    // Without registry, streaming returns UnsupportedOperation
    match result {
        Err(ref e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("requires a connected worker") || msg.contains("streaming"),
                "Expected streaming error message, got: {}",
                msg
            );
        }
        Ok(stream) => {
            // If it somehow worked (or if there's a fallback), verify output
            use futures::StreamExt;
            let chunks: Vec<_> = stream.collect().await;
            let stdout: String = chunks
                .iter()
                .filter_map(|c| c.as_ref().ok())
                .filter(|c| c.chunk_type == bastion_domain::execution::stream::ChunkType::Stdout)
                .flat_map(|c| String::from_utf8_lossy(&c.data).chars().collect::<Vec<_>>())
                .collect();
            assert!(
                stdout.contains("streaming test"),
                "Expected streaming output, got: {}",
                stdout
            );
        }
    }

    // Cleanup
    provider
        .terminate(&sandbox_id)
        .await
        .expect("Failed to terminate");
    cleanup(&rootfs);
}
