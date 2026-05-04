//! Template materialization infrastructure.
//!
//! Provides concrete implementations of the `ProviderMaterializer` trait
//! for various backends (universal, Podman-optimized, etc.).

pub mod adapters;
pub mod layer;
pub mod podman;
pub mod snapshot;
pub mod store;
pub mod universal;

pub use adapters::{AptAdapter, AsdfAdapter, SdkmanAdapter};
pub use layer::ZipLayerMaterializer;
pub use podman::PodmanOptimizedMaterializer;
pub use snapshot::SnapshotManager;
pub use store::FsArtifactStore;
pub use universal::UniversalMaterializer;
