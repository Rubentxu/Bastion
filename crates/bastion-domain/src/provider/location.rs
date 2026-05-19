//! Generic location types for provider resources.
//!
//! This module provides a unified `Location<C>` enum that can represent
//! local files, remote URLs, and pre-baked resources, parameterized by
//! context type to provide type safety.

use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::path::PathBuf;

/// Marker trait for location context types.
///
/// Each context type corresponds to a specific resource type (e.g., artifacts, binaries).
/// The context is used as a type parameter to ensure compile-time safety when
/// mixing different location types.
pub trait LocationContext: 'static + std::fmt::Debug + Clone {}

/// Context for artifact locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ArtifactContext;
impl LocationContext for ArtifactContext {}

/// Context for binary locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BinaryContext;
impl LocationContext for BinaryContext {}

/// Context for raw/untyped locations (generic fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RawContext;
impl LocationContext for RawContext {}

/// A generic location type that can represent local files, remote URLs,
/// or pre-baked resources.
///
/// The context type `C` parameter provides type safety to distinguish
/// between different resource types (e.g., `ArtifactLocation = Location<ArtifactContext>`).
///
/// # Variants
///
/// - `Local`: Resource exists locally on the filesystem
/// - `Remote`: Resource must be downloaded from a remote URL
/// - `PreBaked`: Resource is pre-baked into the runtime image
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Location<C: LocationContext = RawContext> {
    /// Resource exists locally on the filesystem.
    Local {
        /// Absolute or relative path to the resource.
        #[serde(default)]
        path: PathBuf,
    },
    /// Resource must be downloaded from a remote URL.
    Remote {
        /// URL to download the resource from.
        url: String,
        /// Expected SHA256 checksum of the downloaded file.
        #[serde(default)]
        checksum: Option<String>,
    },
    /// Resource is pre-baked into the runtime image.
    PreBaked {
        /// Path within the image where the resource is located.
        #[serde(default)]
        path: PathBuf,
    },

    /// Phantom variant to make the context type C meaningful.
    #[serde(skip)]
    _Phantom(PhantomData<C>),
}

impl<C: LocationContext> Location<C> {
    /// Create a local location.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local { path: path.into() }
    }

    /// Create a remote location.
    pub fn remote(url: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: None,
        }
    }

    /// Create a remote location with checksum.
    pub fn remote_with_checksum(url: impl Into<String>, checksum: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: Some(checksum.into()),
        }
    }

    /// Create a pre-baked location.
    pub fn pre_baked(path: impl Into<PathBuf>) -> Self {
        Self::PreBaked { path: path.into() }
    }

    /// Check if this is a local location.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// Check if this is a remote location.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Check if this is a pre-baked location.
    pub fn is_pre_baked(&self) -> bool {
        matches!(self, Self::PreBaked { .. })
    }

    /// Get the path for local or pre-baked locations.
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Self::Local { path } | Self::PreBaked { path } => Some(path),
            Self::Remote { .. } | Self::_Phantom(_) => None,
        }
    }

    /// Get the URL for remote locations.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Remote { url, .. } => Some(url),
            Self::Local { .. } | Self::PreBaked { .. } | Self::_Phantom(_) => None,
        }
    }

    /// Get the checksum for remote locations.
    pub fn checksum(&self) -> Option<&str> {
        match self {
            Self::Remote { checksum, .. } => checksum.as_deref(),
            Self::Local { .. } | Self::PreBaked { .. } | Self::_Phantom(_) => None,
        }
    }
}

/// Socket location types for network connections.
///
/// These represent different ways to connect to a provider runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketLocation {
    /// Unix domain socket.
    Unix {
        /// Socket file path.
        path: PathBuf,
    },
    /// TCP socket.
    Tcp {
        /// Host name or IP address.
        host: String,
        /// Port number.
        port: u16,
    },
    /// SSH tunnel to remote socket.
    Ssh {
        /// Host:port of the remote endpoint.
        address: String,
        /// Path to SSH private key.
        #[serde(default)]
        ssh_key: Option<String>,
        /// SSH username.
        #[serde(default)]
        ssh_user: Option<String>,
    },
}

impl SocketLocation {
    /// Create a Unix socket location.
    pub fn unix(path: impl Into<PathBuf>) -> Self {
        Self::Unix { path: path.into() }
    }

