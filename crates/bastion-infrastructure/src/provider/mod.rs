//! Provider adapters — implement SandboxProvider for each backend.

pub mod config;
pub mod docker;
pub mod factory;
pub mod firecracker;
pub mod gvisor;
pub mod podman;
pub mod registry;

pub use config::{ProviderConfig, ProviderCapabilitiesConfig};
pub use docker::DockerProvider;
pub use factory::{ProviderFactory, ProviderInfo};
pub use firecracker::FirecrackerProvider;
pub use gvisor::GVisorProvider;
pub use podman::PodmanProvider;
pub use registry::{ProviderRegistry, ProviderRegistryEntry, RegistryError};
