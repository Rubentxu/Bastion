//! Compatibility shim — blanket impl of SandboxProvider for segregated traits.

mod shim {
    use async_trait::async_trait;
    use futures::Stream;
    use std::collections::HashMap;
    use std::path::Path;
    use std::pin::Pin;

    use crate::execution::command::{CommandResult, CommandSpec};
    use crate::execution::stream::CommandChunk;
    use crate::file_ops::FileEntry;
    use crate::provider::capabilities::ProviderCapabilities;
    use crate::sandbox::entity::Sandbox;
    use crate::sandbox::snapshot::SnapshotInfo;
    use crate::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
    use crate::shared::DomainError;
    use crate::shared::id::SandboxId;

    use super::super::executor::TaskExecutor;
    use super::super::lifecycle::SandboxLifecycle;
    use super::super::port::SandboxProvider;

    /// Stream type for command output chunks.
    pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>;

    /// Blanket impl of SandboxProvider for types that implement both
    /// SandboxLifecycle and TaskExecutor.
    ///
    /// This provides binary compatibility with existing code that uses
    /// the unified SandboxProvider trait while enabling the segregated
    /// trait architecture.
    #[async_trait]
    impl<T: SandboxLifecycle + TaskExecutor> SandboxProvider for T {
        // ── Lifecycle ──────────────────────────────────────────────

        async fn create(
            &self,
            id: &SandboxId,
            template: &str,
            resources: &ResourcesSpec,
            network: &NetworkSpec,
            env_vars: &HashMap<String, String>,
            timeout_ms: u64,
        ) -> Result<Sandbox, DomainError> {
            SandboxLifecycle::create(self, id, template, resources, network, env_vars, timeout_ms)
                .await
        }

        async fn terminate(&self, id: &SandboxId) -> Result<(), DomainError> {
            SandboxLifecycle::terminate(self, id).await
        }

        async fn is_alive(&self, id: &SandboxId) -> Result<bool, DomainError> {
            SandboxLifecycle::is_alive(self, id).await
        }

        // ── Execution ──────────────────────────────────────────────

        async fn run_command(
            &self,
            id: &SandboxId,
            command: &CommandSpec,
        ) -> Result<CommandResult, DomainError> {
            TaskExecutor::run_command(self, id, command).await
        }

        async fn run_command_stream(
            &self,
            id: &SandboxId,
            command: &CommandSpec,
        ) -> Result<CommandStream, DomainError> {
            TaskExecutor::run_command_stream(self, id, command).await
        }

        async fn cancel_command(
            &self,
            id: &SandboxId,
            grace_period_ms: u64,
        ) -> Result<bool, DomainError> {
            TaskExecutor::cancel_command(self, id, grace_period_ms).await
        }

        // ── File Operations ────────────────────────────────────────

        async fn write_file(
            &self,
            id: &SandboxId,
            path: &str,
            content: &[u8],
        ) -> Result<(), DomainError> {
            TaskExecutor::write_file(self, id, path, content).await
        }

        async fn read_file(&self, id: &SandboxId, path: &str) -> Result<Vec<u8>, DomainError> {
            TaskExecutor::read_file(self, id, path).await
        }

        async fn list_files(
            &self,
            id: &SandboxId,
            dir: &str,
        ) -> Result<Vec<FileEntry>, DomainError> {
            TaskExecutor::list_files(self, id, dir).await
        }

        async fn copy_to(
            &self,
            id: &SandboxId,
            host_dir: &Path,
            target: &str,
        ) -> Result<(), DomainError> {
            TaskExecutor::copy_to(self, id, host_dir, target).await
        }

        // ── Snapshot Operations ─────────────────────────────────────

        async fn create_snapshot(
            &self,
            id: &SandboxId,
            name: &str,
        ) -> Result<SnapshotInfo, DomainError> {
            SandboxLifecycle::create_snapshot(self, id, name).await
        }

        async fn restore_snapshot(&self, snapshot_id: &str) -> Result<Sandbox, DomainError> {
            SandboxLifecycle::restore_snapshot(self, snapshot_id).await
        }

        async fn snapshot_exists(&self, snapshot_id: &str) -> Result<bool, DomainError> {
            SandboxLifecycle::snapshot_exists(self, snapshot_id).await
        }

        async fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), DomainError> {
            SandboxLifecycle::delete_snapshot(self, snapshot_id).await
        }

        async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>, DomainError> {
            SandboxLifecycle::list_snapshots(self).await
        }

        // ── Metadata ───────────────────────────────────────────────

        fn capabilities(&self) -> ProviderCapabilities {
            SandboxLifecycle::capabilities(self)
        }

        fn name(&self) -> &str {
            SandboxLifecycle::name(self)
        }

        // ── Provider-scoped Operations ────────────────────────────────

        async fn list_sandboxes(
            &self,
            filter: &SandboxFilter,
        ) -> Result<Vec<Sandbox>, DomainError> {
            SandboxLifecycle::list_sandboxes(self, filter).await
        }

        async fn get_info(&self, id: &SandboxId) -> Result<Sandbox, DomainError> {
            SandboxLifecycle::get_info(self, id).await
        }

        async fn set_timeout(&self, id: &SandboxId, timeout_ms: u64) -> Result<(), DomainError> {
            SandboxLifecycle::set_timeout(self, id, timeout_ms).await
        }
    }
}

pub use shim::*;
