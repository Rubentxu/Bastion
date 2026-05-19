//! Binary reference types for provider binaries.
//!
//! Describes where a provider binary comes from.

use serde::{Deserialize, Serialize};

/// Reference to a binary executable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BinaryRef {
    /// Binary exists locally on the filesystem.
    Local {
        /// Absolute or relative path to the binary.
        path: String,
    },
    /// Binary must be downloaded from a remote URL.
    Remote {
        /// URL to download the binary from.
        url: String,
        /// Expected SHA256 checksum of the downloaded file.
        checksum: Option<String>,
    },
}

impl BinaryRef {
    /// Create a local binary reference.
    pub fn local(path: impl Into<String>) -> Self {
        Self::Local { path: path.into() }
    }

    /// Create a remote binary reference.
    pub fn remote(url: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: None,
        }
    }

    /// Create a remote binary reference with checksum.
    pub fn remote_with_checksum(url: impl Into<String>, checksum: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: Some(checksum.into()),
        }
    }

    /// Check if this is a local binary.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// Check if this is a remote binary.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Get the path for local binaries, None for remote.
    pub fn local_path(&self) -> Option<&str> {
        match self {
            Self::Local { path } => Some(path),
            Self::Remote { .. } => None,
        }
    }

    /// Get the URL for remote binaries, None for local.
    pub fn remote_url(&self) -> Option<&str> {
        match self {
            Self::Local { .. } => None,
            Self::Remote { url, .. } => Some(url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_ref_local() {
        let binary = BinaryRef::local("/usr/bin/firecracker");
        assert!(binary.is_local());
        assert!(!binary.is_remote());
        assert_eq!(binary.local_path(), Some("/usr/bin/firecracker"));
    }

    #[test]
    fn test_binary_ref_remote() {
        let binary = BinaryRef::remote("https://example.com/firecracker");
        assert!(!binary.is_local());
        assert!(binary.is_remote());
        assert_eq!(binary.remote_url(), Some("https://example.com/firecracker"));
    }

    #[test]
    fn test_binary_ref_remote_with_checksum() {
        let binary = BinaryRef::remote_with_checksum(
            "https://example.com/firecracker",
            "abc123",
        );
        match binary {
            BinaryRef::Remote { url, checksum } => {
                assert_eq!(url, "https://example.com/firecracker");
                assert_eq!(checksum, Some("abc123".to_string()));
            }
            _ => panic!("expected remote"),
        }
    }

    #[test]
    fn test_binary_ref_serde_local() {
        let binary = BinaryRef::local("/usr/bin/runsc");
        let json = serde_json::to_string(&binary).unwrap();
        assert!(json.contains("\"type\":\"local\""));
        let parsed: BinaryRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.local_path(), Some("/usr/bin/runsc"));
    }

    #[test]
    fn test_binary_ref_serde_remote() {
        let binary = BinaryRef::remote("https://example.com/runsc");
        let json = serde_json::to_string(&binary).unwrap();
        assert!(json.contains("\"type\":\"remote\""));
        let parsed: BinaryRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.remote_url(), Some("https://example.com/runsc"));
    }
}
