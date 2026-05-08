//! Image source trait — manages container image retrieval.
//!
//! Provides a unified abstraction for different image formats:
//! - OCI image layouts (directory with rootfs)
//! - Squashfs disk images
//! - WebAssembly modules

use std::path::PathBuf;

use async_trait::async_trait;

use crate::shared::DomainError;

/// Kind of image source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// OCI image layout (directory with rootfs)
    Oci,
    /// Squashfs disk image
    Squashfs,
    /// WebAssembly module
    Wasm,
}

/// Configuration for an image source.
#[derive(Debug, Clone)]
pub struct ImageSourceConfig {
    /// The kind of image.
    pub kind: ImageKind,
    /// Path to the image (directory for OCI, file for Squashfs/Wasm).
    pub path: PathBuf,
    /// Whether the image is read-only.
    pub read_only: bool,
}

/// Image source trait — abstracts over different image formats.
///
/// Implementations validate and provide access to root filesystems
/// for sandbox providers (gVisor, Firecracker, etc.).
#[cfg(feature = "use-segregated-traits")]
#[async_trait]
pub trait ImageSource: Send + Sync + std::fmt::Debug {
    /// Validate that the image exists and is usable.
    async fn validate(&self) -> Result<(), DomainError>;

    /// Get the rootfs path for this image.
    fn rootfs_path(&self) -> PathBuf;

    /// Get the kind of this image source.
    fn kind(&self) -> ImageKind;

    /// Get the read_only flag.
    fn read_only(&self) -> bool {
        self.config().read_only
    }

    /// Get the config.
    fn config(&self) -> &ImageSourceConfig;
}

// =============================================================================
// OCI Image
// =============================================================================

/// OCI image layout source (directory-based rootfs).
///
/// The path must be a directory containing a valid OCI rootfs with typical
/// Linux filesystem structure (/bin, /lib, etc.).
#[derive(Debug)]
pub struct OciImage {
    config: ImageSourceConfig,
}

impl OciImage {
    /// Create a new OCI image source.
    pub fn new(path: PathBuf, read_only: bool) -> Self {
        Self {
            config: ImageSourceConfig {
                kind: ImageKind::Oci,
                path,
                read_only,
            },
        }
    }
}

#[async_trait]
impl ImageSource for OciImage {
    async fn validate(&self) -> Result<(), DomainError> {
        // Check path exists and is a directory
        let metadata = tokio::fs::metadata(&self.config.path).await.map_err(|e| {
            DomainError::Config(format!("OCI rootfs not found: {}", e))
        })?;
        if !metadata.is_dir() {
            return Err(DomainError::Config(
                "OCI rootfs is not a directory".into(),
            ));
        }
        // Check for rootfs subdirectory or presence of /bin
        // This is a heuristic - missing /bin is not an error, just unusual
        let has_bin = tokio::fs::metadata(self.config.path.join("bin"))
            .await
            .is_ok();
        if !has_bin {
            // OCI rootfs without /bin may be unusual but not invalid
            // The warning will be emitted by the caller if needed
        }
        Ok(())
    }

    fn rootfs_path(&self) -> PathBuf {
        self.config.path.clone()
    }

    fn kind(&self) -> ImageKind {
        ImageKind::Oci
    }

    fn config(&self) -> &ImageSourceConfig {
        &self.config
    }
}

// =============================================================================
// Squashfs Image
// =============================================================================

/// Squashfs disk image source.
///
/// Squashfs is a read-only compressed filesystem commonly used in live CDs
/// and container rootfs images.
#[derive(Debug)]
pub struct SquashfsImage {
    config: ImageSourceConfig,
}

impl SquashfsImage {
    /// Create a new Squashfs image source.
    ///
    /// Squashfs images are always read-only.
    pub fn new(path: PathBuf) -> Self {
        Self {
            config: ImageSourceConfig {
                kind: ImageKind::Squashfs,
                path,
                read_only: true, // Squashfs is always read-only
            },
        }
    }
}

/// Squashfs magic bytes: "hsqs" (0x68737173)
const SQUASHFS_MAGIC: &[u8] = b"hsqs";

#[async_trait]
impl ImageSource for SquashfsImage {
    async fn validate(&self) -> Result<(), DomainError> {
        // Check file exists
        let metadata =
            tokio::fs::metadata(&self.config.path)
                .await
                .map_err(|e| {
                    DomainError::Config(format!("Squashfs image not found: {}", e))
                })?;
        if !metadata.is_file() {
            return Err(DomainError::Config(
                "Squashfs path is not a file".into(),
            ));
        }
        // Magic byte check — read first 4 bytes
        let mut file = tokio::fs::File::open(&self.config.path).await?;
        let mut magic = [0u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut file, &mut magic).await?;
        if &magic != SQUASHFS_MAGIC {
            return Err(DomainError::Config(format!(
                "Not a valid squashfs image: magic bytes {:02x?}",
                magic
            ))
            .into());
        }
        Ok(())
    }

    fn rootfs_path(&self) -> PathBuf {
        self.config.path.clone()
    }

    fn kind(&self) -> ImageKind {
        ImageKind::Squashfs
    }

    fn config(&self) -> &ImageSourceConfig {
        &self.config
    }

    fn read_only(&self) -> bool {
        true // Always read-only
    }
}

// =============================================================================
// Wasm Bundle
// =============================================================================

/// WebAssembly module bundle source.
///
/// Wasm modules are self-contained executables that run in a Wasm runtime.
#[derive(Debug)]
pub struct WasmBundle {
    config: ImageSourceConfig,
}

impl WasmBundle {
    /// Create a new Wasm bundle source.
    ///
    /// Wasm bundles are always read-only.
    pub fn new(path: PathBuf) -> Self {
        Self {
            config: ImageSourceConfig {
                kind: ImageKind::Wasm,
                path,
                read_only: true,
            },
        }
    }
}

/// Wasm magic bytes: "\0asm" at bytes 0-3
const WASM_MAGIC: &[u8] = b"\0asm";

#[async_trait]
impl ImageSource for WasmBundle {
    async fn validate(&self) -> Result<(), DomainError> {
        let metadata = tokio::fs::metadata(&self.config.path).await.map_err(|e| {
            DomainError::Config(format!("WASM bundle not found: {}", e))
        })?;
        if !metadata.is_file() {
            return Err(DomainError::Config("WASM bundle is not a file".into()));
        }
        // Magic byte check
        let mut file = tokio::fs::File::open(&self.config.path).await?;
        let mut magic = [0u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut file, &mut magic).await?;
        if &magic != WASM_MAGIC {
            return Err(DomainError::Config(format!(
                "Not a valid WASM binary: magic bytes {:02x?}",
                magic
            ))
            .into());
        }
        Ok(())
    }

    fn rootfs_path(&self) -> PathBuf {
        self.config.path.clone()
    }

    fn kind(&self) -> ImageKind {
        ImageKind::Wasm
    }

    fn config(&self) -> &ImageSourceConfig {
        &self.config
    }
}
