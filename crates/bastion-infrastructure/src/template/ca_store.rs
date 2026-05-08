//! Content-addressed store adapter for zero-copy tool provisioning.
//!
//! This adapter looks up tools in a content-addressed store and provides
//! hardlink-based provisioning to avoid copying large tool binaries.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use bastion_domain::shared::DomainError;
use bastion_domain::template::{
    ManagerType, SupportLevel, ToolManagerAdapter, ToolVerifyStep, ToolchainPlan, ToolchainRequest,
    ToolchainStep,
};
use sha2::{Digest, Sha256};

/// Content-addressed store adapter.
/// Provides zero-copy tool provisioning via hardlinks from a CA store.
pub struct CaStoreAdapter {
    /// Root directory of the content-addressed store.
    store_root: PathBuf,
    /// Directory where hardlinks to tools are created.
    provision_root: PathBuf,
}

impl CaStoreAdapter {
    pub fn new(store_root: impl Into<PathBuf>, provision_root: impl Into<PathBuf>) -> Self {
        Self {
            store_root: store_root.into(),
            provision_root: provision_root.into(),
        }
    }

    /// Look up a tool by its content hash.
    pub fn lookup(&self, hash: &str) -> Option<PathBuf> {
        let store_path = self.store_root.join(hash);
        if store_path.exists() {
            Some(store_path)
        } else {
            None
        }
    }

    /// Verify the checksum of an installed tool.
    pub fn verify(&self, path: &PathBuf, expected_hash: &str) -> Result<(), DomainError> {
        // Compute sha256 of the file
        let data = std::fs::read(path)
            .map_err(|e| DomainError::Internal(format!("Failed to read tool file: {}", e)))?;
        let computed = format!("sha256:{:x}", Sha256::digest(&data));

        if computed == expected_hash {
            Ok(())
        } else {
            Err(DomainError::Internal(format!(
                "checksum mismatch: expected {}, got {}",
                expected_hash, computed
            )))
        }
    }

    /// Create a hardlink from the CA store to the provision directory.
    fn create_hardlink(&self, hash: &str, tool_name: &str) -> Result<PathBuf, DomainError> {
        let store_path = self.store_root.join(hash);
        let provision_path = self.provision_root.join(tool_name);

        // Ensure provision directory exists
        if let Some(parent) = provision_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DomainError::Internal(format!("Failed to create provision dir: {}", e))
            })?;
        }

        // Create hardlink (fails if already exists)
        std::fs::hard_link(&store_path, &provision_path)
            .map_err(|e| DomainError::Internal(format!("Failed to create hardlink: {}", e)))?;

        Ok(provision_path)
    }
}

#[async_trait]
impl ToolManagerAdapter for CaStoreAdapter {
    fn id(&self) -> &'static str {
        "ca-store"
    }

    fn name(&self) -> &'static str {
        "Content-Addressed Store"
    }

    fn manager_type(&self) -> ManagerType {
        ManagerType::CaStore
    }

    fn supports(&self, req: &ToolchainRequest) -> SupportLevel {
        // Only support when using ContentAddressed strategy
        if req.strategy == bastion_domain::template::ToolchainStrategy::ContentAddressed {
            SupportLevel::Full
        } else {
            SupportLevel::None
        }
    }

    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError> {
        // For CA store, we need the tool hash from the request constraints
        let tool_hash = req
            .constraints
            .get("tool_hash")
            .cloned()
            .ok_or_else(|| DomainError::Validation("Missing tool_hash constraint".into()))?;

        let tool_name = req
            .constraints
            .get("tool_name")
            .cloned()
            .unwrap_or_else(|| req.capability.clone());

        // Verify tool exists in CA store
        let store_path = self.lookup(&tool_hash).ok_or_else(|| {
            DomainError::NotFound(format!("Tool {} not found in CA store", tool_hash))
        })?;

        // Create hardlink to provision directory
        let provision_path = self.create_hardlink(&tool_hash, &tool_name)?;

        // Compute expected hash for verification
        let data = std::fs::read(&store_path)
            .map_err(|e| DomainError::Internal(format!("Failed to read store file: {}", e)))?;
        let computed = format!("sha256:{:x}", Sha256::digest(&data));

        Ok(ToolchainPlan {
            capability: req.capability.clone(),
            adapter_used: "ca-store".into(),
            steps: vec![ToolchainStep {
                description: format!(
                    "Provision tool from CA store via hardlink to {}",
                    provision_path.display()
                ),
                command: format!("ln {} {}", store_path.display(), provision_path.display()),
                env: HashMap::new(),
                timeout_ms: 10_000,
                expected_exit_code: 0,
            }],
            verification: vec![ToolVerifyStep {
                label: format!("Verify {} checksum", tool_name),
                command: format!("sha256sum {}", provision_path.display()),
                expected_output_contains: Some(computed),
                expected_exit_code: 0,
            }],
            env: HashMap::new(),
            path_prefix: vec![
                provision_path
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn test_ca_store_adapter_lookup_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store_root = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();

        // Create a fake tool file
        let tool_path = store_root.join("sha256:abc123");
        std::fs::write(&tool_path, b"fake tool content").unwrap();

        let adapter = CaStoreAdapter::new(&store_root, temp_dir.path().join("provision"));
        let result = adapter.lookup("sha256:abc123");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), tool_path);
    }

    #[test]
    fn test_ca_store_adapter_lookup_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let adapter = CaStoreAdapter::new(
            temp_dir.path().join("store"),
            temp_dir.path().join("provision"),
        );
        let result = adapter.lookup("sha256:nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_ca_store_adapter_verify_correct_hash() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store_root = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();

        // Create a file with known content
        let content = b"hello world";
        let hash = format!("sha256:{:x}", Sha256::digest(content));
        let tool_path = store_root.join(&hash);
        std::fs::write(&tool_path, content).unwrap();

        let adapter = CaStoreAdapter::new(&store_root, temp_dir.path().join("provision"));
        let result = adapter.verify(&tool_path, &hash);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ca_store_adapter_verify_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store_root = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();

        // Create a file
        let tool_path = store_root.join("sha256:test");
        std::fs::write(&tool_path, b"some content").unwrap();

        let adapter = CaStoreAdapter::new(&store_root, temp_dir.path().join("provision"));
        let result = adapter.verify(&tool_path, "sha256:wronghash");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
    }

    #[test]
    fn test_supports_content_addressed_strategy() {
        let temp_dir = tempfile::tempdir().unwrap();
        let adapter = CaStoreAdapter::new(
            temp_dir.path().join("store"),
            temp_dir.path().join("provision"),
        );

        let ca_req = ToolchainRequest {
            sandbox_id: bastion_domain::shared::id::SandboxId::new("test"),
            capability: "jvm-build".into(),
            constraints: HashMap::new(),
            strategy: bastion_domain::template::ToolchainStrategy::ContentAddressed,
        };
        assert_eq!(adapter.supports(&ca_req), SupportLevel::Full);

        let auto_req = ToolchainRequest {
            sandbox_id: bastion_domain::shared::id::SandboxId::new("test"),
            capability: "jvm-build".into(),
            constraints: HashMap::new(),
            strategy: bastion_domain::template::ToolchainStrategy::Auto,
        };
        assert_eq!(adapter.supports(&auto_req), SupportLevel::None);
    }
}
