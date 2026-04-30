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

use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
}

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

    let data_dir = PathBuf::from("/tmp/bastion-firecracker-test");
    std::fs::create_dir_all(&data_dir).ok();

    bastion_infrastructure::provider::FirecrackerProvider::new(fc_bin, kernel, rootfs, data_dir)
        .expect("Failed to create FirecrackerProvider")
}

#[tokio::test]
async fn test_firecracker_create_and_terminate() {
    let provider = provider();
    let sandbox_id = SandboxId::generate();

    let sandbox = provider
        .create(
            &sandbox_id,
            "", // no template, use default rootfs
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            60_000, // 1 minute
        )
        .await
        .expect("Failed to create firecracker sandbox");

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
async fn test_firecracker_multiple_vms() {
    let provider = provider();

    let id1 = SandboxId::generate();
    let id2 = SandboxId::generate();

    let s1 = provider
        .create(
            &id1,
            "",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            60_000,
        )
        .await
        .expect("Failed to create VM 1");

    let s2 = provider
        .create(
            &id2,
            "",
            &ResourcesSpec::default(),
            &NetworkSpec::default(),
            &std::collections::HashMap::new(),
            60_000,
        )
        .await
        .expect("Failed to create VM 2");

    assert!(s1.is_active());
    assert!(s2.is_active());

    // Both should be independently alive
    assert!(provider.is_alive(&id1).await.unwrap());
    assert!(provider.is_alive(&id2).await.unwrap());

    // Terminate one, verify the other is still alive
    provider.terminate(&id1).await.expect("Failed to terminate VM 1");
    assert!(!provider.is_alive(&id1).await.unwrap());
    assert!(provider.is_alive(&id2).await.unwrap());

    provider.terminate(&id2).await.expect("Failed to terminate VM 2");
}
