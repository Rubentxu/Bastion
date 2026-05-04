//! Provider adapters — implement SandboxProvider for each backend.

pub mod docker;
pub mod factory;
pub mod firecracker;
pub mod gvisor;
pub mod podman;

pub use docker::DockerProvider;
pub use factory::{ProviderFactory, ProviderInfo};
pub use firecracker::FirecrackerProvider;
pub use gvisor::GVisorProvider;
pub use podman::PodmanProvider;