    /// Create a TCP socket location.
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Self::Tcp {
            host: host.into(),
            port,
        }
    }

    /// Create an SSH socket location.
    pub fn ssh(address: impl Into<String>) -> Self {
        Self::Ssh {
            address: address.into(),
            ssh_key: None,
            ssh_user: None,
        }
    }

    /// Create an SSH socket location with key and user.
    pub fn ssh_with_auth(
        address: impl Into<String>,
        ssh_key: impl Into<String>,
        ssh_user: impl Into<String>,
    ) -> Self {
        Self::Ssh {
            address: address.into(),
            ssh_key: Some(ssh_key.into()),
            ssh_user: Some(ssh_user.into()),
        }
    }
}

/// Mount location types for volume mounts.
///
/// These represent different types of mounts into provider sandboxes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MountLocation {
    /// Bind mount from host filesystem.
    Bind {
        /// Source path on the host.
        source: PathBuf,
        /// Target path inside the sandbox.
        target: PathBuf,
        /// Whether the mount is read-only.
        #[serde(default)]
        read_only: bool,
    },
    /// Named volume.
    Volume {
        /// Volume name.
        name: String,
    },
}

impl MountLocation {
    /// Create a read-write bind mount.
    pub fn bind_rw(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self::Bind {
            source: source.into(),
            target: target.into(),
            read_only: false,
        }
    }

    /// Create a read-only bind mount.
    pub fn bind_ro(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self::Bind {
            source: source.into(),
            target: target.into(),
            read_only: true,
        }
    }

    /// Create a named volume mount.
    pub fn volume(name: impl Into<String>) -> Self {
        Self::Volume { name: name.into() }
    }
}

// ============================================================================
// Conversions from legacy types to new generic types
// ============================================================================

use crate::provider::artifact_location::ArtifactLocation as LegacyArtifactLocation;
use crate::provider::binary_ref::BinaryRef as LegacyBinaryRef;

impl From<LegacyArtifactLocation> for Location<ArtifactContext> {
    fn from(other: LegacyArtifactLocation) -> Self {
        match other {
            LegacyArtifactLocation::Local { path } => Location::Local { path: path.into() },
            LegacyArtifactLocation::Remote { url, checksum } => {
                Location::Remote { url, checksum }
            }
            LegacyArtifactLocation::PreBakedInImage { path } => {
                Location::PreBaked { path: path.into() }
            }
        }
    }
}

impl From<Location<ArtifactContext>> for LegacyArtifactLocation {
    fn from(other: Location<ArtifactContext>) -> Self {
        match other {
            Location::Local { path } => LegacyArtifactLocation::Local { path: path.to_string_lossy().into() },
            Location::Remote { url, checksum } => LegacyArtifactLocation::Remote { url, checksum },
            Location::PreBaked { path } => {
                LegacyArtifactLocation::PreBakedInImage { path: path.to_string_lossy().into() }
            }
            Location::_Phantom(_) => unreachable!(),
        }
    }
}

impl From<LegacyBinaryRef> for Location<BinaryContext> {
    fn from(other: LegacyBinaryRef) -> Self {
        match other {
            LegacyBinaryRef::Local { path } => Location::Local { path: path.into() },
            LegacyBinaryRef::Remote { url, checksum } => Location::Remote { url, checksum },
        }
    }
}

