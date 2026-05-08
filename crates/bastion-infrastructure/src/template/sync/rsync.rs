//! Rsync backend for DeltaSync — local-optimized file sync.
//!
//! Uses the rsync binary for efficient delta transfer when available.
//! Preferred for local providers like Podman.

use super::DeltaSyncBackend;
use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Rsync backend — uses rsync binary for efficient sync.
pub struct RsyncBackend {
    /// Base path for rsync operations.
    #[allow(dead_code)]
    base_path: PathBuf,
}

impl RsyncBackend {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

#[async_trait]
impl DeltaSyncBackend for RsyncBackend {
    async fn sync(
        &self,
        source: &Path,
        target: &Path,
        exclude: &[String],
    ) -> Result<u64, DomainError> {
        // Build rsync command
        let mut args = vec![
            "-avz".to_string(), // archive, verbose, compress
        ];

        // Add exclude patterns
        for pattern in exclude {
            args.push(format!("--exclude={}", pattern));
        }

        args.push(source.to_string_lossy().to_string());
        args.push(target.to_string_lossy().to_string());

        tracing::debug!(
            source = %source.display(),
            target = %target.display(),
            backend = "rsync",
            "Starting rsync sync"
        );

        // Execute rsync
        let output = Command::new("rsync")
            .args(&args)
            .output()
            .map_err(|e| DomainError::Internal(format!("Failed to execute rsync: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DomainError::Internal(format!("rsync failed: {}", stderr)));
        }

        // Parse bytes transferred from rsync output
        // rsync outputs "X bytes received" or "X bytes sent"
        let stdout = String::from_utf8_lossy(&output.stdout);
        let bytes_transferred = parse_rsync_output(&stdout);

        Ok(bytes_transferred)
    }

    fn name(&self) -> &'static str {
        "rsync"
    }

    async fn is_available(&self) -> bool {
        // Check if rsync binary exists
        Command::new("rsync")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Parse rsync output to get bytes transferred.
fn parse_rsync_output(output: &str) -> u64 {
    // Look for patterns like "X bytes received" or "sent X bytes"
    for line in output.lines() {
        if line.contains("bytes received")
            && let Some(bytes) = extract_number(line, "bytes received")
        {
            return bytes;
        }
        if line.contains("bytes sent")
            && let Some(bytes) = extract_number(line, "bytes sent")
        {
            return bytes;
        }
    }
    0
}

fn extract_number(line: &str, pattern: &str) -> Option<u64> {
    let idx = line.find(pattern)?;
    let before = &line[..idx];
    // Find the last number before the pattern
    let number_str: String = before.chars().filter(|c| c.is_ascii_digit()).collect();
    number_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rsync_output_bytes_received() {
        let output = "receiving incremental file list\nfile1.txt\n500 bytes received";
        assert_eq!(parse_rsync_output(output), 500);
    }

    #[test]
    fn test_parse_rsync_output_bytes_sent() {
        let output = "sending incremental file list\n1000 bytes sent";
        assert_eq!(parse_rsync_output(output), 1000);
    }

    #[test]
    fn test_parse_rsync_output_no_match() {
        let output = "sending incremental file list";
        assert_eq!(parse_rsync_output(output), 0);
    }
}
