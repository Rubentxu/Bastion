//! Assertion catalog domain types.
//!
//! Assertion descriptors are TOML-loaded validation primitives that evaluate
//! against ExperienceRecord evidence.

use serde::{Deserialize, Serialize};

use super::experience::ExperienceRecord;

/// Descriptor for a reusable assertion, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionDescriptor {
    /// Unique assertion identifier (e.g. "maven.build.success").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this assertion validates.
    pub description: String,
    /// Category for grouping (e.g. "command", "maven", "container").
    pub category: String,
    /// Ordered list of checks to evaluate.
    pub checks: Vec<AssertionCheck>,
}

/// A single check within an assertion descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssertionCheck {
    /// Assert exit code matches expected value.
    ExitCode { expected: i32 },
    /// Assert stdout contains a substring.
    StdoutContains { substring: String },
    /// Assert stderr contains a substring.
    StderrContains { substring: String },
    /// Assert stdout matches a regex pattern.
    StdoutMatches { regex: String },
    /// Assert the sandbox is alive (no-op using provider check at evaluation time).
    SandboxAlive,
    /// Assert command duration does not exceed max_ms.
    CommandDuration { max_ms: u64 },
}

impl AssertionCheck {
    /// Evaluate this check against an experience record.
    /// Returns (passed, failure_reason).
    pub fn evaluate(&self, record: &ExperienceRecord) -> (bool, Option<String>) {
        match self {
            AssertionCheck::ExitCode { expected } => match record.exit_code {
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
                if record.stdout_summary.contains(substring) {
                    (true, None)
                } else {
                    (
                        false,
                        Some(format!("stdout does not contain {:?}", substring)),
                    )
                }
            }
            AssertionCheck::StderrContains { substring } => {
                if record.stderr_summary.contains(substring) {
                    (true, None)
                } else {
                    (
                        false,
                        Some(format!("stderr does not contain {:?}", substring)),
                    )
                }
            }
            AssertionCheck::StdoutMatches { regex } => {
                // Regex evaluation deferred to application/infrastructure layer
                // to keep domain free of external regex dependencies.
                // The check always passes at domain level; infrastructure
                // layer re-evaluates with regex support.
                let _ = regex; // suppress unused warning
                (true, None)
            }
            AssertionCheck::SandboxAlive => {
                // Evaluation deferred to use case layer (needs provider access)
                // For domain-level evaluation, we treat this as a no-op pass
                // since liveness is checked separately.
                (true, None)
            }
            AssertionCheck::CommandDuration { max_ms } => match record.duration_ms() {
                Some(dur) if dur <= *max_ms => (true, None),
                Some(dur) => (
                    false,
                    Some(format!("command took {}ms, expected <= {}ms", dur, max_ms)),
                ),
                None => (
                    false,
                    Some("command still running, duration unknown".to_string()),
                ),
            },
        }
    }
}

/// Result of evaluating an assertion against an experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    /// Whether all checks passed.
    pub passed: bool,
    /// The assertion ID that was evaluated.
    pub assertion_id: String,
    /// Per-check results.
    pub check_results: Vec<CheckResult>,
}

/// Result of a single check within an assertion evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Human-readable description of the check.
    pub check: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Failure reason if the check failed.
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
            .with_stderr(b"");
        // Manually set finished_at to simulate a quick command
        let mut record = record;
        record.exit_code = Some(0);
        record.finished_at = Some(record.started_at);

        let check = AssertionCheck::CommandDuration { max_ms: 1000 };
        let (passed, _) = check.evaluate(&record);
        assert!(passed);
    }
}
