//! Provider instance entity and related types.
//!
//! ProviderInstance is an ENTITY persisted to TOML in .bastion/provider-instances/.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::instance_config::ProviderInstanceConfig;
use super::instance_constraints::InstanceConstraints;
use super::provider_type::ProviderTypeId;
use crate::shared::DomainError;

const INVALID_NAME_CHARS: &[char] = &['/', '\\', ':', '\0'];

fn is_valid_name(name: &str) -> Result<(), DomainError> {
    if name.is_empty() {
        return Err(DomainError::Validation(
            "Provider instance name cannot be empty".into(),
        ));
    }
    if name.contains("..") {
        return Err(DomainError::Validation(
            "Provider instance name cannot contain '..'".into(),
        ));
    }
    for c in INVALID_NAME_CHARS {
        if name.contains(*c) {
            return Err(DomainError::Validation(format!(
                "Provider instance name contains invalid character: {:?}",
                c
            )));
        }
    }
    Ok(())
}

/// Unique identifier for a provider instance.
///
/// ENTITY — persisted in TOML.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderInstanceId(pub Uuid);

impl ProviderInstanceId {
    /// Create a new ProviderInstanceId with a random UUID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create a ProviderInstanceId from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ProviderInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProviderInstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a provider instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderInstanceStatus {
    /// Being loaded from TOML/DB — initial state.
    Loading,
    /// Ready to accept sandboxes.
    Active,
    /// Working but with warnings — may need attention.
    Degraded,
    /// Not functional — requires intervention.
    Offline,
}

impl ProviderInstanceStatus {
    /// Check if this status allows accepting new sandboxes.
    pub fn can_accept_sandboxes(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Check if this is a terminal state (no automatic recovery).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Offline)
    }

    /// Check if this status indicates the instance is operational.
    pub fn is_operational(&self) -> bool {
        matches!(self, Self::Active | Self::Degraded)
    }

    /// Check if transitioning from this status to another is valid.
    ///
    /// Valid transitions:
    /// - Loading → Active, Degraded, Offline
    /// - Active → Degraded, Offline
    /// - Degraded → Active, Offline
    /// - Offline → (terminal, no valid transitions out)
    pub fn is_valid_transition(&self, target: ProviderInstanceStatus) -> bool {
        matches!(
            (self, target),
            (Self::Loading, Self::Active | Self::Degraded | Self::Offline)
                | (Self::Active, Self::Degraded | Self::Offline)
                | (Self::Degraded, Self::Active | Self::Offline)
        )
    }
}

impl std::fmt::Display for ProviderInstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => write!(f, "loading"),
            Self::Active => write!(f, "active"),
            Self::Degraded => write!(f, "degraded"),
            Self::Offline => write!(f, "offline"),
        }
    }
}

