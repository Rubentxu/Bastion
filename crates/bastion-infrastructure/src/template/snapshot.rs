//! Snapshot management — create and restore sandbox snapshots.
//!
//! Works with any SandboxProvider by using shell commands for
//! container commit/restore operations.

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;
use bastion_domain::template::ProviderKind;

/// Creates and restores Snapshots using podman/docker commands.
pub struct SnapshotManager {
    #[allow(dead_code)]
    provider_kind: ProviderKind,
}

impl SnapshotManager {
    pub fn new(provider_kind: ProviderKind) -> Self {
        Self { provider_kind }
    }

    /// Create a snapshot of a running sandbox.
    pub async fn create_snapshot(
        &self,
        sandbox_id: &SandboxId,
        name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        let container_name = sandbox_id.to_string();
        let image_tag = format!("bastion-snap-{}:latest", name.replace('/', "-"));

        tracing::info!(sandbox_id = %sandbox_id, name, "Creating snapshot");

        // podman commit <container> <image>:<tag>
        let output = tokio::process::Command::new("podman")
            .args(["commit", &container_name, &image_tag])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman commit: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to create snapshot: {}",
                stderr
            )));
        }

        let now = chrono::Utc::now();
        let snapshot_id = format!("snap:{}-{}", name.replace('/', "-"), now.timestamp());

        tracing::info!(snapshot_id = %snapshot_id, "Snapshot created");
        Ok(SnapshotInfo {
            snapshot_id,
            sandbox_id: sandbox_id.to_string(),
            name: name.to_string(),
            created_at: now,
            size_bytes: 0, // approximate, not critical
        })
    }

    /// Check if a snapshot exists.
    pub async fn snapshot_exists(&self, snapshot_id: &str) -> Result<bool, DomainError> {
        let name = Self::snapshot_name_from_id(snapshot_id);
        let image_tag = format!("bastion-snap-{}:latest", name);

        let output = tokio::process::Command::new("podman")
            .args(["image", "exists", &image_tag])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman image exists: {}", e)))?;

        Ok(output.status.success())
    }

    /// Restore a sandbox from a snapshot.
    pub async fn restore_snapshot(&self, snapshot_id: &str) -> Result<Sandbox, DomainError> {
        let name = Self::snapshot_name_from_id(snapshot_id);
        let image_tag = format!("bastion-snap-{}:latest", name);

        let new_id = SandboxId::generate();
        let container_name = new_id.to_string();

        tracing::info!(snapshot_id, image = %image_tag, new_id = %new_id, "Restoring snapshot");

        // Check image exists
        let exists = tokio::process::Command::new("podman")
            .args(["image", "exists", &image_tag])
            .output()
            .await;

        match exists {
            Ok(o) if o.status.success() => {}
            _ => {
                return Err(DomainError::NotFound(format!(
                    "Snapshot image '{}' not found",
                    image_tag
                )));
            }
        }

        // Create container from snapshot image
        let create = tokio::process::Command::new("podman")
            .args([
                "create",
                "--name",
                &container_name,
                &image_tag,
                "sleep",
                "infinity",
            ])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman create: {}", e)))?;

        if !create.status.success() {
            let stderr = String::from_utf8_lossy(&create.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to create from snapshot: {}",
                stderr
            )));
        }

        // Start container
        let start = tokio::process::Command::new("podman")
            .args(["start", &container_name])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman start: {}", e)))?;

        if !start.status.success() {
            let stderr = String::from_utf8_lossy(&start.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to start snapshot container: {}",
                stderr
            )));
        }

        let mut sandbox = Sandbox::new(
            new_id.clone(),
            bastion_domain::shared::id::TemplateId::new("podman-snapshot"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        sandbox.mark_running()?;

        tracing::info!(new_id = %new_id, "Snapshot restored");
        Ok(sandbox)
    }

    /// Delete a snapshot (remove the image).
    pub async fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), DomainError> {
        let name = Self::snapshot_name_from_id(snapshot_id);
        let image_tag = format!("bastion-snap-{}:latest", name);

        let output = tokio::process::Command::new("podman")
            .args(["rmi", &image_tag])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman rmi: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to delete snapshot: {}",
                stderr
            )));
        }
        Ok(())
    }

    /// List all snapshots (images matching bastion-snap-*).
    pub async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
        // List ALL images, then filter in Rust — podman wildcard doesn't handle
        // registry prefixes (e.g., localhost/bastion-snap-*) correctly in all contexts.
        let output = tokio::process::Command::new("podman")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("podman images: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DomainError::Internal(format!(
                "Failed to list snapshots: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut snapshots = Vec::new();
        let now = chrono::Utc::now();

        for line in stdout.lines() {
            let image_tag = line.trim();
            if image_tag.is_empty() {
                continue;
            }
            // Handle registry prefixes: localhost/bastion-snap-*, docker.io/bastion-snap-*, etc.
            let normalized = match image_tag.rfind("/bastion-snap-") {
                Some(pos) => &image_tag[pos + 1..], // strip registry prefix
                None => image_tag,
            };
            // Parse "bastion-snap-<name>:latest" format
            if let Some(name) = normalized.strip_prefix("bastion-snap-") {
                let name = name.trim_end_matches(":latest").replace('-', "/");
                snapshots.push(SnapshotInfo {
                    snapshot_id: format!("snap:{}", name),
                    sandbox_id: String::new(),
                    name,
                    created_at: now,
                    size_bytes: 0,
                });
            }
        }

        Ok(snapshots)
    }

    pub(crate) fn snapshot_name_from_id(snapshot_id: &str) -> String {
        let id = snapshot_id.strip_prefix("snap:").unwrap_or(snapshot_id);

        // Split on last '-', but only if the last part looks like a timestamp (all digits)
        if let Some((name, suffix)) = id.rsplit_once('-')
            && suffix.chars().all(|c| c.is_ascii_digit())
            && suffix.len() >= 10
        {
            return name.to_string();
        }

        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_name_extraction() {
        // Test standard format
        let name = SnapshotManager::snapshot_name_from_id("snap:test-snap-1712345678");
        assert_eq!(name, "test-snap");

        // Test with special characters
        let name2 = SnapshotManager::snapshot_name_from_id("snap:my-java-build-1712345678");
        assert_eq!(name2, "my-java-build");

        // Test without prefix should return original
        let name3 = SnapshotManager::snapshot_name_from_id("plain-name");
        assert_eq!(name3, "plain-name");
    }
}
