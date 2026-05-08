//! Template materialization infrastructure.
//!
//! Provides concrete implementations of the `ProviderMaterializer` trait
//! for various backends (universal, Podman-optimized, etc.).

pub mod adapters;
pub mod ca_store;
pub mod capability_config;
pub mod capability_registry;
pub mod layer;
pub mod podman;
pub mod snapshot;
pub mod store;
pub mod sync;
pub mod universal;

pub use adapters::{AptAdapter, AsdfAdapter, SdkmanAdapter};
pub use ca_store::CaStoreAdapter;
pub use capability_config::{CapabilityConfig, ToolVerifyStepDef, ToolchainDef, ToolchainStepDef};
pub use capability_registry::{CapabilityRegistry, CapabilityRegistryError};
pub use layer::ZipLayerMaterializer;
pub use podman::PodmanOptimizedMaterializer;
pub use snapshot::SnapshotManager;
pub use store::FsArtifactStore;
pub use sync::{DeltaSyncBackend, RsyncBackend, TarStreamBackend};
pub use universal::UniversalMaterializer;
