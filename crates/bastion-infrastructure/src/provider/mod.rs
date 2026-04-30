//! Provider adapters — implement SandboxProvider for each backend.

pub mod factory;
pub mod podman;

pub use factory::{ProviderFactory, ProviderInfo};
pub use podman::PodmanProvider;
