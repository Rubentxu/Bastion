//! Sandbox state machine — manages sandbox state transitions.

#[cfg(feature = "use-segregated-traits")]
use dashmap::DashMap;
#[cfg(feature = "use-segregated-traits")]
use std::collections::HashMap;
#[cfg(feature = "use-segregated-traits")]
use std::sync::Arc;
#[cfg(feature = "use-segregated-traits")]
use std::time::Instant;

#[cfg(feature = "use-segregated-traits")]
use crate::sandbox::value_objects::SandboxStatus;
#[cfg(feature = "use-segregated-traits")]
use crate::shared::DomainError;
#[cfg(feature = "use-segregated-traits")]
use crate::shared::id::SandboxId;

/// State entry stored in the FSM.
#[cfg(feature = "use-segregated-traits")]
pub struct StateEntry {
    /// Current status of the sandbox.
    pub status: SandboxStatus,
    /// Opaque payload (ContainerState, VmState, etc).
    pub payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    /// When the sandbox was registered.
    pub registered_at: Instant,
}

#[cfg(feature = "use-segregated-traits")]
impl std::fmt::Debug for StateEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateEntry")
            .field("status", &self.status)
            .field("payload", &"<opaque>")
            .field("registered_at", &self.registered_at)
            .finish()
    }
}

/// Sandbox state machine — manages sandbox state transitions.
///
/// Replaces per-provider `DashMap<String, ContainerState>` /
/// `DashMap<String, VmState>` / `RwLock<HashMap>` patterns with a
/// unified, observable state tracking mechanism.
///
/// State transitions (from design):
/// - `Pending → Running` (automatic after create)
/// - `Pending → Stopped` (terminate before start)
/// - `Pending → Failed` (create failure)
/// - `Running → Stopped` (normal terminate)
/// - `Running → Paused` (pause, future capability)
/// - `Running → Failed` (unrecoverable error)
/// - `Paused → Running` (resume)
/// - `Paused → Stopped` (terminate from paused)
///
/// Invalid transitions (return `Err(Validation)`):
/// - `Stopped → *` (already terminated)
/// - `Failed → *` (already failed)
/// - `Pending → Paused` (not yet running)
#[cfg(feature = "use-segregated-traits")]
#[derive(Debug, Default)]
pub struct SandboxStateMachine {
    states: Arc<DashMap<SandboxId, StateEntry>>,
}

#[cfg(feature = "use-segregated-traits")]
impl SandboxStateMachine {
    /// Create a new empty state machine.
    pub fn new() -> Self {
        Self {
            states: Arc::new(DashMap::new()),
        }
    }

    /// Register a new sandbox in `Pending` state.
    ///
    /// Returns error if the sandbox ID already exists (idempotent create
    /// is handled by the provider calling `transition` instead).
    pub fn register(&self, id: SandboxId) -> Result<(), DomainError> {
        let entry = StateEntry {
            status: SandboxStatus::Pending,
            payload: None,
            registered_at: Instant::now(),
        };

        // Use insert with a check — DashMap::insert returns None if key was absent
        if self.states.insert(id, entry).is_some() {
            return Err(DomainError::AlreadyExists(
                "Sandbox already registered".into(),
            ));
        }

        Ok(())
    }

    /// Transition a sandbox to a new status.
    ///
    /// Returns the previous status, or error if the transition is invalid.
    pub fn transition(
        &self,
        id: &SandboxId,
        new_status: SandboxStatus,
    ) -> Result<SandboxStatus, DomainError> {
        // Get the current entry
        let mut entry = self
            .states
            .get_mut(id)
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let old_status = entry.status;

        // Validate the transition
        if !Self::is_valid_transition(old_status, new_status) {
            return Err(DomainError::Validation(format!(
                "Invalid transition from {:?} to {:?}",
                old_status, new_status
            )));
        }

        // Apply the transition
        entry.status = new_status;

        Ok(old_status)
    }

    /// Check if a transition from `from` to `to` is valid.
    fn is_valid_transition(from: SandboxStatus, to: SandboxStatus) -> bool {
        matches!(
            (from, to),
            // Pending can go to Running, Stopped, or Failed
            (SandboxStatus::Pending, SandboxStatus::Running)
                | (SandboxStatus::Pending, SandboxStatus::Stopped)
                | (SandboxStatus::Pending, SandboxStatus::Failed)
            // Running can go to Paused, Stopped, or Failed
                | (SandboxStatus::Running, SandboxStatus::Paused)
                | (SandboxStatus::Running, SandboxStatus::Stopped)
                | (SandboxStatus::Running, SandboxStatus::Failed)
            // Paused can go back to Running or to Stopped
                | (SandboxStatus::Paused, SandboxStatus::Running)
                | (SandboxStatus::Paused, SandboxStatus::Stopped)
        )
    }

    /// Get the current status of a sandbox.
    pub fn get_state(&self, id: &SandboxId) -> Option<SandboxStatus> {
        self.states.get(id).map(|e| e.status)
    }

    /// Attach an opaque state payload (ContainerState, VmState, etc).
    ///
    /// The payload is stored as `Box<dyn Any + Send + Sync>` and can be
    /// retrieved via `get_payload` or `remove`.
    pub fn attach_payload<T: Send + Sync + 'static>(
        &self,
        id: &SandboxId,
        payload: T,
    ) -> Result<(), DomainError> {
        let mut entry = self
            .states
            .get_mut(id)
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        entry.payload = Some(Box::new(payload));
        Ok(())
    }

    /// Get the payload for a sandbox, if present and of the expected type.
    pub fn get_payload<T: Clone + Send + Sync + 'static>(&self, id: &SandboxId) -> Option<T> {
        let entry = self.states.get(id)?;
        let payload = entry.payload.as_ref()?;
        payload.downcast_ref::<T>().cloned()
    }

    /// Remove a sandbox and return its payload for cleanup.
    ///
    /// This is called by the provider during `terminate` to get the opaque
    /// state back (e.g., `ContainerState` with the `Child` handle to kill).
    pub fn remove(&self, id: &SandboxId) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        self.states.remove(id).and_then(|(_, entry)| entry.payload)
    }

    /// List all active sandbox IDs (sandboxes still in the map).
    pub fn list_active(&self) -> Vec<SandboxId> {
        self.states
            .iter()
            .map(|item| {
                let key = item.key();
                key.clone()
            })
            .collect()
    }

    /// Count sandboxes grouped by status.
    pub fn count_by_status(&self) -> HashMap<SandboxStatus, usize> {
        let mut counts: HashMap<SandboxStatus, usize> = HashMap::new();
        for entry in self.states.iter() {
            *counts.entry(entry.status).or_default() += 1;
        }
        counts
    }
}
