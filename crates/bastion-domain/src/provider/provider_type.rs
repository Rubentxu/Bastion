//! Provider type definitions — value objects for provider backend types.
//!
//! ProviderType is a VALUE OBJECT registered in ProviderTypeRegistry.
//! It is NOT persisted — only referenced by ProviderInstance.

use serde::{Deserialize, Serialize};

use super::capabilities::ProviderCapabilities;

/// Unique identifier for a provider type.
///
/// Value object — registered in ProviderTypeRegistry, NOT persisted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderTypeId(pub String);

impl ProviderTypeId {
    /// Create a new ProviderTypeId.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Create a ProviderTypeId from a static string (for convenience).
    pub fn from_static(id: &'static str) -> Self {
        Self(id.to_string())
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderTypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle model for a provider type.
///
/// Describes how the provider manages resource lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleModel {
    /// Container-based isolation (Docker/Podman)
    Container,
    /// Virtual machine isolation (Firecracker)
    VirtualMachine,
    /// WebAssembly sandbox
    Wasm,
    /// Serverless/function as a service
    Serverless,
    /// Direct host execution
    Local,
}

impl std::fmt::Display for LifecycleModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container => write!(f, "container"),
            Self::VirtualMachine => write!(f, "virtual_machine"),
            Self::Wasm => write!(f, "wasm"),
            Self::Serverless => write!(f, "serverless"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Provider type definition.
///
/// A value object describing the capabilities and characteristics of a
/// provider backend type (e.g., "podman", "firecracker").
///
/// Registered in ProviderTypeRegistry at startup. NOT persisted directly —
/// ProviderInstance references ProviderTypeId.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderType {
    /// Unique identifier for this provider type.
    pub id: ProviderTypeId,
    /// Capabilities supported by this provider type.
    pub capabilities: ProviderCapabilities,
    /// Lifecycle model for this provider type.
    pub lifecycle_model: LifecycleModel,
    /// Average startup time in milliseconds.
    pub avg_startup_ms: u32,
    /// Whether this provider requires KVM hardware virtualization.
    pub requires_kvm: bool,
    /// Whether this provider supports snapshots.
    pub supports_snapshots: bool,
}

impl ProviderType {
    /// Create a new ProviderType.
    pub fn new(
        id: ProviderTypeId,
        capabilities: ProviderCapabilities,
        lifecycle_model: LifecycleModel,
        avg_startup_ms: u32,
        requires_kvm: bool,
        supports_snapshots: bool,
    ) -> Self {
        Self {
            id,
            capabilities,
            lifecycle_model,
            avg_startup_ms,
            requires_kvm,
            supports_snapshots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_id_new() {
        let id = ProviderTypeId::new("podman");
        assert_eq!(id.as_str(), "podman");
    }

    #[test]
    fn test_provider_type_id_display() {
        let id = ProviderTypeId::new("firecracker");
        assert_eq!(format!("{}", id), "firecracker");
    }

    #[test]
    fn test_provider_type_id_equality() {
        let id1 = ProviderTypeId::new("podman");
        let id2 = ProviderTypeId::new("podman");
        let id3 = ProviderTypeId::new("docker");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_lifecycle_model_variants() {
        assert!(matches!(LifecycleModel::Container, LifecycleModel::Container));
        assert!(matches!(LifecycleModel::VirtualMachine, LifecycleModel::VirtualMachine));
        assert!(matches!(LifecycleModel::Wasm, LifecycleModel::Wasm));
        assert!(matches!(LifecycleModel::Serverless, LifecycleModel::Serverless));
        assert!(matches!(LifecycleModel::Local, LifecycleModel::Local));
    }

    #[test]
    fn test_lifecycle_model_display() {
        assert_eq!(format!("{}", LifecycleModel::Container), "container");
        assert_eq!(format!("{}", LifecycleModel::VirtualMachine), "virtual_machine");
        assert_eq!(format!("{}", LifecycleModel::Wasm), "wasm");
        assert_eq!(format!("{}", LifecycleModel::Serverless), "serverless");
        assert_eq!(format!("{}", LifecycleModel::Local), "local");
    }

    #[test]
    fn test_lifecycle_model_serde() {
        let model = LifecycleModel::Container;
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, "\"container\"");
        let parsed: LifecycleModel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, model);
    }

    #[test]
    fn test_provider_type_serde() {
        let provider_type = ProviderType::new(
            ProviderTypeId::new("podman"),
            ProviderCapabilities::default(),
            LifecycleModel::Container,
            1500,
            false,
            true,
        );
        let json = serde_json::to_string(&provider_type).unwrap();
        let parsed: ProviderType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id.as_str(), "podman");
        assert!(matches!(parsed.lifecycle_model, LifecycleModel::Container));
    }
}
