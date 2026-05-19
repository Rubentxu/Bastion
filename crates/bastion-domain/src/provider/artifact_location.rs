//! Artifact location types for build artifacts.
//!
//! Describes where a build artifact is stored.

use serde::{Deserialize, Serialize};

/// Location of a build artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactLocation {
    /// Artifact exists locally on the filesystem.
    Local {
        /// Absolute or relative path to the artifact.
        path: String,
    },
    /// Artifact must be downloaded from a remote URL.
    Remote {
        /// URL to download the artifact from.
        url: String,
        /// Expected SHA256 checksum of the downloaded file.
        checksum: Option<String>,
    },
    /// Artifact is pre-baked into the runtime image.
    PreBakedInImage {
        /// Path within the image where the artifact is located.
        path: String,
    },
}

impl ArtifactLocation {
    /// Create a local artifact location.
    pub fn local(path: impl Into<String>) -> Self {
        Self::Local { path: path.into() }
    }

    /// Create a remote artifact location.
    pub fn remote(url: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: None,
        }
    }

    /// Create a remote artifact location with checksum.
    pub fn remote_with_checksum(url: impl Into<String>, checksum: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: Some(checksum.into()),
        }
    }

    /// Create a pre-baked artifact location.
    pub fn pre_baked(path: impl Into<String>) -> Self {
        Self::PreBakedInImage { path: path.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_location_local() {
        let location = ArtifactLocation::local("/artifacts/output.so");
        assert!(matches!(location, ArtifactLocation::Local { .. }));
    }

    #[test]
    fn test_artifact_location_remote() {
        let location = ArtifactLocation::remote("https://cdn.example.com/output.so");
        assert!(matches!(location, ArtifactLocation::Remote { .. }));
    }

    #[test]
    fn test_artifact_location_pre_baked() {
        let location = ArtifactLocation::pre_baked("/opt/app/output.so");
        assert!(matches!(location, ArtifactLocation::PreBakedInImage { .. }));
    }

    #[test]
    fn test_artifact_location_serde_local() {
        let location = ArtifactLocation::local("/local/artifact");
        let json = serde_json::to_string(&location).unwrap();
        assert!(json.contains("\"type\":\"local\""));
        let parsed: ArtifactLocation = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ArtifactLocation::Local { .. }));
    }

    #[test]
    fn test_artifact_location_serde_remote() {
        let location = ArtifactLocation::remote("https://example.com/artifact");
        let json = serde_json::to_string(&location).unwrap();
        assert!(json.contains("\"type\":\"remote\""));
        let parsed: ArtifactLocation = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ArtifactLocation::Remote { .. }));
    }

    #[test]
    fn test_artifact_location_serde_pre_baked() {
        let location = ArtifactLocation::pre_baked("/prebaked/artifact");
        let json = serde_json::to_string(&location).unwrap();
        assert!(json.contains("\"type\":\"pre_baked_in_image\""));
        let parsed: ArtifactLocation = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ArtifactLocation::PreBakedInImage { .. }));
    }
}