impl From<Location<BinaryContext>> for LegacyBinaryRef {
    fn from(other: Location<BinaryContext>) -> Self {
        match other {
            Location::Local { path } => LegacyBinaryRef::Local { path: path.to_string_lossy().into() },
            Location::Remote { url, checksum } => LegacyBinaryRef::Remote { url, checksum },
            Location::PreBaked { path } => {
                // BinaryRef doesn't have PreBaked, default to Local
                LegacyBinaryRef::Local { path: path.to_string_lossy().into() }
            }
            Location::_Phantom(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Location tests ====================

    #[test]
    fn test_location_local() {
        let loc = Location::<ArtifactContext>::local("/artifacts/output.so");
        assert!(loc.is_local());
        assert!(!loc.is_remote());
        assert!(!loc.is_pre_baked());
        assert_eq!(loc.path(), Some(&PathBuf::from("/artifacts/output.so")));
    }

    #[test]
    fn test_location_remote() {
        let loc = Location::<ArtifactContext>::remote("https://cdn.example.com/output.so");
        assert!(!loc.is_local());
        assert!(loc.is_remote());
        assert!(!loc.is_pre_baked());
        assert_eq!(
            loc.url(),
            Some("https://cdn.example.com/output.so")
        );
        assert!(loc.checksum().is_none());
    }

    #[test]
    fn test_location_remote_with_checksum() {
        let loc =
            Location::<ArtifactContext>::remote_with_checksum("https://cdn.example.com/output.so", "abc123");
        assert!(loc.is_remote());
        assert_eq!(loc.checksum(), Some("abc123"));
    }

    #[test]
    fn test_location_pre_baked() {
        let loc = Location::<ArtifactContext>::pre_baked("/opt/app/output.so");
        assert!(!loc.is_local());
        assert!(!loc.is_remote());
        assert!(loc.is_pre_baked());
        assert_eq!(loc.path(), Some(&PathBuf::from("/opt/app/output.so")));
    }

    #[test]
    fn test_location_context_markers() {
        // Verify that different context types create distinct types
        let artifact_loc: Location<ArtifactContext> = Location::local("/artifact");
        let binary_loc: Location<BinaryContext> = Location::local("/binary");
        // Both are local, but they are different types
        assert!(artifact_loc.is_local());
        assert!(binary_loc.is_local());
    }

    #[test]
    fn test_location_serde_local() {
        let loc = Location::<ArtifactContext>::local("/local/artifact");
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("\"type\":\"local\""));
        let parsed: Location<ArtifactContext> = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_local());
    }

    #[test]
    fn test_location_serde_remote() {
        let loc = Location::<ArtifactContext>::remote("https://example.com/artifact");
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("\"type\":\"remote\""));
        let parsed: Location<ArtifactContext> = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_remote());
    }

    #[test]
    fn test_location_serde_pre_baked() {
        let loc = Location::<ArtifactContext>::pre_baked("/prebaked/artifact");
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("\"type\":\"pre_baked\""));
        let parsed: Location<ArtifactContext> = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_pre_baked());
    }

    // ==================== SocketLocation tests ====================

    #[test]
    fn test_socket_location_unix() {
        let sock = SocketLocation::unix("/run/podman/podman.sock");
        assert!(matches!(sock, SocketLocation::Unix { .. }));
    }

    #[test]
    fn test_socket_location_tcp() {
        let sock = SocketLocation::tcp("localhost", 2375);
        assert!(matches!(sock, SocketLocation::Tcp { host, port: 2375 } if host == "localhost"));
    }

    #[test]
    fn test_socket_location_ssh() {
        let sock = SocketLocation::ssh("example.com:2375");
        assert!(matches!(sock, SocketLocation::Ssh { address, .. } if address == "example.com:2375"));
    }

    #[test]
    fn test_socket_location_ssh_with_auth() {
        let sock = SocketLocation::ssh_with_auth("example.com:22", "/path/to/key", "user");
        match sock {
            SocketLocation::Ssh {
                address,
                ssh_key,
                ssh_user,
            } => {
                assert_eq!(address, "example.com:22");
                assert_eq!(ssh_key, Some("/path/to/key".to_string()));
                assert_eq!(ssh_user, Some("user".to_string()));
            }
            _ => panic!("expected Ssh variant"),
        }
    }

    #[test]
    fn test_socket_location_serde() {
        let sock = SocketLocation::unix("/var/run/docker.sock");
        let json = serde_json::to_string(&sock).unwrap();
        assert!(json.contains("\"type\":\"unix\""));
        let parsed: SocketLocation = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SocketLocation::Unix { .. }));
    }

    // ==================== MountLocation tests ====================

    #[test]
    fn test_mount_location_bind_rw() {
        let mount = MountLocation::bind_rw("/host/data", "/container/data");
        assert!(matches!(
            mount,
            MountLocation::Bind {
                source,
                target,
                read_only: false
            } if source == PathBuf::from("/host/data") && target == PathBuf::from("/container/data")
        ));
    }

    #[test]
    fn test_mount_location_bind_ro() {
        let mount = MountLocation::bind_ro("/host/artifacts", "/artifacts");
        match mount {
            MountLocation::Bind {
                source,
                target,
                read_only,
            } => {
                assert_eq!(source, PathBuf::from("/host/artifacts"));
                assert_eq!(target, PathBuf::from("/artifacts"));
                assert!(read_only);
            }
            _ => panic!("expected Bind variant"),
        }
    }

    #[test]
    fn test_mount_location_volume() {
        let mount = MountLocation::volume("my-volume");
        assert!(matches!(mount, MountLocation::Volume { name } if name == "my-volume"));
    }

    #[test]
    fn test_mount_location_serde() {
        let mount = MountLocation::bind_rw("/source", "/target");
        let json = serde_json::to_string(&mount).unwrap();
        assert!(json.contains("\"type\":\"bind\""));
        let parsed: MountLocation = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, MountLocation::Bind { .. }));
    }

    // ==================== Conversion tests ====================

    #[test]
    fn test_location_from_artifact_location() {
        use crate::provider::artifact_location::ArtifactLocation as LegacyArtifactLocation;

        // Test conversion from legacy ArtifactLocation to new Location
        let legacy = LegacyArtifactLocation::local("/artifacts/output.so");
        let new_loc: Location<ArtifactContext> = legacy.into();
        assert!(new_loc.is_local());
        assert_eq!(new_loc.path(), Some(&PathBuf::from("/artifacts/output.so")));

        let legacy = LegacyArtifactLocation::remote("https://cdn.example.com/output.so");
        let new_loc: Location<ArtifactContext> = legacy.into();
        assert!(new_loc.is_remote());

        let legacy = LegacyArtifactLocation::pre_baked("/opt/app/output.so");
        let new_loc: Location<ArtifactContext> = legacy.into();
        assert!(new_loc.is_pre_baked());
    }

    #[test]
    fn test_location_to_artifact_location() {
        use crate::provider::artifact_location::ArtifactLocation as LegacyArtifactLocation;

        // Test conversion from new Location to legacy ArtifactLocation
        let new_loc = Location::<ArtifactContext>::local("/artifacts/output.so");
        let legacy: LegacyArtifactLocation = new_loc.into();
        assert!(matches!(legacy, LegacyArtifactLocation::Local { .. }));

        let new_loc = Location::<ArtifactContext>::remote("https://cdn.example.com/output.so");
        let legacy: LegacyArtifactLocation = new_loc.into();
        assert!(matches!(legacy, LegacyArtifactLocation::Remote { .. }));

        let new_loc = Location::<ArtifactContext>::pre_baked("/opt/app/output.so");
        let legacy: LegacyArtifactLocation = new_loc.into();
        assert!(matches!(legacy, LegacyArtifactLocation::PreBakedInImage { .. }));
    }

    #[test]
    fn test_location_from_binary_ref() {
        use crate::provider::binary_ref::BinaryRef as LegacyBinaryRef;

        // Test conversion from legacy BinaryRef to new Location
        let legacy = LegacyBinaryRef::local("/usr/bin/binary");
        let new_loc: Location<BinaryContext> = legacy.into();
        assert!(new_loc.is_local());

        let legacy = LegacyBinaryRef::remote("https://example.com/binary");
        let new_loc: Location<BinaryContext> = legacy.into();
        assert!(new_loc.is_remote());
    }

    #[test]
    fn test_location_to_binary_ref() {
        use crate::provider::binary_ref::BinaryRef as LegacyBinaryRef;

        // Test conversion from new Location to legacy BinaryRef
        let new_loc = Location::<BinaryContext>::local("/usr/bin/binary");
        let legacy: LegacyBinaryRef = new_loc.into();
        assert!(matches!(legacy, LegacyBinaryRef::Local { .. }));

        let new_loc = Location::<BinaryContext>::remote("https://example.com/binary");
        let legacy: LegacyBinaryRef = new_loc.into();
        assert!(matches!(legacy, LegacyBinaryRef::Remote { .. }));

        // PreBaked in new Location converts to Local in legacy BinaryRef
        let new_loc = Location::<BinaryContext>::pre_baked("/prebaked/binary");
        let legacy: LegacyBinaryRef = new_loc.into();
        assert!(matches!(legacy, LegacyBinaryRef::Local { .. }));
    }
}