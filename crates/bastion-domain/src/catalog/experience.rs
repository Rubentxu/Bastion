//! Experience record domain types.
//!
//! An `ExperienceRecord` captures structured evidence from a tool execution
//! for later assertion evaluation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::shared::{DomainError, id::SandboxId};

/// Status of a tool execution experience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceStatus {
    Success,
    Failure,
    Timeout,
    Cancelled,
}

impl ExperienceStatus {
    /// Returns true for terminal statuses (no further action possible).
    pub fn is_terminal(&self) -> bool {
        matches!(self, ExperienceStatus::Success | ExperienceStatus::Failure)
    }
}

/// A structured record of a tool execution for later assertion evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceRecord {
    /// Unique identifier for this experience.
    pub id: String,
    /// Optional correlation key — groups related experiences across tools.
    pub trace_id: Option<String>,
    /// Name of the tool that produced this experience.
    pub tool_name: String,
    /// Sandbox ID the tool operated on (if applicable).
    pub sandbox_id: Option<SandboxId>,
    /// When the tool execution started.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// When the tool execution finished (None if still running).
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Exit code of the command (None if not applicable).
    pub exit_code: Option<i32>,
    /// Truncated stdout preview (max 1 KiB, preserving head and tail).
    pub stdout_summary: String,
    /// Truncated stderr preview (max 1 KiB, preserving head and tail).
    pub stderr_summary: String,
    /// Outcome status.
    pub status: ExperienceStatus,
    /// Additional context as JSON.
    pub metadata: serde_json::Value,
}

impl ExperienceRecord {
    /// Create a new experience record with the given tool name.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            trace_id: None,
            tool_name: tool_name.into(),
            sandbox_id: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            exit_code: None,
            stdout_summary: String::new(),
            stderr_summary: String::new(),
            status: ExperienceStatus::Failure,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Set the trace ID.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Set the sandbox ID.
    pub fn with_sandbox_id(mut self, sandbox_id: SandboxId) -> Self {
        self.sandbox_id = Some(sandbox_id);
        self
    }

    /// Mark the experience as completed with the given exit code.
    pub fn completed(mut self, exit_code: i32) -> Self {
        self.exit_code = Some(exit_code);
        self.finished_at = Some(chrono::Utc::now());
        self.status = if exit_code == 0 {
            ExperienceStatus::Success
        } else {
            ExperienceStatus::Failure
        };
        self
    }

    /// Mark the experience as timed out.
    pub fn timed_out(mut self) -> Self {
        self.status = ExperienceStatus::Timeout;
        self.finished_at = Some(chrono::Utc::now());
        self
    }

    /// Mark the experience as cancelled.
    pub fn cancelled(mut self) -> Self {
        self.status = ExperienceStatus::Cancelled;
        self.finished_at = Some(chrono::Utc::now());
        self
    }

    /// Set the stdout summary (truncated to 1 KiB, preserving head and tail).
    pub fn with_stdout(mut self, stdout: &[u8]) -> Self {
        let s = String::from_utf8_lossy(stdout);
        self.stdout_summary = summarize_output(&s);
        self
    }

    /// Set the stderr summary (truncated to 1 KiB, preserving head and tail).
    pub fn with_stderr(mut self, stderr: &[u8]) -> Self {
        let s = String::from_utf8_lossy(stderr);
        self.stderr_summary = summarize_output(&s);
        self
    }

    /// Set arbitrary metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Duration in milliseconds if finished, None otherwise.
    pub fn duration_ms(&self) -> Option<u64> {
        self.finished_at
            .map(|finished| (finished - self.started_at).num_milliseconds() as u64)
    }
}

fn summarize_output(output: &str) -> String {
    const MAX_SUMMARY_BYTES: usize = 1024;
    const HEAD_BYTES: usize = 512;
    const TAIL_BYTES: usize = 512;

    if output.len() <= MAX_SUMMARY_BYTES {
        return output.to_string();
    }

    let head_end = floor_char_boundary(output, HEAD_BYTES);
    let tail_start = ceil_char_boundary(output, output.len().saturating_sub(TAIL_BYTES));

    format!(
        "{}\n... [truncated: showing head and tail] ...\n{}",
        &output[..head_end],
        &output[tail_start..]
    )
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Port for experience storage (implemented by infrastructure).
#[async_trait]
pub trait ExperienceStore: Send + Sync {
    /// Persist an experience record.
    async fn save(&self, record: &ExperienceRecord) -> Result<(), DomainError>;

    /// Retrieve an experience by its ID.
    async fn find_by_id(&self, id: &str) -> Result<Option<ExperienceRecord>, DomainError>;

    /// List all experiences for a given trace ID, sorted by started_at descending.
    async fn find_by_trace_id(&self, trace_id: &str) -> Result<Vec<ExperienceRecord>, DomainError>;

    /// List the most recent experiences (up to limit).
    async fn list_all(&self, limit: usize) -> Result<Vec<ExperienceRecord>, DomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_experience_record_builder() {
        let record = ExperienceRecord::new("sandbox_run")
            .with_trace_id("petclinic-fase014")
            .with_sandbox_id(SandboxId::new("sb-123"))
            .completed(0)
            .with_stdout(b"BUILD SUCCESS")
            .with_stderr(b"");

        assert_eq!(record.trace_id, Some("petclinic-fase014".to_string()));
        assert_eq!(record.exit_code, Some(0));
        assert_eq!(record.status, ExperienceStatus::Success);
        assert!(record.stdout_summary.contains("BUILD SUCCESS"));
    }

    #[test]
    fn test_experience_status_is_terminal() {
        assert!(ExperienceStatus::Success.is_terminal());
        assert!(ExperienceStatus::Failure.is_terminal());
        assert!(!ExperienceStatus::Timeout.is_terminal());
        assert!(!ExperienceStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_stdout_summary_preserves_tail_marker() {
        let mut output = "download log\n".repeat(200);
        output.push_str("[INFO] BUILD SUCCESS\n");

        let record = ExperienceRecord::new("sandbox_run").with_stdout(output.as_bytes());

        assert!(record.stdout_summary.len() <= 1100);
        assert!(record.stdout_summary.contains("download log"));
        assert!(record.stdout_summary.contains("BUILD SUCCESS"));
        assert!(record.stdout_summary.contains("showing head and tail"));
    }
}
