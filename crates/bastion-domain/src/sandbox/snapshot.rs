use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub snapshot_id: String,
    pub sandbox_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub size_bytes: u64,
}