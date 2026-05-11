//! # Bastion Domain
//!
//! Core domain layer containing entities, value objects, repository interfaces (ports),
//! and domain events for the Bastion sandbox gateway.

// SandboxProvider trait is intentionally deprecated in favor of SandboxLifecycle + TaskExecutor.
// The trait definition itself emits the deprecation warning; suppress at crate root.
#![allow(deprecated)]
//!
//! ## Architecture
//! This crate has ZERO external infrastructure dependencies. It defines the
//! ubiquitous language and business rules that all other layers depend on.
//!
//! ## Bounded Contexts
//! - **Sandbox**: Lifecycle management of isolated execution environments
//! - **Execution**: Running commands and streaming output
//! - **Provider**: Backend abstraction for container runtimes
//! - **FileOps**: File operations within sandboxes

pub mod catalog;
pub mod execution;
pub mod file_ops;
pub mod orientation;
pub mod provider;
pub mod sandbox;
pub mod secret;
pub mod shared;
pub mod template;
