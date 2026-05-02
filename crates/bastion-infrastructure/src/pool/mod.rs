//! Sandbox pool management module.
//!
//! Provides warm container pooling to reduce sandbox creation latency.

pub mod manager;

pub use manager::{PoolConfig, PoolQueue, PoolStats, RecoveryResult, SandboxPoolManager};
