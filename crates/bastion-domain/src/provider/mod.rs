//! Provider bounded context — abstraction over sandbox backends.
//!
//! Defines the SandboxProvider port (trait) that infrastructure adapters implement.

pub mod capabilities;
pub mod port;
pub mod router;

#[cfg(feature = "use-segregated-traits")]
pub mod compat;
#[cfg(feature = "use-segregated-traits")]
pub mod executor;
#[cfg(feature = "use-segregated-traits")]
pub mod image_source;
#[cfg(feature = "use-segregated-traits")]
pub mod lifecycle;
#[cfg(feature = "use-segregated-traits")]
pub mod network;
#[cfg(feature = "use-segregated-traits")]
pub mod rootfs;
#[cfg(feature = "use-segregated-traits")]
pub mod state_machine;

pub use capabilities::ProviderCapabilities;
pub use port::SandboxProvider;
pub use router::CommandRouter;
