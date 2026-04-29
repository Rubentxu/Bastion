//! Command execution types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Specification for a command to execute in a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

impl CommandSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: vec![],
            working_dir: None,
            env_vars: HashMap::new(),
            timeout_ms: None,
        }
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
}
