//! Template and artifacts domain — capabilities, preconfigured templates, materialization.
//!
//! Defines the `TemplateArtifact` abstraction cross-provider: a versioned, verifiable
//! artifact that provides capabilities to sandboxes, independent of the underlying
//! technology (OCI container, microVM snapshot, Lambda layer, etc.).
//!
//! Each provider chooses how to materialize the artifact: bind mount, overlay,
//! lazy remote layer, snapshot restore, or fallback extract.

mod artifact;
mod catalog;
mod layer;
mod materializer;
mod store;
mod toolchain;

pub use artifact::{
    ArtifactId, ArtifactMediaType, ArtifactSecurityMetadata, CapabilityDescriptor,
    PreparedEnvironmentSpec, TemplateArtifact, ToolDescriptor, VerificationStep,
};
pub use catalog::{ArtifactCatalog, CatalogEntry};
pub use layer::{LayerArtifact, LayerStack, LayerStackError, LAYER_MOUNT_PREFIX, MAX_LAYERS_PER_FUNCTION};
pub use materializer::{
    MaterializationMode, MaterializationResult, ProviderKind, ProviderMaterializer,
};
pub use store::ArtifactStore;
pub use toolchain::{
    PreparedEnvironment, SupportLevel, ToolManagerAdapter, ToolResolver, ToolchainPlan,
    ToolchainRequest, ToolchainStep, ToolchainStrategy, ToolVerifyResult, ToolVerifyStep,
};