/// A running instance of a provider backend.
///
/// ENTITY — persisted to TOML in .bastion/provider-instances/{name}.toml.
/// Has a unique identity (ProviderInstanceId) and lifecycle status.
///
/// State transitions are protected — only the aggregate root can mutate status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInstance {
    id: ProviderInstanceId,
    type_id: ProviderTypeId,
    name: String,
    display_name: String,
    description: Option<String>,
    status: ProviderInstanceStatus,
    config: ProviderInstanceConfig,
    constraints: InstanceConstraints,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ProviderInstance {
    /// Create a new provider instance in Loading state.
    ///
    /// Name must not be empty or contain `/`, `\`, `:`, or null bytes.
    /// Returns `Err(DomainError::Validation)` if name is invalid.
    pub fn new(
        id: ProviderInstanceId,
        type_id: ProviderTypeId,
        name: String,
        display_name: String,
        description: Option<String>,
        config: ProviderInstanceConfig,
        constraints: InstanceConstraints,
    ) -> Result<Self, DomainError> {
        is_valid_name(&name)?;
        let now = Utc::now();
        Ok(Self {
            id,
            type_id,
            name,
            display_name,
            description,
            status: ProviderInstanceStatus::Loading,
            config,
            constraints,
            created_at: now,
            updated_at: now,
        })
    }

    /// Transition to Active status.
    ///
    /// Only valid from Loading or Degraded.
    pub fn mark_active(&mut self) -> Result<(), DomainError> {
        self.transition_to(ProviderInstanceStatus::Active)
    }

    /// Transition to Degraded status.
    ///
    /// Only valid from Loading, Active, or Degraded.
    pub fn mark_degraded(&mut self) -> Result<(), DomainError> {
        self.transition_to(ProviderInstanceStatus::Degraded)
    }

    /// Transition to Offline status.
    ///
    /// Only valid from Loading, Active, or Degraded.
    pub fn mark_offline(&mut self) -> Result<(), DomainError> {
        self.transition_to(ProviderInstanceStatus::Offline)
    }

    /// Transition to Failed status.
    ///
    /// Valid from ANY status including Offline and Failed themselves.
    pub fn mark_failed(&mut self) {
        self.status = ProviderInstanceStatus::Offline;
        self.updated_at = Utc::now();
    }

    /// Internal transition helper enforcing FSM rules.
    fn transition_to(&mut self, target: ProviderInstanceStatus) -> Result<(), DomainError> {
        if self.status.is_valid_transition(target) {
            self.status = target;
            self.updated_at = Utc::now();
            Ok(())
        } else {
            Err(DomainError::Validation(format!(
                "Invalid transition from {:?} to {:?}",
                self.status, target
            )))
        }
    }

    /// Check if the instance can accept new sandboxes.
    pub fn can_accept_sandboxes(&self) -> bool {
        self.status.can_accept_sandboxes()
    }

    /// Accessor: unique identifier.
    pub fn id(&self) -> &ProviderInstanceId {
        &self.id
    }

    /// Accessor: provider type.
    pub fn type_id(&self) -> &ProviderTypeId {
        &self.type_id
    }

    /// Accessor: instance name (used in TOML filename).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Accessor: display name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Accessor: optional description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Accessor: current status.
    pub fn status(&self) -> ProviderInstanceStatus {
        self.status
    }

    /// Accessor: config.
    pub fn config(&self) -> &ProviderInstanceConfig {
        &self.config
    }

    /// Accessor: constraints.
    pub fn constraints(&self) -> &InstanceConstraints {
        &self.constraints
    }

    /// Accessor: creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Accessor: last update timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::instance_config::ProviderInstanceConfig;

    fn make_test_config() -> ProviderInstanceConfig {
        ProviderInstanceConfig::podman()
    }

    fn create_test_instance() -> ProviderInstance {
        ProviderInstance::new(
            ProviderInstanceId::new(),
            ProviderTypeId::new("podman"),
            "test-podman".to_string(),
            "Test Podman".to_string(),
            Some("A test instance".to_string()),
            make_test_config(),
            InstanceConstraints::default(),
        )
        .expect("test instance should be valid")
    }

    #[test]
    fn test_provider_instance_id_new() {
        let id = ProviderInstanceId::new();
        assert!(!id.as_uuid().is_nil());
    }

    #[test]
    fn test_provider_instance_id_display() {
        let id = ProviderInstanceId::new();
        let display = format!("{}", id);
        assert!(!display.is_empty());
    }

    #[test]
    fn test_provider_instance_status_can_accept_sandboxes() {
        assert!(!ProviderInstanceStatus::Loading.can_accept_sandboxes());
        assert!(ProviderInstanceStatus::Active.can_accept_sandboxes());
        assert!(!ProviderInstanceStatus::Degraded.can_accept_sandboxes());
        assert!(!ProviderInstanceStatus::Offline.can_accept_sandboxes());
    }

    #[test]
    fn test_provider_instance_status_is_terminal() {
        assert!(!ProviderInstanceStatus::Loading.is_terminal());
        assert!(!ProviderInstanceStatus::Active.is_terminal());
        assert!(!ProviderInstanceStatus::Degraded.is_terminal());
        assert!(ProviderInstanceStatus::Offline.is_terminal());
    }

    #[test]
    fn test_provider_instance_status_is_operational() {
        assert!(!ProviderInstanceStatus::Loading.is_operational());
        assert!(ProviderInstanceStatus::Active.is_operational());
        assert!(ProviderInstanceStatus::Degraded.is_operational());
        assert!(!ProviderInstanceStatus::Offline.is_operational());
    }

    #[test]
    fn test_provider_instance_status_valid_transitions() {
        assert!(ProviderInstanceStatus::Loading.is_valid_transition(ProviderInstanceStatus::Active));
        assert!(ProviderInstanceStatus::Loading.is_valid_transition(ProviderInstanceStatus::Degraded));
        assert!(ProviderInstanceStatus::Loading.is_valid_transition(ProviderInstanceStatus::Offline));
        assert!(ProviderInstanceStatus::Active.is_valid_transition(ProviderInstanceStatus::Degraded));
        assert!(ProviderInstanceStatus::Active.is_valid_transition(ProviderInstanceStatus::Offline));
        assert!(ProviderInstanceStatus::Degraded.is_valid_transition(ProviderInstanceStatus::Active));
        assert!(ProviderInstanceStatus::Degraded.is_valid_transition(ProviderInstanceStatus::Offline));
        assert!(!ProviderInstanceStatus::Offline.is_valid_transition(ProviderInstanceStatus::Active));
        assert!(!ProviderInstanceStatus::Offline.is_valid_transition(ProviderInstanceStatus::Degraded));
        assert!(!ProviderInstanceStatus::Active.is_valid_transition(ProviderInstanceStatus::Loading));
    }

    #[test]
    fn test_provider_instance_new() {
        let instance = create_test_instance();
        assert!(!instance.id().as_uuid().is_nil());
        assert_eq!(instance.type_id().as_str(), "podman");
        assert_eq!(instance.name(), "test-podman");
        assert_eq!(instance.display_name(), "Test Podman");
        assert!(instance.description().is_some());
        assert!(matches!(instance.status(), ProviderInstanceStatus::Loading));
    }

    #[test]
    fn test_provider_instance_new_rejects_empty_name() {
        let err = ProviderInstance::new(
            ProviderInstanceId::new(),
            ProviderTypeId::new("podman"),
            "".to_string(),
            "Test".to_string(),
            None,
            make_test_config(),
            InstanceConstraints::default(),
        )
        .expect_err("empty name should be rejected");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_new_rejects_slash() {
        let err = ProviderInstance::new(
            ProviderInstanceId::new(),
            ProviderTypeId::new("podman"),
            "foo/bar".to_string(),
            "Test".to_string(),
            None,
            make_test_config(),
            InstanceConstraints::default(),
        )
        .expect_err("name with / should be rejected");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_provider_instance_mark_active() {
        let mut instance = create_test_instance();
        instance.mark_active().expect("Loading->Active should succeed");
        assert!(matches!(instance.status(), ProviderInstanceStatus::Active));
    }

    #[test]
    fn test_provider_instance_mark_active_from_degraded() {
        let mut instance = create_test_instance();
        instance.mark_degraded().expect("Loading->Degraded should succeed");
        instance.mark_active().expect("Degraded->Active should succeed");
        assert!(matches!(instance.status(), ProviderInstanceStatus::Active));
    }

    #[test]
    fn test_provider_instance_mark_active_from_offline_fails() {
        let mut instance = create_test_instance();
        instance.mark_offline().expect("Loading->Offline should succeed");
        instance.mark_active().expect_err("Offline->Active should fail");
        assert!(matches!(instance.status(), ProviderInstanceStatus::Offline));
    }

    #[test]
    fn test_provider_instance_mark_degraded() {
        let mut instance = create_test_instance();
        instance.mark_degraded().expect("Loading->Degraded should succeed");
        assert!(matches!(instance.status(), ProviderInstanceStatus::Degraded));
    }

    #[test]
    fn test_provider_instance_mark_offline() {
        let mut instance = create_test_instance();
        instance.mark_offline().expect("Loading->Offline should succeed");
        assert!(matches!(instance.status(), ProviderInstanceStatus::Offline));
    }

    #[test]
    fn test_provider_instance_mark_failed() {
        let mut instance = create_test_instance();
        instance.mark_failed();
        assert!(matches!(instance.status(), ProviderInstanceStatus::Offline));
    }

    #[test]
    fn test_provider_instance_mark_failed_from_active() {
        let mut instance = create_test_instance();
        instance.mark_active().expect("Loading->Active");
        instance.mark_failed();
        assert!(matches!(instance.status(), ProviderInstanceStatus::Offline));
    }

    #[test]
    fn test_provider_instance_can_accept_sandboxes() {
        let mut instance = create_test_instance();
        assert!(!instance.can_accept_sandboxes());
        instance.mark_active().expect("Loading->Active");
        assert!(instance.can_accept_sandboxes());
    }

    #[test]
    fn test_provider_instance_serde() {
        let instance = create_test_instance();
        let json = serde_json::to_string(&instance).unwrap();
        let parsed: ProviderInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name(), instance.name());
        assert_eq!(parsed.type_id(), instance.type_id());
    }

    #[test]
    fn test_provider_instance_accessors() {
        let instance = create_test_instance();
        assert_eq!(instance.id(), instance.id());
        assert_eq!(instance.type_id().as_str(), "podman");
        assert_eq!(instance.name(), "test-podman");
        assert_eq!(instance.display_name(), "Test Podman");
        assert!(instance.description().is_some());
        assert!(instance.created_at() <= instance.updated_at());
    }
}
