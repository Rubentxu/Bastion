//! Provider bounded context — abstraction over sandbox backends.
//!
//! Defines the SandboxProvider port (trait) that infrastructure adapters implement.

pub mod capabilities;
pub mod port;
pub mod router;

// ── New type modules (Phase 1) ──────────────────────────────────────────────

pub mod artifact_location;
pub mod binary_ref;
pub mod instance;
pub mod instance_config;
pub mod instance_constraints;
pub mod image_reference;
pub mod mount_ref;
pub mod provider_type;
pub mod instance_repository;
pub mod secret_source;
pub mod socket_ref;
pub mod type_registry;
pub mod volume_source;
pub mod worker_binary_source;

// ── Legacy modules ─────────────────────────────────────────────────────────────

pub mod compat;
pub mod executor;
pub mod image_source;
pub mod lifecycle;
pub mod network;
pub mod rootfs;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub use artifact_location::ArtifactLocation;
pub use binary_ref::BinaryRef;
pub use capabilities::ProviderCapabilities;
pub use image_reference::{
    ImagePullPolicy, ImageReference, Platform, RegistryCredentials, SecretValue, SecretValueKind,
};
pub use instance::{ProviderInstance, ProviderInstanceId, ProviderInstanceStatus};
pub use instance_repository::{Error, ProviderInstanceRepository, Result};
pub use instance_config::{
    AwsCredentials, AwsCredentialsKind, KubernetesCredentials, K8sCredentialsKind,
    ProviderInstanceConfig, WasmRuntime, WasmRuntimeKind,
};
pub use instance_constraints::InstanceConstraints;
pub use mount_ref::{ContainerNetworkMode, MountRef};
pub use port::SandboxProvider;
pub use router::CommandRouter;
pub use secret_source::SecretSource;
pub use socket_ref::SocketRef;
pub use provider_type::{LifecycleModel, ProviderType, ProviderTypeId};
pub use type_registry::ProviderTypeRegistry;
pub use volume_source::{HostPathType, VolumeMedium, VolumeSource};
pub use worker_binary_source::{ModuleRef, WorkerBinarySource};
