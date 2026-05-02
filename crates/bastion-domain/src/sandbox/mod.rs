//! Sandbox bounded context — the core aggregate.
//!
//! A Sandbox is an isolated execution environment managed by a Provider.

pub mod entity;
pub mod events;
pub mod repository;
pub mod snapshot;
pub mod value_objects;

pub use entity::Sandbox;
pub use events::*;
pub use repository::SandboxRepository;
pub use snapshot::SnapshotInfo;
pub use value_objects::*;
