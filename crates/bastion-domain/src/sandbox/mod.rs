//! Sandbox bounded context — the core aggregate.
//!
//! A Sandbox is an isolated execution environment managed by a Provider.

pub mod entity;
pub mod value_objects;
pub mod repository;
pub mod events;

pub use entity::Sandbox;
pub use value_objects::*;
pub use repository::SandboxRepository;
pub use events::*;
