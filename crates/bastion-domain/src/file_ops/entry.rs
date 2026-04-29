//! File entry value object.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A file or directory entry within a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub modified_at: Option<DateTime<Utc>>,
    pub permissions: String,
}
