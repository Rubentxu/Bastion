//! Rootfs manager trait — manages root filesystem operations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(feature = "use-segregated-traits")]
use crate::shared::DomainError;
#[cfg(feature = "use-segregated-traits")]
use crate::shared::id::SandboxId;

/// Trait for managing OCI bundle root filesystems.
///
/// Provides operations for preparing sandboxes: copying rootfs images,
/// injecting worker binaries, and generating OCI runtime configuration.
#[cfg(feature = "use-segregated-traits")]
#[async_trait::async_trait]
pub trait RootfsManager: Send + Sync + std::fmt::Debug {
    /// Prepare an OCI bundle for a sandbox.
    ///
    /// Creates a bundle directory at `bundle_dir`, copies the rootfs from `rootfs_src`,
    /// injects the worker binary, and writes the OCI config.json.
    ///
    /// Returns the path to the bundle directory.
    async fn prepare_oci_bundle(
        &self,
        sandbox_id: &SandboxId,
        bundle_dir: &Path,
        rootfs_src: &Path,
        worker_binary: &Path,
        env_vars: &HashMap<String, String>,
        entrypoint: &[String],
    ) -> Result<PathBuf, DomainError>;

    /// Recursively copy a directory.
    ///
    /// Symlinks are copied as regular files (target content), not preserved as symlinks.
    async fn copy_dir(&self, src: &Path, dst: &Path) -> Result<(), DomainError>;

    /// Inject the worker binary into a bundle rootfs.
    ///
    /// Copies the worker binary to `bundle_dir/rootfs/usr/local/bin/` and sets executable permissions.
    async fn inject_worker(&self, bundle_dir: &Path, worker_binary: &Path) -> Result<(), DomainError>;

    /// Generate OCI config.json for a bundle.
    ///
    /// Returns a JSON value representing a minimal OCI runtime config.
    fn generate_oci_config(
        &self,
        sandbox_id: &SandboxId,
        env_vars: &HashMap<String, String>,
        entrypoint: &[String],
    ) -> Result<serde_json::Value, DomainError>;
}
