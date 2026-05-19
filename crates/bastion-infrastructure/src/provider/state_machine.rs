//! Sandbox state machine implementation using DashMap.
//!
//! Provides concurrent sandbox state tracking for infrastructure providers.

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bastion_domain::sandbox::value_objects::SandboxStatus;
use bastion_domain::shared::{DomainError, id::SandboxId};

pub struct StateEntry {
    pub status: SandboxStatus,
    pub payload: Option<Box<dyn std::any::Any + Send + Sync>>,
    pub registered_at: Instant,
}

impl std::fmt::Debug for StateEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateEntry")
            .field("status", &self.status)
            .field("payload", &"<opaque>")
            .field("registered_at", &self.registered_at)
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct DashMapSandboxStateMachine {
    states: Arc<DashMap<SandboxId, StateEntry>>,
}

impl DashMapSandboxStateMachine {
    pub fn new() -> Self {
        Self {
            states: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, id: SandboxId) -> Result<(), DomainError> {
        let entry = StateEntry {
            status: SandboxStatus::Pending,
            payload: None,
            registered_at: Instant::now(),
        };

        if self.states.insert(id, entry).is_some() {
            return Err(DomainError::AlreadyExists(
                "Sandbox already registered".into(),
            ));
        }

        Ok(())
    }

    pub fn transition(
        &self,
        id: &SandboxId,
        new_status: SandboxStatus,
    ) -> Result<SandboxStatus, DomainError> {
        let mut entry = self
            .states
            .get_mut(id)
            .ok_or_else(|| DomainError::NotFound(format!("Sandbox {} not found", id)))?;

        let old_status = entry.status;

        if !Self::is_valid_transition(old_status, new_status) {
            return Err(DomainError::Validation(format!(
                "Invalid transition from {:?} to {:?}",
                old_status, new_status
            )));
        }

        entry.status = new_status;

        Ok(old_status)
    }

    pub fn is_valid_transition(from: SandboxStatus, to: SandboxStatus) -> bool {
        matches!(
            (from, to),
            (SandboxStatus::Pending, SandboxStatus::Running)
                | (SandboxStatus::Pending, SandboxStatus::Stopped)
                | (SandboxStatus::Pending, SandboxStatus::Failed)
                | (SandboxStatus::Running, SandboxStatus::Paused)
                | (SandboxStatus::Running, SandboxStatus::Stopped)
                | (SandboxStatus::Running, SandboxStatus::Failed)
                | (SandboxStatus::Paused, SandboxStatus::Running)
                | (SandboxStatus::Paused, SandboxStatus::Stopped)
        )
    }

    pub fn get_state(&self, id: &SandboxId) -> Option<SandboxStatus> {
        self.states.get(id).map(|e| e.status)
    }

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

    pub fn get_payload<T: Clone + Send + Sync + 'static>(&self, id: &SandboxId) -> Option<T> {
        let entry = self.states.get(id)?;
        let payload = entry.payload.as_ref()?;
        payload.downcast_ref::<T>().cloned()
    }

    pub fn remove(&self, id: &SandboxId) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        self.states.remove(id).and_then(|(_, entry)| entry.payload)
    }

    pub fn list_active(&self) -> Vec<SandboxId> {
        self.states.iter().map(|item| item.key().clone()).collect()
    }

    pub fn count_by_status(&self) -> HashMap<SandboxStatus, usize> {
        let mut counts: HashMap<SandboxStatus, usize> = HashMap::new();
        for entry in self.states.iter() {
            *counts.entry(entry.status).or_default() += 1;
        }
        counts
    }
}
