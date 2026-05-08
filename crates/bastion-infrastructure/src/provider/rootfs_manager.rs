//! Default implementation of RootfsManager using tokio for async I/O.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use bastion_domain::provider::rootfs::RootfsManager;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

/// Default RootfsManager implementation.
///
/// Uses async tokio I/O for all filesystem operations.
#[derive(Debug, Default)]
pub struct DefaultRootfsManager;

impl DefaultRootfsManager {
    /// Create a new DefaultRootfsManager.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RootfsManager for DefaultRootfsManager {
    /// Prepare an OCI bundle for a sandbox.
    ///
    /// Steps:
    /// 1. Create bundle directory
    /// 2. Copy rootfs from rootfs_src to bundle_dir/rootfs
    /// 3. Inject worker binary into the rootfs
    /// 4. Write OCI config.json
    async fn prepare_oci_bundle(
        &self,
        sandbox_id: &SandboxId,
        bundle_dir: &Path,
        rootfs_src: &Path,
        worker_binary: &Path,
        env_vars: &HashMap<String, String>,
        entrypoint: &[String],
    ) -> Result<PathBuf, DomainError> {
        let rootfs_dest = bundle_dir.join("rootfs");

        // Step 1: Create bundle directory
        fs::create_dir_all(&rootfs_dest)
            .await
            .map_err(|e| DomainError::Internal(format!("Cannot create bundle directory: {e}")))?;

        // Step 2: Copy rootfs
        tracing::info!(
            source = %rootfs_src.display(),
            dest = %rootfs_dest.display(),
            "Copying rootfs for OCI bundle"
        );
        self.copy_dir(rootfs_src, &rootfs_dest).await?;

        // Step 3: Inject worker binary
        self.inject_worker(bundle_dir, worker_binary).await?;

        // Step 4: Write config.json
        let config = self.generate_oci_config(sandbox_id, env_vars, entrypoint)?;
        let config_path = bundle_dir.join("config.json");
        let config_str =
            serde_json::to_string_pretty(&config)
                .map_err(|e| DomainError::Internal(format!("Failed to serialize config.json: {e}")))?;
        fs::write(&config_path, config_str)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to write config.json: {e}")))?;

        tracing::info!(
            bundle = %bundle_dir.display(),
            "OCI bundle created"
        );
        Ok(bundle_dir.to_path_buf())
    }

    /// Recursively copy a directory using async tokio I/O.
    ///
    /// Symlinks are copied as regular files (target content is copied), not preserved as symlinks.
    async fn copy_dir(&self, src: &Path, dst: &Path) -> Result<(), DomainError> {
        let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];

        while let Some((current_src, current_dst)) = stack.pop() {
            let mut entries = fs::read_dir(&current_src)
                .await
                .map_err(|e| {
                    DomainError::Internal(format!("Cannot read directory {}: {e}", current_src.display()))
                })?;

            let mut dir_entry = entries.next_entry().await.map_err(|e| {
                DomainError::Internal(format!("Cannot read dir entry: {e}"))
            })?;

            while let Some(entry) = dir_entry {
                let ty = entry.file_type().await.map_err(|e| {
                    DomainError::Internal(format!("Cannot get file type: {e}"))
                })?;
                let src_path = entry.path();
                let dst_path = current_dst.join(entry.file_name());

                if ty.is_dir() {
                    fs::create_dir_all(&dst_path)
                        .await
                        .map_err(|e| {
                            DomainError::Internal(format!("Cannot create dir {}: {e}", dst_path.display()))
                        })?;
                    stack.push((src_path, dst_path));
                } else if ty.is_symlink() {
                    // Symlinks: read target and copy as regular file
                    let target = fs::read_link(&src_path)
                        .await
                        .map_err(|e| {
                            DomainError::Internal(format!("Cannot read symlink {}: {e}", src_path.display()))
                        })?;
                    // Copy the target content as a regular file
                    copy_file_async(&target, &dst_path).await?;
                } else {
                    copy_file_async(&src_path, &dst_path).await?;
                }

                dir_entry = entries.next_entry().await.map_err(|e| {
                    DomainError::Internal(format!("Cannot read dir entry: {e}"))
                })?;
            }
        }

