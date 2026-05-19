//! Volume source types for provider storage.
//!
//! Describes the backing storage for volumes.

use serde::{Deserialize, Serialize};

/// Type of host path.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPathType {
    /// Directory must exist on the host.
    Directory,
    /// File must exist on the host.
    File,
    /// Directory will be created if it doesn't exist.
    #[default]
    DirectoryOrCreate,
    /// File will be created if it doesn't exist.
    FileOrCreate,
    /// Directory or file will be created if it doesn't exist.
    Tmpfs,
}

/// Medium type for volumes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeMedium {
    /// Standard filesystem storage.
    #[default]
    Default,
    /// Memory-backed filesystem (tmpfs).
    Memory,
    /// Huge pages.
    HugePages,
}

/// Source of a volume for provider instances.
///
/// Describes where persistent storage comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VolumeSource {
    /// Empty volume (ephemeral).
    Empty {},
    /// Host path volume.
    HostPath {
        /// Path on the host filesystem.
        path: String,
        /// Type of host path.
        #[serde(default)]
        path_type: HostPathType,
    },
    /// Persistent volume claim.
    PersistentVolumeClaim {
        /// Name of the PVC.
        claim_name: String,
        /// Whether the volume is read-only.
        #[serde(default)]
        read_only: bool,
    },
    /// ConfigMap volume.
    ConfigMap {
        /// Name of the ConfigMap.
        name: String,
        /// Optional items to select from the ConfigMap.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        items: Option<std::collections::HashMap<String, String>>,
    },
    /// Secret volume.
    Secret {
        /// Name of the Secret.
        name: String,
        /// Optional items to select from the Secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        items: Option<std::collections::HashMap<String, String>>,
    },
    /// Volume with a specific medium (e.g., hugepages).
    Medium {
        /// The medium type.
        medium: VolumeMedium,
        /// Size limit for the volume (in bytes).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_limit: Option<u64>,
    },
}

impl VolumeSource {
    /// Create an empty volume source.
    pub fn empty() -> Self {
        Self::Empty {}
    }

    /// Create a host path volume source.
    pub fn host_path(path: impl Into<String>) -> Self {
        Self::HostPath {
            path: path.into(),
            path_type: HostPathType::default(),
        }
    }

    /// Create a host path volume source with a specific type.
    pub fn host_path_with_type(path: impl Into<String>, path_type: HostPathType) -> Self {
        Self::HostPath {
            path: path.into(),
            path_type,
        }
    }

    /// Create a persistent volume claim source.
    pub fn pvc(claim_name: impl Into<String>) -> Self {
        Self::PersistentVolumeClaim {
            claim_name: claim_name.into(),
            read_only: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_source_empty() {
        let source = VolumeSource::empty();
        assert!(matches!(source, VolumeSource::Empty { .. }));
    }

    #[test]
    fn test_volume_source_host_path() {
        let source = VolumeSource::host_path("/data");
        assert!(matches!(source, VolumeSource::HostPath { .. }));
    }

    #[test]
    fn test_volume_source_pvc() {
        let source = VolumeSource::pvc("my-pvc");
        assert!(matches!(source, VolumeSource::PersistentVolumeClaim { .. }));
    }

    #[test]
    fn test_host_path_type_default() {
        assert_eq!(HostPathType::default(), HostPathType::DirectoryOrCreate);
    }

    #[test]
    fn test_volume_medium_default() {
        assert_eq!(VolumeMedium::default(), VolumeMedium::Default);
    }

    #[test]
    fn test_volume_source_serde_empty() {
        let source = VolumeSource::empty();
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"empty\""));
        let parsed: VolumeSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, VolumeSource::Empty { .. }));
    }

    #[test]
    fn test_volume_source_serde_host_path() {
        let source = VolumeSource::host_path("/mnt/data");
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"host_path\""));
        let parsed: VolumeSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, VolumeSource::HostPath { .. }));
    }

    #[test]
    fn test_volume_source_serde_pvc() {
        let source = VolumeSource::pvc("my-claim");
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"persistent_volume_claim\""));
        let parsed: VolumeSource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, VolumeSource::PersistentVolumeClaim { .. }));
    }
}
