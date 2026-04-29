//! Provider bounded context — abstraction over sandbox backends.
//!
//! Defines the SandboxProvider port (trait) that infrastructure adapters implement.

pub mod port;
pub mod capabilities;

pub use port::SandboxProvider;
pub use capabilities::ProviderCapabilities;
