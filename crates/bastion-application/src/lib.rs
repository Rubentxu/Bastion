//! # Bastion Application
//!
//! Application layer containing use cases that orchestrate domain objects.
//! This layer depends ONLY on the domain layer — no infrastructure concerns.

// TODO: Migrate all SandboxProvider usages to SandboxLifecycle + TaskExecutor.
// Tracked as a phased migration; suppress deprecation warnings until complete.
#![allow(deprecated)]

pub mod catalog;
pub mod execution;
pub mod file_ops;
pub mod sandbox;
pub mod template;

pub use catalog::*;

pub use execution::*;
pub use file_ops::*;
pub use sandbox::*;
pub use template::*;
