//! Sandbox aggregate root.
//!
//! The Sandbox entity encapsulates the lifecycle and invariants of an isolated
//! execution environment.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::value_objects::{NetworkSpec, ResourcesSpec, SandboxStatus};
use crate::project::{ProjectId, SandboxPurpose};
use crate::provider::instance::ProviderInstanceId;
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
    /// The specific provider instance this sandbox is running on (None for legacy/migration period).
    pub provider_instance_id: Option<ProviderInstanceId>,
    pub status: SandboxStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resources: ResourcesSpec,
    pub network: NetworkSpec,
    pub metadata: std::collections::HashMap<String, String>,
    /// Project this sandbox belongs to (None for legacy sandboxes).
    pub project_id: Option<ProjectId>,
    /// Purpose of this sandbox (None for legacy).
    pub purpose: Option<SandboxPurpose>,
}

impl Sandbox {
    /// Create a new sandbox in Pending state.
    pub fn new(
        id: SandboxId,
        template_id: TemplateId,
        provider_id: ProviderId,
        provider_instance_id: Option<ProviderInstanceId>,
        resources: ResourcesSpec,
        network: NetworkSpec,
    ) -> Self {
        Self {
            id,
            template_id,
            provider_id,
            provider_instance_id,
            status: SandboxStatus::Pending,
            created_at: Utc::now(),
            expires_at: None,
            resources,
            network,
            metadata: std::collections::HashMap::new(),
            project_id: None,
            purpose: None,
        }
    }

    /// Create a new sandbox with project scope.
    pub fn new_with_project(
        id: SandboxId,
        template_id: TemplateId,
        provider_id: ProviderId,
        provider_instance_id: Option<ProviderInstanceId>,
        resources: ResourcesSpec,
        network: NetworkSpec,
        project_id: ProjectId,
        purpose: SandboxPurpose,
    ) -> Self {
        Self {
            id,
            template_id,
            provider_id,
            provider_instance_id,
            status: SandboxStatus::Pending,
            created_at: Utc::now(),
            expires_at: None,
            resources,
            network,
            metadata: std::collections::HashMap::new(),
            project_id: Some(project_id),
            purpose: Some(purpose),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
    use crate::shared::id::ProviderId;

    fn create_test_sandbox() -> Sandbox {
        Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        )
    }

    #[test]
    fn test_sandbox_new_has_no_project() {
        let sandbox = create_test_sandbox();
        assert!(sandbox.project_id.is_none());
        assert!(sandbox.purpose.is_none());
    }

    #[test]
    fn test_sandbox_new_with_project() {
        let sandbox = Sandbox::new_with_project(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
            ProjectId::new("proj-1"),
            SandboxPurpose::E2eTest,
        );
        assert!(sandbox.project_id.is_some());
        assert_eq!(sandbox.project_id.as_ref().unwrap().as_str(), "proj-1");
        assert!(sandbox.purpose.is_some());
        assert_eq!(sandbox.purpose.unwrap(), SandboxPurpose::E2eTest);
    }

    #[test]
    fn test_sandbox_serialize_with_project_fields() {
        let sandbox = Sandbox::new_with_project(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
            ProjectId::new("proj-1"),
            SandboxPurpose::PipelineStage,
        );
        let json = serde_json::to_string(&sandbox).unwrap();
        assert!(json.contains("\"project_id\""));
        assert!(json.contains("\"purpose\""));
        let parsed: Sandbox = serde_json::from_str(&json).unwrap();
        assert!(parsed.project_id.is_some());
        assert_eq!(parsed.purpose, Some(SandboxPurpose::PipelineStage));
    }

    #[test]
    fn test_mark_failed_from_pending() {
        let sandbox = Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        assert_eq!(sandbox.status, SandboxStatus::Pending);
        let mut sandbox = sandbox;
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
    }

    #[test]
    fn test_mark_failed_from_running() {
        let mut sandbox = Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        sandbox.mark_running().unwrap();
        assert_eq!(sandbox.status, SandboxStatus::Running);
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
    }

    #[test]
    fn test_mark_failed_from_paused() {
        let mut sandbox = Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        sandbox.mark_running().unwrap();
        sandbox.status = SandboxStatus::Paused; // Direct transition to Paused
        assert_eq!(sandbox.status, SandboxStatus::Paused);
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
    }

    #[test]
    fn test_mark_failed_from_stopped() {
        let mut sandbox = Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        sandbox.mark_running().unwrap();
        sandbox.terminate().unwrap();
        assert_eq!(sandbox.status, SandboxStatus::Stopped);
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
    }

    #[test]
    fn test_mark_failed_from_already_failed() {
        let mut sandbox = Sandbox::new(
            SandboxId::new("test-sandbox"),
            TemplateId::new("template-1"),
            ProviderId::new("provider-1"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
        // Calling mark_failed again should still work (idempotent)
        sandbox.mark_failed();
        assert_eq!(sandbox.status, SandboxStatus::Failed);
    }
}
