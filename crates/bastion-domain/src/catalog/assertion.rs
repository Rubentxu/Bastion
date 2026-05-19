//! Assertion catalog domain types.
//!
//! Assertion descriptors are TOML-loaded validation primitives that evaluate
//! against ExperienceRecord evidence.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::experience::ExperienceRecord;
use crate::shared::DomainError;

pub mod evaluator {
    pub use super::AssertionEvaluator;
}

/// Descriptor for a reusable assertion, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub checks: Vec<AssertionCheck>,
}

/// A single check within an assertion descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssertionCheck {
    ExitCode { expected: i32 },
    StdoutContains { substring: String },
    StderrContains { substring: String },
    StdoutMatches { regex: String },
    SandboxAlive,
    CommandDuration { max_ms: u64 },
}

impl AssertionCheck {
    /// Evaluate this check against an experience record.
    /// Simple checks (ExitCode, Contains, Duration) are evaluated synchronously.
    /// Regex and SandboxAlive checks are deferred to AssertionEvaluator.
    pub fn evaluate(&self, record: &ExperienceRecord) -> (bool, Option<String>) {
        match self {
            AssertionCheck::ExitCode { expected } => match record.exit_code() {
                Some(code) if code == *expected => (true, None),
                Some(code) => (
                    false,
                    Some(format!("expected exit code {}, got {}", expected, code)),
                ),
                None => (
                    false,
                    Some(format!("expected exit code {}, got none", expected)),
                ),
            },
            AssertionCheck::StdoutContains { substring } => {
                if record.stdout_summary().contains(substring) {
                    (true, None)
                } else {
                    (false, Some(format!("stdout does not contain {:?}", substring)))
                }
            }
            AssertionCheck::StderrContains { substring } => {
                if record.stderr_summary().contains(substring) {
                    (true, None)
                } else {
                    (false, Some(format!("stderr does not contain {:?}", substring)))
                }
            }
            AssertionCheck::StdoutMatches { .. } => {
                (true, None)
            }
            AssertionCheck::SandboxAlive => {
                (true, None)
            }
            AssertionCheck::CommandDuration { max_ms } => match record.duration_ms() {
                Some(dur) if dur <= *max_ms => (true, None),
                Some(dur) => (
                    false,
                    Some(format!("command took {}ms, expected <= {}ms", dur, max_ms)),
                ),
                None => (false, Some("command still running, duration unknown".to_string())),
            },
        }
    }
}

#[async_trait]
pub trait AssertionEvaluator: Send + Sync {
    async fn evaluate(&self, check: &AssertionCheck, record: &ExperienceRecord) -> Result<(bool, Option<String>), DomainError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    pub passed: bool,
    pub assertion_id: String,
    pub check_results: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub check: String,
    pub passed: bool,
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::experience::ExperienceRecord;

    fn make_record(exit_code: Option<i32>, stdout: &str, stderr: &str) -> ExperienceRecord {
        ExperienceRecord::new("sandbox_run")
            .completed(exit_code.unwrap_or(0))
            .with_stdout(stdout.as_bytes())
            .with_stderr(stderr.as_bytes())
    }

    #[test]
    fn test_exit_code_check_passes() {
        let record = make_record(Some(0), "", "");
        let check = AssertionCheck::ExitCode { expected: 0 };
        let (passed, reason) = check.evaluate(&record);
        assert!(passed);
        assert!(reason.is_none());
    }

    #[test]
    fn test_exit_code_check_fails() {
        let record = make_record(Some(1), "", "");
        let check = AssertionCheck::ExitCode { expected: 0 };
        let (passed, reason) = check.evaluate(&record);
        assert!(!passed);
        assert!(reason.is_some());
    }

    #[test]
    fn test_stdout_contains_passes() {
        let record = make_record(Some(0), "BUILD SUCCESS", "");
        let check = AssertionCheck::StdoutContains {
            substring: "BUILD SUCCESS".to_string(),
        };
        let (passed, _) = check.evaluate(&record);
        assert!(passed);
    }

    #[test]
    fn test_stdout_contains_fails() {
        let record = make_record(Some(0), "BUILD FAILURE", "");
        let check = AssertionCheck::StdoutContains {
            substring: "BUILD SUCCESS".to_string(),
        };
        let (passed, reason) = check.evaluate(&record);
        assert!(!passed);
        assert!(reason.is_some());
    }

    #[test]
    fn test_command_duration_passes() {
        let record = ExperienceRecord::new("sandbox_run")
            .with_trace_id("trace1")
            .with_stdout(b"done")
            .with_stderr(b"")
            .completed(0);

        let check = AssertionCheck::CommandDuration { max_ms: 1000 };
        let (passed, _) = check.evaluate(&record);
        assert!(passed);
    }
}
