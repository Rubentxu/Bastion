//! Network backends — implementations of the `NetworkBackend` trait.

pub mod tap_backend;
pub mod host_backend;
pub mod bridge_backend;

pub use tap_backend::TapBackend;
pub use host_backend::HostBackend;
pub use bridge_backend::BridgeBackend;
