//! Mount reference types for provider volumes and mounts.
//!
//! Describes how directories are mounted into provider sandboxes.

use serde::{Deserialize, Serialize};

/// A mount specification for binding a directory into a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountRef {
    /// Source path on the host.
    pub source: String,
    /// Target path inside the sandbox.
    pub target: String,
    /// Whether the mount is read-only.
    #[serde(default)]
    pub read_only: bool,
    /// Mount type (defaults to bind).
    #[serde(default)]
    pub mount_type: String,
}

impl MountRef {
    /// Create a new read-write mount.
    pub fn new_rw(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            read_only: false,
            mount_type: "bind".to_string(),
        }
    }

    /// Create a new read-only mount.
    pub fn new_ro(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            read_only: true,
            mount_type: "bind".to_string(),
        }
    }
}

/// Network mode for containers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerNetworkMode {
    /// Bridge networking (default).
    #[default]
    Bridge,
    /// No networking.
    None,
    /// Host networking.
    Host,
    /// Container networking (join another container's network).
    Container,
}

impl std::fmt::Display for ContainerNetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bridge => write!(f, "bridge"),
            Self::None => write!(f, "none"),
            Self::Host => write!(f, "host"),
            Self::Container => write!(f, "container"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mount_ref_new_rw() {
        let mount = MountRef::new_rw("/host/data", "/container/data");
        assert_eq!(mount.source, "/host/data");
        assert_eq!(mount.target, "/container/data");
        assert!(!mount.read_only);
    }

    #[test]
    fn test_mount_ref_new_ro() {
        let mount = MountRef::new_ro("/host/artifacts", "/artifacts");
        assert_eq!(mount.source, "/host/artifacts");
        assert_eq!(mount.target, "/artifacts");
        assert!(mount.read_only);
    }

    #[test]
    fn test_mount_ref_serde() {
        let mount = MountRef::new_rw("/source", "/target");
        let json = serde_json::to_string(&mount).unwrap();
        let parsed: MountRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source, "/source");
        assert_eq!(parsed.target, "/target");
        assert!(!parsed.read_only);
    }

    #[test]
    fn test_container_network_mode_default() {
        assert_eq!(ContainerNetworkMode::default(), ContainerNetworkMode::Bridge);
    }

    #[test]
    fn test_container_network_mode_display() {
        assert_eq!(format!("{}", ContainerNetworkMode::Bridge), "bridge");
        assert_eq!(format!("{}", ContainerNetworkMode::None), "none");
        assert_eq!(format!("{}", ContainerNetworkMode::Host), "host");
        assert_eq!(format!("{}", ContainerNetworkMode::Container), "container");
    }

    #[test]
    fn test_container_network_mode_serde() {
        let mode = ContainerNetworkMode::Host;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"host\"");
        let parsed: ContainerNetworkMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }
}
