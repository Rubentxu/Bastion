//! Network backends — implementations of the `NetworkBackend` trait.

pub mod bridge_backend;
pub mod host_backend;
pub mod tap_backend;

pub use bridge_backend::BridgeBackend;
pub use host_backend::HostBackend;
pub use tap_backend::TapBackend;
