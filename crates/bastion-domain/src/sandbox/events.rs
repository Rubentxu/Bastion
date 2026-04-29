//! Domain events for the Sandbox bounded context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::shared::id::SandboxId;

/// Domain events emitted during sandbox lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxEvent {
    Created(SandboxCreated),
    Started(SandboxStarted),
    Terminated(SandboxTerminated),
    Failed(SandboxFailed),
    Expired(SandboxExpired),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCreated {
    pub sandbox_id: SandboxId,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxStarted {
    pub sandbox_id: SandboxId,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxTerminated {
    pub sandbox_id: SandboxId,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxFailed {
    pub sandbox_id: SandboxId,
    pub reason: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxExpired {
    pub sandbox_id: SandboxId,
    pub occurred_at: DateTime<Utc>,
}
