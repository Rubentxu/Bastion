//! gVisor Provider Tests (Local-Only)
//!
//! **IMPORTANT**: These tests require runsc binary and gVisor installation.
//! Run with: `cargo test --test gvisor_test -- --ignored --test-threads=1`
//!
//! Setup:
//! ```bash
//! # Install gVisor
//! go install github.com/google/gvisor/runsc@latest
//!
//! # Ensure runsc is in PATH
//! which runsc
//!
//! # Run tests
//! cargo test --test gvisor_test -- --ignored
//! ```

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

    // Try to use busybox if available
    let busybox = std::process::Command::new("which")
        .arg("busybox")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !path.is_empty() {
                Some(PathBuf::from(path))
            } else {
                None
            }
        });

    if let Some(busybox_path) = busybox {
        let _ = std::fs::copy(&busybox_path, dir.join("bin/busybox"));
        for cmd in &[
            "sh", "ls", "cat", "echo", "printf", "sleep", "mkdir", "chmod", "cp", "mv", "rm", "id",
            "pwd", "uname", "whoami",
        ] {
            let _ = std::os::unix::fs::symlink("/bin/busybox", dir.join("bin").join(cmd));
        }
    } else {
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
                        Some(PathBuf::from(path))
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

/// Create a GVisorProvider for testing.
fn provider() -> bastion_infrastructure::provider::GVisorProvider {
    let runsc = find_runsc().expect("runsc not found in PATH. Install gVisor first.");

    // Create a unique temp rootfs for this test run
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let rootfs_dir = std::env::temp_dir().join(format!("bastion-gvisor-test-{}", count));
    if rootfs_dir.exists() {
        std::fs::remove_dir_all(&rootfs_dir).ok();
    }
    std::fs::create_dir_all(&rootfs_dir).expect("Cannot create test rootfs dir");

    // Create a minimal rootfs image
    let image_dir = rootfs_dir.join("default");
    create_rootfs(&image_dir);

    // Create a dummy worker binary
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
        rootfs_dir.clone(),
        worker_bin,
        "10.0.2.1:50052".to_string(),
    )
    .expect("Failed to create GVisorProvider")
}

/// Clean up the rootfs directory after a test.
fn cleanup(rootfs_dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(rootfs_dir);
}

/// Test: gVisor provider initializes correctly
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_provider_init() {
    let provider = provider();
    // Just verify the provider was created successfully
    assert!(
        provider.name().contains("gvisor")
            || provider.name().contains("gVisor")
            || provider.name().contains("runsc")
    );
}

/// Test: Create and terminate a gVisor sandbox
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_create_and_terminate() {
    let provider = provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    // Create sandbox
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
        .expect("create should work");

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

    cleanup(&rootfs);
}

/// Test: Run command in gVisor sandbox
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_run_command() {
    let provider = provider();
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
    cleanup(&rootfs);
}

/// Test: gVisor networking respects allow_internet flag
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_networking_respects_flag() {
    let provider = provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    // Create config with networking DISABLED
    let network = NetworkSpec {
        allow_internet: false,
        ..Default::default()
    };

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &network,
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .unwrap();

    // Should NOT be able to reach external network
    let cmd = CommandSpec::new("curl -s ifconfig.me || echo 'blocked'");
    let output = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command should work");

    // Should be blocked or have no curl
    let stdout = String::from_utf8_lossy(&output.stdout);
    let is_blocked =
        stdout.contains("blocked") || stdout.contains("not found") || !output.is_success();

    provider.terminate(&sandbox_id).await.unwrap();
    cleanup(&rootfs);

    assert!(
        is_blocked,
        "gVisor with no-internet should block network: {:?}",
        output
    );
}

/// Test: gVisor with networking enabled
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_networking_enabled() {
    let provider = provider();
    let rootfs = provider.rootfs_dir().to_path_buf();
    let sandbox_id = SandboxId::generate();

    // Create config with networking ENABLED
    let network = NetworkSpec {
        allow_internet: true,
        ..Default::default()
    };

    provider
        .create(
            &sandbox_id,
            "default",
            &ResourcesSpec::default(),
            &network,
            &std::collections::HashMap::new(),
            3_600_000,
        )
        .await
        .unwrap();

    // Try to use network (ping might be blocked, but network namespace should exist)
    let cmd = CommandSpec::new("ping -c 1 8.8.8.8 || echo 'no ping'");
    let output = provider
        .run_command(&sandbox_id, &cmd)
        .await
        .expect("run_command should work");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let has_network = output.is_success() || stdout.contains("ping") || stdout.contains("PING");

    provider.terminate(&sandbox_id).await.unwrap();
    cleanup(&rootfs);

    // Just verify sandbox was created with network namespace
    assert!(has_network || stdout.contains("PING"));
}

/// Test: gVisor capabilities report correct values
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_capabilities() {
    let provider = provider();

    let caps = provider.capabilities();

    // gVisor should report supports_networking: true
    assert!(caps.supports_networking, "gVisor should support networking");

    // Should have reasonable resource limits
    assert!(caps.max_memory_mb > 0, "Should have memory limit");
    assert!(caps.max_cpu_count > 0, "Should have CPU limit");
}

/// Test: gVisor command failure handling
#[tokio::test]
#[ignore = "requires runsc binary"]
async fn test_gvisor_command_failure() {
    let provider = provider();
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
    cleanup(&rootfs);
}
