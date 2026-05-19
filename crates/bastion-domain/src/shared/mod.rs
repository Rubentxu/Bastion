//! Shared kernel — common types used across bounded contexts.

pub mod brn;
pub mod error;
pub mod id;

pub use brn::{Brn, BrnError, BrnNamespace, BrnType};
pub use error::DomainError;
pub use id::SandboxId;