        Ok(())
    }

    /// Inject the worker binary into a bundle rootfs.
    ///
    /// Copies the worker binary to `bundle_dir/rootfs/usr/local/bin/bastion-worker`
    /// and sets executable permissions.
    async fn inject_worker(&self, bundle_dir: &Path, worker_binary: &Path) -> Result<(), DomainError> {
        let rootfs = bundle_dir.join("rootfs");
        let worker_dest = rootfs.join("usr/local/bin/bastion-worker");

        // Create parent directory if it doesn't exist
        if let Some(parent) = worker_dest.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| {
                    DomainError::Internal(format!("Cannot create directory {}: {e}", parent.display()))
                })?;
        }

        // Copy the worker binary
        fs::copy(worker_binary, &worker_dest)
            .await
            .map_err(|e| {
                DomainError::Internal(format!("Failed to copy worker binary to rootfs: {e}"))
            })?;

        // Set executable permissions
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&worker_dest)
            .await
            .map_err(|e| {
                DomainError::Internal(format!("Failed to stat worker binary: {e}"))
            })?;
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs::set_permissions(&worker_dest, perms)
            .await
            .map_err(|e| {
                DomainError::Internal(format!("Failed to set executable permissions: {e}"))
            })?;

        // Create /workspace directory in rootfs
        let workspace = rootfs.join("workspace");
        fs::create_dir_all(&workspace).await.ok();

        tracing::debug!(
            bundle = %bundle_dir.display(),
            worker_dest = %worker_dest.display(),
            "Worker binary injected"
        );

        Ok(())
    }

    /// Generate OCI config.json for a bundle.
    ///
    /// Returns a minimal OCI runtime config similar to gVisor's config.
    fn generate_oci_config(
        &self,
        sandbox_id: &SandboxId,
        env_vars: &HashMap<String, String>,
        entrypoint: &[String],
    ) -> Result<serde_json::Value, DomainError> {
        // Build environment variables list
        let mut env_list: Vec<String> = vec![
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ];
        for (key, value) in env_vars {
            env_list.push(format!("{}={}", key, value));
        }

        // Default entrypoint if none provided
        let process_args = if entrypoint.is_empty() {
            vec!["sleep".to_string(), "999999".to_string()]
        } else {
            entrypoint.to_vec()
        };

        Ok(serde_json::json!({
            "ociVersion": "1.0.2",
            "process": {
                "terminal": false,
                "user": { "uid": 0, "gid": 0 },
                "args": process_args,
                "env": env_list,
                "cwd": "/",
                "capabilities": {
                    "bounding": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ],
                    "effective": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ],
                    "inheritable": [],
                    "permitted": [
                        "CAP_CHOWN", "CAP_DAC_OVERRIDE", "CAP_FSETID",
                        "CAP_FOWNER", "CAP_MKNOD", "CAP_NET_RAW",
                        "CAP_SETGID", "CAP_SETUID", "CAP_SETFCAP",
                        "CAP_SETPCAP", "CAP_NET_BIND_SERVICE",
                        "CAP_SYS_CHROOT", "CAP_KILL", "CAP_AUDIT_WRITE"
                    ]
                }
            },
            "root": {
                "path": "rootfs",
                "readonly": false
            },
            "hostname": format!("sandbox-{}", sandbox_id),
            "mounts": [
                {
                    "destination": "/proc",
                    "type": "proc",
                    "source": "proc"
                },
                {
                    "destination": "/dev",
                    "type": "tmpfs",
                    "source": "tmpfs",
                    "options": ["nosuid", "strictatime", "mode=755", "size=65536k"]
                },
                {
                    "destination": "/sys",
                    "type": "sysfs",
                    "source": "sysfs",
                    "options": ["nosuid", "noexec", "nodev"]
                }
            ],
            "linux": {
                // NOTE: network namespace removed — rootless runsc does not support
                // sandbox-level networking. Container uses host network (--network=host equivalent).
                "namespaces": [
                    { "type": "pid" },
                    { "type": "ipc" },
                    { "type": "uts" },
                    { "type": "mount" }
                ],
                "resources": {
                    "devices": [
                        {
                            "allow": false,
                            "access": "rwm"
                        }
                    ]
                }
            }
        }))
    }
}

/// Helper function to copy a file asynchronously.
async fn copy_file_async(src: &Path, dst: &Path) -> Result<(), DomainError> {
    fs::copy(src, dst)
        .await
        .map_err(|e| {
            DomainError::Internal(format!(
                "Cannot copy {} to {}: {e}",
                src.display(),
                dst.display()
            ))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_copy_dir_recursive() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        // Create source structure
        fs::create_dir_all(&src.join("subdir")).await.unwrap();
        fs::write(&src.join("file1.txt"), "content1").await.unwrap();
        fs::write(&src.join("subdir/file2.txt"), "content2").await.unwrap();

        let manager = DefaultRootfsManager::new();
        manager.copy_dir(&src, &dst).await.unwrap();

        // Verify
        assert!(dst.join("file1.txt").exists());
        assert!(dst.join("subdir/file2.txt").exists());
        assert_eq!(
            fs::read_to_string(&dst.join("file1.txt")).await.unwrap(),
            "content1"
        );
    }

    #[test]
    fn test_generate_oci_config() {
        let manager = DefaultRootfsManager::new();
        let sandbox_id = SandboxId::new("test-sandbox");
        let env_vars = HashMap::new();
        let entrypoint = vec!["/bin/sh".to_string(), "-c".to_string(), "echo hello".to_string()];

        let config = manager
            .generate_oci_config(&sandbox_id, &env_vars, &entrypoint)
            .unwrap();

        assert_eq!(config["ociVersion"], "1.0.2");
        assert_eq!(config["hostname"], "sandbox-test-sandbox");
        assert_eq!(config["process"]["args"], serde_json::json!(["/bin/sh", "-c", "echo hello"]));
    }
}
