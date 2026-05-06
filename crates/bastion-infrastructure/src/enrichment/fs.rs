//! Sandbox-backed FileSystem implementation.
//!
//! Implements the `FileSystem` trait by executing commands inside the sandbox
//! via the `SandboxProvider` interface.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;

use enrichment_engine::traits::{EnrichmentError, FileSystem};

/// A `FileSystem` impl that executes read/find commands inside a sandbox.
#[derive(Debug)]
pub struct SandboxFileSystem {
    provider: Arc<dyn SandboxProvider>,
    sandbox_id: SandboxId,
}

impl SandboxFileSystem {
    /// Create a new SandboxFileSystem.
    pub fn new(provider: Arc<dyn SandboxProvider>, sandbox_id: SandboxId) -> Self {
        Self { provider, sandbox_id }
    }
}

#[async_trait]
impl FileSystem for SandboxFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String, EnrichmentError> {
        use bastion_domain::execution::command::CommandSpec;

        let cmd = CommandSpec::new(format!("cat {}", path));
        let result = self
            .provider
            .run_command(&self.sandbox_id, &cmd)
            .await
            .map_err(|e| EnrichmentError::FileSystem(e.to_string()))?;

        if result.exit_code != 0 {
            return Err(EnrichmentError::FileSystem(format!(
                "cat {} failed: {}",
                path,
                String::from_utf8_lossy(&result.stderr)
            )));
        }

        Ok(String::from_utf8_lossy(&result.stdout).to_string())
    }

    async fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>, EnrichmentError> {
        use bastion_domain::execution::command::CommandSpec;

        // Use find to execute glob pattern matching in the sandbox
        // Single-quoted pattern is passed literally to find -name (no shell glob expansion)
        let cmd = CommandSpec::new(format!("find . -name '{}' 2>/dev/null", pattern));

        let result = self
            .provider
            .run_command(&self.sandbox_id, &cmd)
            .await
            .map_err(|e| EnrichmentError::FileSystem(e.to_string()))?;

        if result.exit_code != 0 {
            return Ok(Vec::new());
        }

        let output = String::from_utf8_lossy(&result.stdout);
        let paths: Vec<PathBuf> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();

        Ok(paths)
    }
}
