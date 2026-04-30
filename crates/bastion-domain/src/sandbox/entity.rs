//! Sandbox aggregate root.
//!
//! The Sandbox entity encapsulates the lifecycle and invariants of an isolated
//! execution environment.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::value_objects::{NetworkSpec, ResourcesSpec, SandboxStatus};
use crate::shared::id::{ProviderId, SandboxId, TemplateId};

/// Sandbox aggregate root.
///
/// Represents an isolated execution environment with lifecycle management.
/// Invariants:
/// - A sandbox cannot be terminated twice
/// - Status transitions follow: Pending → Running → (Paused | Stopped | Failed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sandbox {
    pub id: SandboxId,
    pub template_id: TemplateId,
    pub provider_id: ProviderId,
    pub status: SandboxStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resources: ResourcesSpec,
    pub network: NetworkSpec,
    pub metadata: std::collections::HashMap<String, String>,
}

impl Sandbox {
    /// Create a new sandbox in Pending state.
    pub fn new(
        id: SandboxId,
        template_id: TemplateId,
        provider_id: ProviderId,
        resources: ResourcesSpec,
        network: NetworkSpec,
    ) -> Self {
        Self {
            id,
            template_id,
            provider_id,
            status: SandboxStatus::Pending,
            created_at: Utc::now(),
            expires_at: None,
            resources,
            network,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Transition to Running state.
    pub fn mark_running(&mut self) -> Result<(), crate::shared::DomainError> {
        match self.status {
            SandboxStatus::Pending | SandboxStatus::Paused => {
                self.status = SandboxStatus::Running;
                Ok(())
            }
            _ => Err(crate::shared::DomainError::Validation(format!(
                "Cannot transition sandbox {} from {:?} to Running",
                self.id, self.status
            ))),
        }
    }

    /// Transition to Stopped state.
    pub fn terminate(&mut self) -> Result<(), crate::shared::DomainError> {
        match self.status {
            SandboxStatus::Running | SandboxStatus::Pending | SandboxStatus::Paused => {
                self.status = SandboxStatus::Stopped;
                Ok(())
            }
            SandboxStatus::Stopped => Err(crate::shared::DomainError::Validation(format!(
                "Sandbox {} is already stopped",
                self.id
            ))),
            SandboxStatus::Failed => Err(crate::shared::DomainError::Validation(format!(
                "Sandbox {} has failed, cannot terminate",
                self.id
            ))),
        }
    }

    /// Transition to Failed state.
    pub fn mark_failed(&mut self) {
        self.status = SandboxStatus::Failed;
    }

    /// Check if the sandbox has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|expires| Utc::now() > expires)
            .unwrap_or(false)
    }

    /// Set expiration time based on timeout in milliseconds.
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.expires_at = Some(self.created_at + chrono::Duration::milliseconds(timeout_ms as i64));
    }

    /// Check if the sandbox is still active (can accept commands).
    pub fn is_active(&self) -> bool {
        matches!(self.status, SandboxStatus::Running | SandboxStatus::Pending)
    }
}
