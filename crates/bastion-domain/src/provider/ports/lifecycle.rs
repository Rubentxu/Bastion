//! Core lifecycle port — sandbox creation, termination, and health checks.
//!
//! This trait focuses ONLY on the fundamental lifecycle operations:
//! create, terminate, and health checking. Other concerns like snapshots,
//! metadata, and execution are handled by separate ports.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::sandbox::entity::Sandbox;
use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use crate::shared::DomainError;
use crate::shared::id::SandboxId;

/// Core lifecycle port — manages sandbox creation, termination, and health checks.
///
/// Implementors: PodmanProvider, DockerProvider, FirecrackerProvider, GVisorProvider.
///
/// ## Architecture
///
/// This trait follows the **Interface Segregation Principle**. Providers that only
/// support a subset of lifecycle operations can implement just the methods they
/// support, with default implementations returning `UnsupportedOperation` for others.
///
/// The `SandboxProvider` combined trait (in `../port.rs`) requires all port traits,
/// but individual providers may only implement the ports they support.
#[async_trait]
pub trait LifecyclePort: Send + Sync + std::fmt::Debug {
    /// Create a new isolated sandbox.
    async fn create(
        &self,
        id: &SandboxId,
        template: &str,
        resources: &ResourcesSpec,
        network: &NetworkSpec,
        env_vars: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError>;

    /// Terminate and clean up a sandbox.
    async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError>;

    /// Check if a sandbox is alive.
    async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError>;
}
