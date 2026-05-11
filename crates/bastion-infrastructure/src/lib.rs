//! # Bastion Infrastructure
//!
//! Infrastructure adapters that implement domain ports (driven adapters).

// TODO: Migrate all SandboxProvider usages to SandboxLifecycle + TaskExecutor.
// Tracked as a phased migration; suppress deprecation warnings until complete.
#![allow(deprecated)]

pub mod catalog;
pub mod config;
pub mod enrichment;
pub mod evaluation;
pub mod grpc;
pub mod metrics;
pub mod persistence;
pub mod pool;
pub mod provider;
pub mod sandbox;
pub mod secret;
pub mod template;
