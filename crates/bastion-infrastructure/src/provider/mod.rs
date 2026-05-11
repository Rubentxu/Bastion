//! Provider adapters — implement SandboxProvider for each backend.

pub mod config;
pub mod docker;
pub mod factory;
pub mod firecracker;
pub mod gvisor;
pub mod local;
pub mod podman;
pub mod registry;
pub mod wasm;

pub mod network;
pub mod rootfs_manager;
pub use rootfs_manager::DefaultRootfsManager;

pub use config::{ProviderCapabilitiesConfig, ProviderConfig};
pub use docker::DockerProvider;
pub use factory::{ProviderFactory, ProviderInfo};
pub use firecracker::FirecrackerProvider;
pub use gvisor::GVisorProvider;
pub use local::LocalProvider;
pub use podman::PodmanProvider;
pub use registry::{ProviderRegistry, ProviderRegistryEntry, RegistryError};
pub use wasm::WasmSandboxProvider;
