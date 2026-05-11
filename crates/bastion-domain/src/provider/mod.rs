//! Provider bounded context — abstraction over sandbox backends.
//!
//! Defines the SandboxProvider port (trait) that infrastructure adapters implement.

pub mod capabilities;
pub mod port;
pub mod router;

pub mod compat;
pub mod executor;
pub mod image_source;
pub mod lifecycle;
pub mod network;
pub mod rootfs;
pub mod state_machine;

pub use capabilities::ProviderCapabilities;
pub use port::SandboxProvider;
pub use router::CommandRouter;
