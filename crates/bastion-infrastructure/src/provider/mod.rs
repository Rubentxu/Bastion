//! Provider adapters — implement SandboxProvider for each backend.

pub mod podman;

pub use podman::PodmanProvider;
