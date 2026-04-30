//! Sandbox repository port (interface).
//!
//! This is a DOMAIN port — the infrastructure layer provides the implementation.

use async_trait::async_trait;

use super::entity::Sandbox;
use crate::shared::id::SandboxId;

/// Repository interface for sandbox persistence.
///
/// Implemented by infrastructure adapters (in-memory, database, etc.).
#[async_trait]
pub trait SandboxRepository: Send + Sync + std::fmt::Debug {
    /// Store a new sandbox.
    async fn save(&self, sandbox: &Sandbox) -> Result<(), crate::shared::DomainError>;

    /// Retrieve a sandbox by ID.
    async fn find_by_id(
        &self,
        id: &SandboxId,
    ) -> Result<Option<Sandbox>, crate::shared::DomainError>;

    /// Update an existing sandbox.
    async fn update(&self, sandbox: &Sandbox) -> Result<(), crate::shared::DomainError>;

    /// Remove a sandbox.
    async fn delete(&self, id: &SandboxId) -> Result<(), crate::shared::DomainError>;

    /// List all active sandboxes.
    async fn find_active(&self) -> Result<Vec<Sandbox>, crate::shared::DomainError>;
}
