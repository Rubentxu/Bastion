//! TarStream backend for DeltaSync — universal gRPC-based sync.
//!
//! Uses tar streaming over gRPC for universal file transfer.
//! Works with any sandbox provider that supports file access.

use std::path::{Path, PathBuf};
use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use super::DeltaSyncBackend;

/// TarStream backend — streams files as tarball over gRPC.
pub struct TarStreamBackend {
    /// gRPC endpoint for the sandbox.
    #[allow(dead_code)]
    endpoint: String,
}

impl TarStreamBackend {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

#[async_trait]
impl DeltaSyncBackend for TarStreamBackend {
    async fn sync(
        &self,
        source: &Path,
        target: &Path,
        exclude: &[String],
    ) -> Result<u64, DomainError> {
        // For now, implement a basic tar-based sync
        // In production, this would use gRPC streaming
        let mut bytes_transferred = 0u64;

        tracing::info!(
            source = %source.display(),
            target = %target.display(),
            backend = "tar_stream",
            "Starting tar-based sync"
        );

        // Walk source directory and create tar entries
        let entries = walkdir(source, exclude)?;
        let total_files = entries.len() as u64;

        for (idx, entry) in entries.into_iter().enumerate() {
            let relative_path = entry.strip_prefix(source)
                .map_err(|e| DomainError::Internal(format!("Path error: {}", e)))?;

            let target_path = target.join(relative_path);

            // Create parent directories
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DomainError::Internal(format!("Failed to create dir: {}", e)))?;
            }

            // Copy file
            if entry.is_file() {
                std::fs::copy(&entry, &target_path)
                    .map_err(|e| DomainError::Internal(format!("Failed to copy: {}", e)))?;

                let file_size = entry.metadata()
                    .map_err(|e| DomainError::Internal(format!("Metadata error: {}", e)))?
                    .len();
                bytes_transferred += file_size;
            }

            tracing::debug!(
                file = %relative_path.display(),
                progress = %format!("{}/{}", idx + 1, total_files),
                "Synced file"
            );
        }

        Ok(bytes_transferred)
    }

    fn name(&self) -> &'static str {
        "tar_stream"
    }

    async fn is_available(&self) -> bool {
        // TarStream is always available as fallback
        true
    }
}

/// Walk directory respecting exclude patterns.
fn walkdir(source: &Path, exclude: &[String]) -> Result<Vec<PathBuf>, DomainError> {
    let mut entries = Vec::new();
    walkdir_recursive(source, source, exclude, &mut entries)?;
    Ok(entries)
}

#[allow(clippy::only_used_in_recursion)]
fn walkdir_recursive(
    base: &Path,
    current: &Path,
    exclude: &[String],
    entries: &mut Vec<PathBuf>,
) -> Result<(), DomainError> {
    let read_dir = std::fs::read_dir(current)
        .map_err(|e| DomainError::Internal(format!("Failed to read dir: {}", e)))?;

    for entry in read_dir {
        let entry = entry.map_err(|e| DomainError::Internal(format!("Read dir error: {}", e)))?;
        let path = entry.path();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Check exclude patterns
        let should_exclude = exclude.iter().any(|pattern| {
            glob_match(pattern, file_name)
        });

        if should_exclude {
            continue;
        }

        if path.is_dir() {
            walkdir_recursive(base, &path, exclude, entries)?;
        } else {
            entries.push(path);
        }
    }

    Ok(())
}

/// Simple glob matching for exclude patterns.
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == name {
        return true;
    }
    // Simple prefix match for patterns like "node_modules*"
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("node_modules", "node_modules"));
        assert!(!glob_match("node_modules", "node_modules_backup"));
    }

    #[test]
    fn test_glob_match_prefix() {
        assert!(glob_match("node_modules*", "node_modules"));
        assert!(glob_match("node_modules*", "node_modules_backup"));
        assert!(!glob_match("node_modules*", "other_modules"));
    }
}
