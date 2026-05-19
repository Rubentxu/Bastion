//! Provider port traits — segregated interfaces for sandbox providers.
//!
//! This module contains focused, cohesive ports that can be implemented
//! independently:
//!
//! - [`LifecyclePort`] — sandbox creation, termination, and health checks
//! - [`ExecutionPort`] — command execution and streaming
//! - [`FilePort`] — file operations within sandboxes
//! - [`SnapshotPort`] — sandbox state snapshots
//! - [`MetadataPort`] — provider capabilities and sandbox metadata
//!
//! ## Architecture
//!
//! The ports follow the **Interface Segregation Principle** from SOLID.
//! Each port has a single responsibility, allowing providers to implement
//! only the functionality they support.
//!
//! The combined [`SandboxProvider`](super::SandboxProvider) trait (in `../port.rs`)
//! requires all ports, but providers may implement only the ports they support
//! and use the blanket impl in `../compat.rs` to get `SandboxProvider` automatically.

pub mod execution;
pub mod file;
pub mod lifecycle;
pub mod metadata;
pub mod snapshot;

// Re-export port traits for convenience
pub use execution::{ExecutionPort, CommandStream};
pub use file::FilePort;
pub use lifecycle::LifecyclePort;
pub use metadata::MetadataPort;
pub use snapshot::SnapshotPort;
