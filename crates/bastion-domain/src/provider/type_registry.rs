//! Provider type registry.
//!
//! Registry for builtin provider types that can be instantiated.

use super::{LifecycleModel, ProviderCapabilities, ProviderType, ProviderTypeId};

/// Registry for provider types.
///
/// Maintains a collection of available provider types (builtin types).
pub struct ProviderTypeRegistry {
    types: std::collections::HashMap<ProviderTypeId, ProviderType>,
}

impl ProviderTypeRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            types: std::collections::HashMap::new(),
        }
    }

    /// Create a registry pre-populated with builtin provider types.
    pub fn with_builtin_types() -> Self {
        let mut registry = Self::new();
        registry.register(podman_type());
        registry.register(docker_type());
        registry.register(gvisor_type());
        registry.register(firecracker_type());
        registry.register(wasm_type());
        registry.register(local_type());
        registry.register(kubernetes_type());
        registry.register(lambda_type());
        registry
    }

    /// Get a provider type by ID.
    pub fn get(&self, id: &ProviderTypeId) -> Option<&ProviderType> {
        self.types.get(id)
    }

    /// Register a new provider type.
    pub fn register(&mut self, provider_type: ProviderType) {
        self.types.insert(provider_type.id.clone(), provider_type);
    }

    /// List all registered provider types.
    pub fn list_types(&self) -> Vec<&ProviderType> {
        self.types.values().collect()
    }
}

impl Default for ProviderTypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Builtin Provider Type Constructors ────────────────────────────────────────

fn container_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::try_new(
        true,  // supports_snapshots
        true,  // supports_streaming
        true,  // supports_pause_resume
        86_400_000,  // max_timeout_ms
        16_384,  // max_memory_mb
        16,  // max_cpu_count
        true,  // supports_networking
        false, // requires_kvm
        1500,  // avg_startup_ms
    )
    .expect("container capabilities should be valid")
}

fn vm_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::try_new(
        true,  // supports_snapshots
        true,  // supports_streaming
        true,  // supports_pause_resume
        86_400_000,  // max_timeout_ms
        65_536,  // max_memory_mb
        32,  // max_cpu_count
        true,  // supports_networking
        true,  // requires_kvm
        500,   // avg_startup_ms
    )
    .expect("vm capabilities should be valid")
}

fn wasm_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::try_new(
        false, // supports_snapshots
        true,  // supports_streaming
        false, // supports_pause_resume
        300_000,  // max_timeout_ms
        512,   // max_memory_mb
        4,     // max_cpu_count
        false, // supports_networking
        false, // requires_kvm
        50,    // avg_startup_ms
    )
    .expect("wasm capabilities should be valid")
}

fn local_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::try_new(
        false, // supports_snapshots
        true,  // supports_streaming
        false, // supports_pause_resume
        86_400_000,  // max_timeout_ms
        65_536,  // max_memory_mb
        64,     // max_cpu_count
        true,   // supports_networking
        false,  // requires_kvm
        1,      // avg_startup_ms (must be > 0)
    )
    .expect("local capabilities should be valid")
}

fn serverless_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::try_new(
        false, // supports_snapshots
        true,  // supports_streaming
        false, // supports_pause_resume
        900_000,  // max_timeout_ms
        3_008,  // max_memory_mb
        2,     // max_cpu_count
        true,  // supports_networking
        false, // requires_kvm
        5000,  // avg_startup_ms
    )
    .expect("serverless capabilities should be valid")
}

/// Create the Podman provider type.
pub fn podman_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("podman"),
        container_capabilities(),
        LifecycleModel::Container,
        1500,
        false,
        true,
    )
}

/// Create the Docker provider type.
pub fn docker_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("docker"),
        container_capabilities(),
        LifecycleModel::Container,
        2000,
        false,
        true,
    )
}

/// Create the gVisor provider type.
pub fn gvisor_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("gvisor"),
        container_capabilities(),
        LifecycleModel::Container,
        800,
        true,
        false,
    )
}

/// Create the Firecracker provider type.
pub fn firecracker_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("firecracker"),
        vm_capabilities(),
        LifecycleModel::VirtualMachine,
        500,
        true,
        true,
    )
}

/// Create the Wasm provider type.
pub fn wasm_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("wasm"),
        wasm_capabilities(),
        LifecycleModel::Wasm,
        50,
        false,
        false,
    )
}

/// Create the Local provider type.
pub fn local_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("local"),
        local_capabilities(),
        LifecycleModel::Local,
        0,
        false,
        false,
    )
}

/// Create the Kubernetes provider type.
pub fn kubernetes_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("kubernetes"),
        serverless_capabilities(),
        LifecycleModel::Serverless,
        5000,
        false,
        false,
    )
}

/// Create the Lambda provider type.
pub fn lambda_type() -> ProviderType {
    ProviderType::new(
        ProviderTypeId::from_static("lambda"),
        serverless_capabilities(),
        LifecycleModel::Serverless,
        1000,
        false,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = ProviderTypeRegistry::new();
        assert!(registry.list_types().is_empty());
    }

    #[test]
    fn test_registry_with_builtin_types() {
        let registry = ProviderTypeRegistry::with_builtin_types();
        let types = registry.list_types();
        assert_eq!(types.len(), 8);
    }

    #[test]
    fn test_registry_get() {
        let registry = ProviderTypeRegistry::with_builtin_types();
        let podman = registry.get(&ProviderTypeId::new("podman"));
        assert!(podman.is_some());
        assert_eq!(podman.unwrap().id.as_str(), "podman");
    }

    #[test]
    fn test_registry_get_not_found() {
        let registry = ProviderTypeRegistry::new();
        let result = registry.get(&ProviderTypeId::new("nonexistent"));
        assert!(result.is_none());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = ProviderTypeRegistry::new();
        registry.register(podman_type());
        assert!(registry.get(&ProviderTypeId::new("podman")).is_some());
    }

    #[test]
    fn test_registry_list_types() {
        let mut registry = ProviderTypeRegistry::new();
        registry.register(podman_type());
        registry.register(docker_type());
        let types = registry.list_types();
        assert_eq!(types.len(), 2);
    }

    #[test]
    fn test_builtin_types_have_correct_ids() {
        assert_eq!(podman_type().id.as_str(), "podman");
        assert_eq!(docker_type().id.as_str(), "docker");
        assert_eq!(gvisor_type().id.as_str(), "gvisor");
        assert_eq!(firecracker_type().id.as_str(), "firecracker");
        assert_eq!(wasm_type().id.as_str(), "wasm");
        assert_eq!(local_type().id.as_str(), "local");
        assert_eq!(kubernetes_type().id.as_str(), "kubernetes");
        assert_eq!(lambda_type().id.as_str(), "lambda");
    }

    #[test]
    fn test_builtin_types_have_lifecycle_models() {
        assert!(matches!(podman_type().lifecycle_model, LifecycleModel::Container));
        assert!(matches!(docker_type().lifecycle_model, LifecycleModel::Container));
        assert!(matches!(gvisor_type().lifecycle_model, LifecycleModel::Container));
        assert!(matches!(firecracker_type().lifecycle_model, LifecycleModel::VirtualMachine));
        assert!(matches!(wasm_type().lifecycle_model, LifecycleModel::Wasm));
        assert!(matches!(local_type().lifecycle_model, LifecycleModel::Local));
        assert!(matches!(kubernetes_type().lifecycle_model, LifecycleModel::Serverless));
        assert!(matches!(lambda_type().lifecycle_model, LifecycleModel::Serverless));
    }
}
