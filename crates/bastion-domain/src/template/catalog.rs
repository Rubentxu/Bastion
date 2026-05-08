//! Artifact catalog — stores and resolves template artifacts.

use std::collections::HashMap;

use super::artifact::{ArtifactId, TemplateArtifact};
use crate::shared::DomainError;

/// An entry in the artifact catalog.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub artifact: TemplateArtifact,
    /// Whether this artifact is enabled (can be used for materialization).
    pub enabled: bool,
}

/// In-memory catalog of template artifacts.
///
/// In production, this would be backed by a file-based store or artifact registry.
#[derive(Debug, Default)]
pub struct ArtifactCatalog {
    entries: HashMap<ArtifactId, CatalogEntry>,
}

impl ArtifactCatalog {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a template artifact.
    pub fn register(&mut self, artifact: TemplateArtifact) {
        let id = artifact.id.clone();
        self.entries.insert(
            id,
            CatalogEntry {
                artifact,
                enabled: true,
            },
        );
    }

    /// Remove an artifact from the catalog by ID.
    pub fn remove(&mut self, id: &ArtifactId) -> Option<CatalogEntry> {
        self.entries.remove(id)
    }

    /// Look up an artifact by ID.
    pub fn get(&self, id: &ArtifactId) -> Option<&CatalogEntry> {
        self.entries.get(id)
    }

    /// Find artifacts that provide a specific capability.
    pub fn find_by_capability(&self, capability: &str) -> Vec<&CatalogEntry> {
        self.entries
            .values()
            .filter(|entry| {
                entry
                    .artifact
                    .capabilities
                    .iter()
                    .any(|c| c.name == capability)
            })
            .collect()
    }

    /// Resolve the best artifact for a capability.
    ///
    /// Current strategy: returns the first enabled artifact that provides the capability.
    /// Future: could consider version constraints, freshness, cache status, etc.
    pub fn resolve(&self, capability: &str) -> Result<&TemplateArtifact, DomainError> {
        let candidates: Vec<&CatalogEntry> = self
            .entries
            .values()
            .filter(|e| e.enabled && e.artifact.capabilities.iter().any(|c| c.name == capability))
            .collect();

        if candidates.is_empty() {
            return Err(DomainError::NotFound(format!(
                "No artifact found for capability '{}'",
                capability
            )));
        }

        // For now, return the first one. Future: version sorting / policy-based selection.
        Ok(&candidates[0].artifact)
    }

    /// List all registered artifacts (enabled or not).
    pub fn list_all(&self) -> Vec<&CatalogEntry> {
        self.entries.values().collect()
    }

    /// List all enabled artifacts.
    pub fn list_enabled(&self) -> Vec<&CatalogEntry> {
        self.entries.values().filter(|e| e.enabled).collect()
    }

    /// Number of registered artifacts.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::artifact::{
        ArtifactMediaType, CapabilityDescriptor, Category, TemplateArtifact, ToolDescriptor,
        VerificationStep,
    };

    fn make_jvm_artifact() -> TemplateArtifact {
        TemplateArtifact::builder("bastion/jvm-build", "v1")
            .media_type(ArtifactMediaType::OciArtifact)
            .digest("sha256:abc123")
            .add_capability(CapabilityDescriptor {
                name: "jvm-build".into(),
                tools: vec![
                    ToolDescriptor {
                        name: "java".into(),
                        version: "17".into(),
                        category: Category::Generic,
                        manager_preference: vec![],
                    },
                    ToolDescriptor {
                        name: "maven".into(),
                        version: "3.9".into(),
                        category: Category::Generic,
                        manager_preference: vec![],
                    },
                ],
                verification: vec![VerificationStep {
                    label: "java -version".into(),
                    command: "java -version".into(),
                    expected_exit_code: 0,
                    expected_output_contains: Some("openjdk".into()),
                }],
            })
            .build()
    }

    fn make_node_artifact() -> TemplateArtifact {
        TemplateArtifact::builder("bastion/node-build", "v1")
            .media_type(ArtifactMediaType::OciArtifact)
            .digest("sha256:def456")
            .add_capability(CapabilityDescriptor {
                name: "node-build".into(),
                tools: vec![
                    ToolDescriptor {
                        name: "node".into(),
                        version: "20".into(),
                        category: Category::Generic,
                        manager_preference: vec![],
                    },
                    ToolDescriptor {
                        name: "npm".into(),
                        version: "10".into(),
                        category: Category::Generic,
                        manager_preference: vec![],
                    },
                ],
                verification: vec![],
            })
            .build()
    }

    #[test]
    fn test_catalog_register_and_find() {
        let mut catalog = ArtifactCatalog::new();
        catalog.register(make_jvm_artifact());
        catalog.register(make_node_artifact());

        assert_eq!(catalog.len(), 2);

        let jvm_entries = catalog.find_by_capability("jvm-build");
        assert_eq!(jvm_entries.len(), 1);
        assert_eq!(jvm_entries[0].artifact.name, "bastion/jvm-build");

        let node_entries = catalog.find_by_capability("node-build");
        assert_eq!(node_entries.len(), 1);
    }

    #[test]
    fn test_catalog_resolve() {
        let mut catalog = ArtifactCatalog::new();
        catalog.register(make_jvm_artifact());

        let resolved = catalog.resolve("jvm-build").unwrap();
        assert_eq!(resolved.name, "bastion/jvm-build");
    }

    #[test]
    fn test_catalog_resolve_missing() {
        let catalog = ArtifactCatalog::new();
        let result = catalog.resolve("unknown-capability");
        assert!(result.is_err());
    }
}
