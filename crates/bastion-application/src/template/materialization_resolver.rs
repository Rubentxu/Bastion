//! Materialization strategy resolver.
//!
//! Extracts strategy resolution logic from gateway server.rs,
//! providing a dedicated resolver for materialization strategies.

use bastion_domain::template::{
    ArtifactCatalog, MaterializationMode, ProviderKind,
};

/// Resolution result containing the selected materializer and mode.
pub struct StrategyResolution {
    /// The materializer to use.
    pub materializer_name: String,
    /// The materialization mode to use.
    pub mode: MaterializationMode,
    /// Whether to skip ToolResolver fallback.
    pub use_tool_resolver_fallback: bool,
}

/// MaterializationStrategyResolver resolves the best materialization strategy
/// based on provider kind and artifact characteristics.
pub struct MaterializationStrategyResolver;

impl MaterializationStrategyResolver {
    /// Resolve the best materialization strategy for the given capability and provider.
    ///
    /// Priority chain:
    /// 1. If artifact exists in catalog → use PodmanOptimizedMaterializer with MountReadonly
    /// 2. If Kubernetes provider → use materializer with Extract mode
    /// 3. Otherwise → use ToolResolver fallback (AptAdapter + AsdfAdapter)
    pub fn resolve(
        capability: &str,
        provider_kind: ProviderKind,
        artifact_catalog: &ArtifactCatalog,
    ) -> StrategyResolution {
        // Try artifact catalog first
        let artifact = artifact_catalog.resolve(capability);

        if artifact.is_ok() {
            // Artifact found in catalog
            let mode = match provider_kind {
                ProviderKind::Podman | ProviderKind::Docker => MaterializationMode::MountReadonly,
                ProviderKind::Kubernetes | ProviderKind::Firecracker => MaterializationMode::Extract,
                _ => MaterializationMode::Auto,
            };

            return StrategyResolution {
                materializer_name: "PodmanOptimizedMaterializer".to_string(),
                mode,
                use_tool_resolver_fallback: false,
            };
        }

        // No artifact found - determine mode for fallback to ToolResolver
        let mode = match provider_kind {
            ProviderKind::Kubernetes => MaterializationMode::Extract,
            _ => MaterializationMode::Auto,
        };

        StrategyResolution {
            materializer_name: "ToolResolver".to_string(),
            mode,
            use_tool_resolver_fallback: true,
        }
    }

    /// Check if a provider is local (supports mount operations).
    pub fn is_local_provider(provider_kind: ProviderKind) -> bool {
        matches!(
            provider_kind,
            ProviderKind::Podman | ProviderKind::Docker | ProviderKind::Local
        )
    }

    /// Get the preferred sync backend for a provider.
    pub fn preferred_sync_backend(provider_kind: ProviderKind) -> &'static str {
        if Self::is_local_provider(provider_kind) {
            "rsync"
        } else {
            "tar_stream"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::template::{
        ArtifactCatalog, TemplateArtifact, ArtifactMediaType, CapabilityDescriptor,
        Category, ToolDescriptor,
    };

    fn make_jvm_artifact() -> TemplateArtifact {
        TemplateArtifact::builder("test/jvm-build", "v1")
            .media_type(ArtifactMediaType::RootfsTar)
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
                ],
                verification: vec![],
            })
            .build()
    }

    #[test]
    fn test_resolve_with_artifact_podman_mount_readonly() {
        let mut catalog = ArtifactCatalog::new();
        catalog.register(make_jvm_artifact());

        let resolution = MaterializationStrategyResolver::resolve(
            "jvm-build",
            ProviderKind::Podman,
            &catalog,
        );

        assert_eq!(resolution.materializer_name, "PodmanOptimizedMaterializer");
        assert_eq!(resolution.mode, MaterializationMode::MountReadonly);
        assert!(!resolution.use_tool_resolver_fallback);
    }

    #[test]
    fn test_resolve_with_artifact_kubernetes_extract_mode() {
        let mut catalog = ArtifactCatalog::new();
        catalog.register(make_jvm_artifact());

        let resolution = MaterializationStrategyResolver::resolve(
            "jvm-build",
            ProviderKind::Kubernetes,
            &catalog,
        );

        assert_eq!(resolution.materializer_name, "PodmanOptimizedMaterializer");
        assert_eq!(resolution.mode, MaterializationMode::Extract);
    }

    #[test]
    fn test_resolve_no_artifact_falls_back_to_tool_resolver() {
        let catalog = ArtifactCatalog::new(); // Empty catalog
        let resolution = MaterializationStrategyResolver::resolve(
            "python-build",
            ProviderKind::Custom,
            &catalog,
        );

        assert_eq!(resolution.materializer_name, "ToolResolver");
        assert!(resolution.use_tool_resolver_fallback);
    }

    #[test]
    fn test_resolve_kubernetes_no_artifact_extract_mode() {
        let catalog = ArtifactCatalog::new(); // Empty catalog
        let resolution = MaterializationStrategyResolver::resolve(
            "python-build",
            ProviderKind::Kubernetes,
            &catalog,
        );

        // Even with no artifact, Kubernetes uses Extract mode for ToolResolver
        assert_eq!(resolution.materializer_name, "ToolResolver");
        assert_eq!(resolution.mode, MaterializationMode::Extract);
        assert!(resolution.use_tool_resolver_fallback);
    }

    #[test]
    fn test_is_local_provider() {
        assert!(MaterializationStrategyResolver::is_local_provider(ProviderKind::Podman));
        assert!(MaterializationStrategyResolver::is_local_provider(ProviderKind::Docker));
        assert!(MaterializationStrategyResolver::is_local_provider(ProviderKind::Local));
        assert!(!MaterializationStrategyResolver::is_local_provider(ProviderKind::Kubernetes));
        assert!(!MaterializationStrategyResolver::is_local_provider(ProviderKind::Firecracker));
        assert!(!MaterializationStrategyResolver::is_local_provider(ProviderKind::Wasm));
    }

    #[test]
    fn test_provider_kind_display() {
        use bastion_domain::template::ProviderKind;
        assert_eq!(format!("{}", ProviderKind::Podman), "podman");
        assert_eq!(format!("{}", ProviderKind::Docker), "docker");
        assert_eq!(format!("{}", ProviderKind::Local), "local");
        assert_eq!(format!("{}", ProviderKind::Wasm), "wasm");
    }

    #[test]
    fn test_preferred_sync_backend_local() {
        assert_eq!(
            MaterializationStrategyResolver::preferred_sync_backend(ProviderKind::Podman),
            "rsync"
        );
    }

    #[test]
    fn test_preferred_sync_backend_remote() {
        assert_eq!(
            MaterializationStrategyResolver::preferred_sync_backend(ProviderKind::Kubernetes),
            "tar_stream"
        );
    }
}
