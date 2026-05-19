//! Sandbox state manager port.
//!
//! Defines the port for sandbox state transitions. Implementations live in
//! bastion-infrastructure (using DashMap for concurrency).
//!
//! Note: Currently implemented as inherent methods in DashMapSandboxStateMachine.
//! The trait is kept for testability and potential future abstraction.

use std::collections::HashMap;

use crate::sandbox::value_objects::SandboxStatus;
use crate::shared::{DomainError, id::SandboxId};

pub trait SandboxStateManager: Send + Sync {
    fn register(&self, id: SandboxId) -> Result<(), DomainError>;
    fn transition(&self, id: &SandboxId, new_status: SandboxStatus) -> Result<SandboxStatus, DomainError>;
    fn get_state(&self, id: &SandboxId) -> Option<SandboxStatus>;
    fn remove(&self, id: &SandboxId) -> Option<Box<dyn std::any::Any + Send + Sync>>;
    fn list_active(&self) -> Vec<SandboxId>;
    fn count_by_status(&self) -> HashMap<SandboxStatus, usize>;
}
