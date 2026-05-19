//! Command execution types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::secret::SecretSource;
use crate::shared::DomainError;

const NO_TIMEOUT: u64 = 0;

fn is_valid_command(command: &str) -> Result<(), DomainError> {
    if command.is_empty() {
        return Err(DomainError::Validation(
            "Command cannot be empty".into(),
        ));
    }
    Ok(())
}

/// Specification for a command to execute in a sandbox.
///
/// `timeout_ms = 0` means no timeout (infinite).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub secrets: HashMap<String, SecretSource>,
}

impl CommandSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: vec![],
            working_dir: None,
            env_vars: HashMap::new(),
            timeout_ms: None,
            secrets: HashMap::new(),
        }
    }

    pub fn try_new(command: impl Into<String>) -> Result<Self, DomainError> {
        let command = command.into();
        is_valid_command(&command)?;
        Ok(Self {
            command,
            args: vec![],
            working_dir: None,
            env_vars: HashMap::new(),
            timeout_ms: None,
            secrets: HashMap::new(),
        })
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn is_no_timeout(&self) -> bool {
        self.timeout_ms == Some(NO_TIMEOUT)
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    pub fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }
}

/// Result of a completed command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
    pub timed_out: bool,
}

impl CommandResult {
    pub fn success(stdout: Vec<u8>) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: vec![],
            duration_ms: 0,
            timed_out: false,
        }
    }

    pub fn failure(exit_code: i32, stderr: Vec<u8>) -> Self {
        Self {
            exit_code,
            stdout: vec![],
            stderr,
            duration_ms: 0,
            timed_out: false,
        }
    }

    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_spec_new_non_empty() {
        let spec = CommandSpec::new("echo hello");
        assert_eq!(spec.command(), "echo hello");
        assert!(spec.args().is_empty());
        assert!(spec.timeout_ms().is_none());
        assert!(!spec.is_no_timeout());
    }

    #[test]
    fn test_command_spec_try_new_rejects_empty() {
        let err = CommandSpec::try_new("").expect_err("empty command should be rejected");
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_command_spec_builder_chain() {
        let spec = CommandSpec::try_new("cargo")
            .unwrap()
            .with_args(vec!["build".to_string()])
            .with_working_dir("/workspace")
            .with_timeout(300_000);
        assert_eq!(spec.command(), "cargo");
        assert_eq!(spec.args(), &["build".to_string()]);
        assert_eq!(spec.working_dir(), Some("/workspace"));
        assert_eq!(spec.timeout_ms(), Some(300_000));
    }

    #[test]
    fn test_command_spec_no_timeout_flag() {
        let spec = CommandSpec::new("sleep").with_timeout(0);
        assert!(spec.is_no_timeout());
        assert_eq!(spec.timeout_ms(), Some(0));
    }

    #[test]
    fn test_command_result_is_success() {
        let r = CommandResult::success(b"output".to_vec());
        assert!(r.is_success());
        assert!(!r.timed_out());
    }

    #[test]
    fn test_command_result_failure() {
        let r = CommandResult::failure(1, b"error".to_vec());
        assert!(!r.is_success());
        assert!(!r.timed_out());
    }
}
