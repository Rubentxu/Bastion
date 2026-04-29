//! Shared kernel — common types used across bounded contexts.

pub mod error;
pub mod id;

pub use error::DomainError;
pub use id::SandboxId;
