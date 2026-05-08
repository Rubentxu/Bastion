//! Snapshot management — deprecated CLI-based module.
//!
//! Snapshot operations have moved to provider trait implementations
//! using the bollard Docker API client (see `snapshot_ops`).
//! Use `SandboxProvider::create_snapshot()`, `restore_snapshot()`, etc. directly.

// Re-export for backward compatibility
pub use super::snapshot_ops::snapshot_name_from_id;
