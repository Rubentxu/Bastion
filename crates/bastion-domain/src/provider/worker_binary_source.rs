//! Worker binary source types.
//!
//! Describes where the worker binary comes from and how it's provided.

use serde::{Deserialize, Serialize};

use super::image_reference::ImageReference;

/// Reference to a WebAssembly module.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModuleRef {
    /// Module stored locally on the filesystem.
    Local {
        /// Path to the WASM module file.
        path: String,
    },
    /// Module stored remotely and downloaded on demand.
    Remote {
        /// URL to download the module from.
        url: String,
        /// Expected SHA256 checksum.
        checksum: Option<String>,
    },
    /// Module stored in an OCI registry.
    Oci {
        /// Reference to the OCI image.
        image: ImageReference,
        /// Path to the module within the image.
        path: String,
    },
}

impl ModuleRef {
    /// Create a local module reference.
    pub fn local(path: impl Into<String>) -> Self {
        Self::Local { path: path.into() }
    }

    /// Create a remote module reference.
    pub fn remote(url: impl Into<String>) -> Self {
        Self::Remote {
            url: url.into(),
            checksum: None,
        }
    }
}

/// Source of the worker binary.
///
/// Describes how the bastion-worker binary is provided to the sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerBinarySource {
    /// Worker binary via bind mount from host filesystem.
    BindMount {
        /// Path on the host to bind mount.
        path: String,
    },
    /// Worker binary pre-baked into the runtime image.
    PreBaked {
        /// Optional path within the image (defaults to standard location).
        path: Option<String>,
    },
    /// Worker binary downloaded from a remote URL.
    Download {
        /// URL to download the binary from.
        url: String,
        /// Version constraint (semver).
        version_constraint: Option<String>,
        /// Expected SHA256 checksum.
        checksum: Option<String>,
        /// Directory to cache the downloaded binary.
        cache_dir: Option<String>,
    },
    /// Worker binary compiled to WebAssembly.
    CompiledToWasm {
        /// Reference to the WASM module.
        module: ModuleRef,
    },
    /// Worker binary from a sidecar container image.
    Sidecar {
        /// Image reference for the sidecar.
        image: ImageReference,
        /// Optional path where the binary is mounted.
        mount_path: Option<String>,
    },
    /// Worker binary from an AWS Lambda layer.
    LambdaLayer {
        /// ARN of the Lambda layer.
        arn: String,
        /// Optional layer version.
        version: Option<String>,
    },
}

impl WorkerBinarySource {
    /// Create a bind mount worker binary source.
    pub fn bind_mount(path: impl Into<String>) -> Self {
        Self::BindMount { path: path.into() }
    }

    /// Create a pre-baked worker binary source.
    pub fn pre_baked() -> Self {
        Self::PreBaked { path: None }
    }

    /// Create a pre-baked worker binary source with a path.
    pub fn pre_baked_with_path(path: impl Into<String>) -> Self {
        Self::PreBaked {
            path: Some(path.into()),
        }
    }

    /// Create a download worker binary source.
    pub fn download(url: impl Into<String>) -> Self {
        Self::Download {
            url: url.into(),
            version_constraint: None,
            checksum: None,
            cache_dir: None,
        }
    }

    /// Create a WASM-compiled worker binary source.
    pub fn wasm(module: ModuleRef) -> Self {
        Self::CompiledToWasm { module }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_ref_local() {
        let module = ModuleRef::local("/path/to/module.wasm");
        assert!(matches!(module, ModuleRef::Local { .. }));
    }

    #[test]
    fn test_module_ref_remote() {
        let module = ModuleRef::remote("https://example.com/module.wasm");
        assert!(matches!(module, ModuleRef::Remote { .. }));
    }

    #[test]
    fn test_worker_binary_source_bind_mount() {
        let source = WorkerBinarySource::bind_mount("/usr/local/bin/bastion-worker");
        assert!(matches!(source, WorkerBinarySource::BindMount { .. }));
    }

    #[test]
    fn test_worker_binary_source_pre_baked() {
        let source = WorkerBinarySource::pre_baked();
        assert!(matches!(source, WorkerBinarySource::PreBaked { .. }));
    }

    #[test]
    fn test_worker_binary_source_download() {
        let source = WorkerBinarySource::download("https://example.com/bastion-worker");
        assert!(matches!(source, WorkerBinarySource::Download { .. }));
    }

    #[test]
    fn test_worker_binary_source_wasm() {
        let module = ModuleRef::local("/path/to/worker.wasm");
        let source = WorkerBinarySource::wasm(module);
        assert!(matches!(source, WorkerBinarySource::CompiledToWasm { .. }));
    }

    #[test]
    fn test_worker_binary_source_serde_bind_mount() {
        let source = WorkerBinarySource::bind_mount("/usr/bin/worker");
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"bind_mount\""));
        let parsed: WorkerBinarySource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, WorkerBinarySource::BindMount { .. }));
    }

    #[test]
    fn test_worker_binary_source_serde_download() {
        let source = WorkerBinarySource::download("https://example.com/worker");
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"download\""));
        let parsed: WorkerBinarySource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, WorkerBinarySource::Download { .. }));
    }

    #[test]
    fn test_worker_binary_source_serde_pre_baked() {
        let source = WorkerBinarySource::pre_baked();
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"pre_baked\""));
        let parsed: WorkerBinarySource = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, WorkerBinarySource::PreBaked { .. }));
    }
}
