//! Provider bounded context — abstraction over sandbox backends.
//!
//! Defines the SandboxProvider port (trait) that infrastructure adapters implement.

pub mod capabilities;
pub mod port;

pub use capabilities::ProviderCapabilities;
pub use port::SandboxProvider;
