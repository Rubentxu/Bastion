//! Snapshot operations using the bollard Docker API client.
//!
//! Shared implementation for DockerProvider and PodmanProvider.
//! Replaces the previous CLI-based SnapshotManager.

use bollard::Docker;
use bollard::models::{ContainerConfig, ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CommitContainerOptionsBuilder, CreateContainerOptionsBuilder, ListImagesOptionsBuilder,
    RemoveImageOptionsBuilder,
};

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::{ProviderId, SandboxId, TemplateId};

const SNAPSHOT_PREFIX: &str = "bastion-snap-";
const SNAPSHOT_TAG: &str = "latest";

/// Build the full image tag for a named snapshot.
fn image_tag(name: &str) -> String {
    format!(
        "{}{}:{}",
        SNAPSHOT_PREFIX,
        name.replace('/', "-"),
        SNAPSHOT_TAG
    )
}

/// Extract snapshot name from a snapshot_id (e.g., "snap:my-build-1712345678").
pub fn snapshot_name_from_id(snapshot_id: &str) -> String {
    let id = snapshot_id.strip_prefix("snap:").unwrap_or(snapshot_id);
    if let Some((name, suffix)) = id.rsplit_once('-')
        && suffix.chars().all(|c| c.is_ascii_digit())
        && suffix.len() >= 10
    {
        return name.to_string();
    }
    id.to_string()
}

/// Check if the image tag matches a bastion snapshot.
fn is_snapshot_image(repo_tag: &str) -> bool {
    // Strip registry prefix like "localhost/" or "docker.io/"
    let normalized = repo_tag.rsplit('/').next().unwrap_or(repo_tag);
    normalized.starts_with(SNAPSHOT_PREFIX)
}

/// Parse an image tag like "bastion-snap-my-build:latest" into snapshot name.
fn parse_snapshot_name(repo_tag: &str) -> Option<String> {
    let normalized = repo_tag.rsplit('/').next().unwrap_or(repo_tag);
    let trimmed = normalized
        .strip_prefix(SNAPSHOT_PREFIX)?
        .trim_end_matches(&format!(":{}", SNAPSHOT_TAG));
    Some(trimmed.to_string())
}

/// Create a snapshot (commit container → image).
pub async fn create_snapshot(
    docker: &Docker,
    container_name: &str,
    name: &str,
) -> Result<SnapshotInfo, DomainError> {
    let tag = image_tag(name);

    let options = CommitContainerOptionsBuilder::default()
        .container(container_name)
        .tag(&tag)
        .build();

    docker
        .commit_container(options, ContainerConfig::default())
        .await
        .map_err(|e| DomainError::Internal(format!("Failed to commit snapshot: {e}")))?;

    let now = chrono::Utc::now();
    let snapshot_id = format!("snap:{}-{}", name.replace('/', "-"), now.timestamp());

    Ok(SnapshotInfo {
        snapshot_id,
        sandbox_id: String::new(),
        name: name.to_string(),
        created_at: now,
        size_bytes: 0,
    })
}

/// Check if a snapshot image exists.
pub async fn snapshot_exists(docker: &Docker, name: &str) -> Result<bool, DomainError> {
    let tag = image_tag(name);
    match docker.inspect_image(&tag).await {
        Ok(_) => Ok(true),
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(false),
        Err(e) => Err(DomainError::Internal(format!(
            "Failed to inspect snapshot image: {e}"
        ))),
    }
}

/// Restore a sandbox from a snapshot image.
pub async fn restore_snapshot(docker: &Docker, snapshot_id: &str) -> Result<Sandbox, DomainError> {
    let name = snapshot_name_from_id(snapshot_id);
    let tag = image_tag(&name);

    // Verify image exists
    docker
        .inspect_image(&tag)
        .await
        .map_err(|e| DomainError::NotFound(format!("Snapshot image '{}' not found: {e}", tag)))?;

    let new_id = SandboxId::generate();
    let container_name = new_id.to_string();

    // Create container from snapshot image
    let create_options = CreateContainerOptionsBuilder::default()
        .name(&container_name)
        .build();

    let container_config = ContainerCreateBody {
        image: Some(tag),
        cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
        tty: Some(false),
        attach_stdout: Some(false),
        attach_stderr: Some(false),
        host_config: Some(HostConfig::default()),
        ..Default::default()
    };

    docker
        .create_container(Some(create_options), container_config)
        .await
        .map_err(|e| {
            DomainError::Internal(format!("Failed to create container from snapshot: {e}"))
        })?;

    // Start container
    docker
        .start_container(
            &container_name,
            None::<bollard::query_parameters::StartContainerOptions>,
        )
        .await
        .map_err(|e| DomainError::Internal(format!("Failed to start snapshot container: {e}")))?;

    let mut sandbox = Sandbox::new(
        new_id.clone(),
        TemplateId::new("podman-snapshot"),
        ProviderId::new("podman"),
        ResourcesSpec::default(),
        NetworkSpec::default(),
    );
    sandbox.mark_running()?;

    Ok(sandbox)
}

/// Delete a snapshot (remove the image).
pub async fn delete_snapshot(docker: &Docker, snapshot_id: &str) -> Result<(), DomainError> {
    let name = snapshot_name_from_id(snapshot_id);
    let tag = image_tag(&name);

    docker
        .remove_image(
            &tag,
            Some(RemoveImageOptionsBuilder::default().build()),
            None,
        )
        .await
        .map_err(|e| DomainError::Internal(format!("Failed to delete snapshot image: {e}")))?;

    Ok(())
}

/// List all snapshots (images matching bastion-snap-*).
pub async fn list_snapshots(docker: &Docker) -> Result<Vec<SnapshotInfo>, DomainError> {
    let options = ListImagesOptionsBuilder::default().all(true).build();

    let images = docker
        .list_images(Some(options))
        .await
        .map_err(|e| DomainError::Internal(format!("Failed to list images: {e}")))?;

    let now = chrono::Utc::now();
    let mut snapshots = Vec::new();

    for image in &images {
        for repo_tag in &image.repo_tags {
            if is_snapshot_image(repo_tag) {
                if let Some(name) = parse_snapshot_name(repo_tag) {
                    snapshots.push(SnapshotInfo {
                        snapshot_id: format!("snap:{}", name),
                        sandbox_id: String::new(),
                        name,
                        created_at: now,
                        size_bytes: image.size as u64,
                    });
                }
            }
        }
    }

    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_name_from_id_standard() {
        let name = snapshot_name_from_id("snap:test-snap-1712345678");
        assert_eq!(name, "test-snap");
    }

    #[test]
    fn test_snapshot_name_from_id_no_prefix() {
        let name = snapshot_name_from_id("plain-name");
        assert_eq!(name, "plain-name");
    }

    #[test]
    fn test_is_snapshot_image() {
        assert!(is_snapshot_image("bastion-snap-my-build:latest"));
        assert!(is_snapshot_image("localhost/bastion-snap-my-build:latest"));
        assert!(!is_snapshot_image("ubuntu:latest"));
    }

    #[test]
    fn test_parse_snapshot_name() {
        assert_eq!(
            parse_snapshot_name("bastion-snap-my-build:latest"),
            Some("my-build".to_string())
        );
        assert_eq!(
            parse_snapshot_name("localhost/bastion-snap-java-17:latest"),
            Some("java-17".to_string())
        );
        assert_eq!(parse_snapshot_name("ubuntu:latest"), None);
    }
}
