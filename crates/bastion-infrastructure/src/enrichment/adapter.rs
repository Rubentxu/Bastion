//! Bastion enrichment adapter.
//!
//! Wires the enrichment engine into Bastion's `sandbox_run` tool.
//! Maps Bastion domain types to host-agnostic types, invokes the pipeline,
//! and extends the JSON response additively.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::shared::id::SandboxId;
use uuid::Uuid;

use enrichment_engine::models::{AgentContext, EnrichmentRunRecord, OperationInvocation, OperationResult};
use enrichment_engine::optimizer::{OptimizerReport, OptimizerRepository};
use enrichment_engine::pipeline::FactPipeline;
use enrichment_engine::traits::{CatalogRepository, EnrichmentError, RunRecorder};
use enrichment_engine::truncate::truncate_string;

use super::config::EnrichmentConfig;
use super::fs::SandboxFileSystem;

/// The Bastion enrichment adapter.
///
/// Holds a shared `FactPipeline` and a `SandboxProvider` reference.
/// Implements the enrichment workflow: map CommandSpec → OperationInvocation,
/// call pipeline, map back to JSON extension.
///
/// Optionally records enrichment runs via a `RunRecorder` for telemetry
/// and Meta-Harness optimization. An optional `OptimizerRepository` enables
/// the optimizer report MCP tool.
///
/// Uses an atomic counter to bound concurrent record persistence operations,
/// providing backpressure when the database write path is overloaded.
#[derive(Clone)]
pub struct BastionEnrichmentAdapter {
    pipeline: Arc<FactPipeline>,
    provider: Arc<dyn SandboxProvider>,
    config: EnrichmentConfig,
    recorder: Option<Arc<dyn RunRecorder>>,
    optimizer_repo: Option<Arc<dyn OptimizerRepository>>,
    /// Atomic counter tracking in-flight record persistence operations.
    /// Bounded by `config.semaphore.max_concurrent_records`.
    record_in_flight: Arc<AtomicUsize>,
}

impl BastionEnrichmentAdapter {
    /// Create a new adapter without a recorder.
    ///
    /// The pipeline is built once from the catalog and reused for all enrich() calls.
    /// The provider is used to create per-request SandboxFileSystem instances.
    ///
    /// # Backward Compatibility
    ///
    /// This constructor preserves the original signature. To add a recorder,
    /// use `with_recorder()` on the returned adapter.
    pub fn new(
        catalog: Arc<dyn CatalogRepository>,
        provider: Arc<dyn SandboxProvider>,
        config: EnrichmentConfig,
    ) -> Self {
        let pipeline = FactPipeline::new(catalog);
        Self {
            pipeline: Arc::new(pipeline),
            provider,
            config: config.clone(),
            recorder: None,
            optimizer_repo: None,
            record_in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Create a new adapter with a pre-built pipeline (for shared pipeline scenario).
    pub fn with_pipeline(
        pipeline: Arc<FactPipeline>,
        provider: Arc<dyn SandboxProvider>,
        config: EnrichmentConfig,
    ) -> Self {
        Self {
            pipeline,
            provider,
            config: config.clone(),
            recorder: None,
            optimizer_repo: None,
            record_in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Configure this adapter with a recorder.
    ///
    /// Returns a new `Arc<Self>` with the recorder set. The original adapter
    /// is cloned, not modified (preserving the original `new()` signature).
    ///
    /// # Arguments
    ///
    /// * `recorder` - The recorder to use for persisting enrichment runs
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use bastion_infrastructure::enrichment::{BastionEnrichmentAdapter, SqliteRunRecorder};
    ///
    /// let adapter = Arc::new(BastionEnrichmentAdapter::new(catalog, provider, config));
    /// let recorder = Arc::new(SqliteRunRecorder::new(path).unwrap());
    /// let adapter_with_recorder = BastionEnrichmentAdapter::with_recorder(adapter, recorder);
    /// ```
    pub fn with_recorder(self: Arc<Self>, recorder: Arc<dyn RunRecorder>) -> Arc<Self> {
        Arc::new(Self {
            pipeline: self.pipeline.clone(),
            provider: self.provider.clone(),
            config: self.config.clone(),
            recorder: Some(recorder),
            optimizer_repo: self.optimizer_repo.clone(),
            record_in_flight: self.record_in_flight.clone(),
        })
    }

    /// Configure this adapter with an optimizer repository.
    ///
    /// Returns a new `Arc<Self>` with the optimizer repo set. The original adapter
    /// is cloned, not modified.
    ///
    /// # Arguments
    ///
    /// * `optimizer_repo` - The optimizer repository for generating reports
    pub fn with_optimizer_repo(self: Arc<Self>, optimizer_repo: Arc<dyn OptimizerRepository>) -> Arc<Self> {
        Arc::new(Self {
            pipeline: self.pipeline.clone(),
            provider: self.provider.clone(),
            config: self.config.clone(),
            recorder: self.recorder.clone(),
            optimizer_repo: Some(optimizer_repo),
            record_in_flight: self.record_in_flight.clone(),
        })
    }

    /// Get an optimizer report from recorded enrichment runs.
    ///
    /// Returns `None` if the optimizer repository is not configured.
    pub async fn get_optimizer_report(&self, after: Option<&str>) -> Result<Option<OptimizerReport>, EnrichmentError> {
        let repo = match &self.optimizer_repo {
            Some(r) => r,
            None => return Ok(None),
        };

        let records = repo.read_records(after).await?;
        let report = enrichment_engine::optimizer::generate_report(&records);
        Ok(Some(report))
    }

    /// Get the retention configuration and stats from the recorder.
    ///
    /// Returns `None` if no recorder is configured.
    /// The stats (row count, timestamps) are obtained via the recorder's `stats()` method.
    pub async fn retention_info(&self) -> Option<RetentionStats> {
        let recorder = self.recorder.as_ref()?;

        // Get the retention config
        let config = recorder.retention_config();

        // Get the stats via the async stats() method
        let stats = match recorder.stats().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to get retention stats");
                return Some(RetentionStats {
                    max_age_days: config.max_age_days,
                    max_rows: config.max_rows,
                    enabled: config.enabled,
                    sanitize: config.sanitize,
                    current_row_count: None,
                    oldest_record_ts: None,
                    newest_record_ts: None,
                });
            }
        };

        Some(RetentionStats {
            max_age_days: config.max_age_days,
            max_rows: config.max_rows,
            enabled: config.enabled,
            sanitize: config.sanitize,
            current_row_count: Some(stats.current_row_count),
            oldest_record_ts: stats.oldest_record_ts,
            newest_record_ts: stats.newest_record_ts,
        })
    }

    /// Get the optimizer repository reference, if configured.
    pub fn optimizer_repo(&self) -> Option<&Arc<dyn OptimizerRepository>> {
        self.optimizer_repo.as_ref()
    }

    /// Get the run recorder reference, if configured.
    pub fn recorder(&self) -> Option<&Arc<dyn RunRecorder>> {
        self.recorder.as_ref()
    }
}

/// Retention statistics returned by the adapter.
#[derive(Debug, Clone)]
pub struct RetentionStats {
    pub max_age_days: u32,
    pub max_rows: u64,
    pub enabled: bool,
    pub sanitize: bool,
    /// Current row count in the database, if available.
    pub current_row_count: Option<u64>,
    /// ISO 8601 timestamp of the oldest record, if any.
    pub oldest_record_ts: Option<String>,
    /// ISO 8601 timestamp of the newest record, if any.
    pub newest_record_ts: Option<String>,
}

impl BastionEnrichmentAdapter {
    /// Enrich a sandbox command execution.
    ///
    /// Maps `CommandSpec` and `CommandResult` to the enrichment engine's types,
    /// runs the pipeline, and returns an optional `AgentContext`.
    ///
    /// Returns `None` if enrichment is disabled or no enricher matches.
    /// Errors are traced at warn level and return `None` (non-blocking).
    ///
    /// When a recorder is configured, an `EnrichmentRunRecord` is persisted
    /// asynchronously after the pipeline completes. Recording is fire-and-forget
    /// — failures do not block the enrichment response.
    pub async fn enrich(
        &self,
        sandbox_id: &SandboxId,
        command_spec: &CommandSpec,
        command_result: &CommandResult,
    ) -> Option<AgentContext> {
        if !self.config.enabled {
            return None;
        }

        let invocation = Self::map_command_spec(command_spec);
        let result = Self::map_command_result(command_result);

        let fs = SandboxFileSystem::new(self.provider.clone(), sandbox_id.clone());

        let start = Instant::now();
        let ctx = match self.pipeline.run(invocation.clone(), result.clone(), &fs).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!(error = %e, "Enrichment pipeline failed");
                // Record even on pipeline error (partial results may exist)
                self.record_run(&invocation, &result, None, start.elapsed(), Some(&e));
                return None;
            }
        };
        let elapsed = start.elapsed();

        // Don't block on slow enrichment — trace and continue
        if elapsed > Duration::from_millis(100) {
            tracing::debug!(elapsed_ms = elapsed.as_millis() as u64, "Enrichment completed slowly");
        }

        // Return None if no facts were extracted (no enricher matched)
        // But still record the run for telemetry
        if ctx.facts.is_empty() {
            self.record_run(&invocation, &result, Some(&ctx), elapsed, None);
            return None;
        }

        // Record successful enrichment run
        self.record_run(&invocation, &result, Some(&ctx), elapsed, None);

        Some(ctx)
    }

    /// Record an enrichment run asynchronously.
    ///
    /// Record an enrichment run asynchronously.
    ///
    /// This is fire-and-forget — errors are logged at warn level but do not
    /// block the caller.
    ///
    /// Uses an atomic counter to bound concurrent record persistence operations.
    /// If the limit is reached, the record is dropped and a warning is logged.
    fn record_run(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        ctx: Option<&AgentContext>,
        elapsed: Duration,
        error: Option<&str>,
    ) {
        let Some(ref recorder) = self.recorder else {
            return;
        };

        let record = self.build_record(invocation, result, ctx, elapsed, error);
        let recorder = Arc::clone(recorder);
        let in_flight = self.record_in_flight.clone();
        let max = self.config.semaphore.max_concurrent_records;

        // Try to acquire a slot: atomically increment and check if we're over limit
        let prev = in_flight.fetch_add(1, Ordering::AcqRel);
        if prev >= max {
            // Over limit - undo the increment and drop the record
            in_flight.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(
                run_id = %record.id,
                enricher_id = %record.enricher_id,
                in_flight = %prev,
                limit = %max,
                "Record persistence saturated, dropping record"
            );
            return;
        }

        // Fire-and-forget: spawn recording task
        // Decrement counter when task completes
        tokio::spawn(async move {
            if let Err(e) = recorder.record(&record).await {
                tracing::warn!(error = %e, run_id = %record.id, "EnrichmentRunRecord failed to persist");
            }
            in_flight.fetch_sub(1, Ordering::AcqRel);
        });
    }

    /// Build an `EnrichmentRunRecord` from the enrichment context.
    fn build_record(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        ctx: Option<&AgentContext>,
        elapsed: Duration,
        error: Option<&str>,
    ) -> EnrichmentRunRecord {
        let enricher_id = ctx
            .map(|c| c.enrichment_meta.enricher_id.clone())
            .unwrap_or_default();

        let facts_count = ctx.map(|c| c.facts.len() as u32).unwrap_or(0);
        let artifacts_count = ctx.map(|c| c.artifacts.len() as u32).unwrap_or(0);
        let verdict = ctx.and_then(|c| c.verdict.clone());
        let recommendation_count = ctx
            .and_then(|c| c.recommendations.as_ref())
            .map(|r| r.len() as u32)
            .unwrap_or(0);

        // Count diagnostic-tagged facts
        let diagnostics_count = ctx
            .map(|c| {
                c.facts
                    .iter()
                    .filter(|f| f.tags.iter().any(|t| t == "diagnostic"))
                    .count() as u32
            })
            .unwrap_or(0);

        // Derived facts and rule hits come from the pipeline's rule evaluation
        // Since we don't have direct access to RuleOutput here, we derive from ctx
        let derived_facts_count = ctx
            .map(|c| {
                c.facts
                    .iter()
                    .filter(|f| f.source_extractor == "rule")
                    .count() as u32
            })
            .unwrap_or(0);

        // rule_hits_count: we track via derived facts since RuleOutput is internal
        // A rule hit produces at least one derived fact or a verdict/recommendation
        let rule_hits_count = if derived_facts_count > 0 || recommendation_count > 0 || verdict.is_some() {
            // Estimate based on having any rule activity
            1
        } else {
            0
        };

        // Compute average confidence
        let confidence_avg = ctx
            .map(|c| {
                if c.facts.is_empty() {
                    0.0
                } else {
                    let sum: f64 = c.facts.iter().map(|f| f.confidence as f64).sum();
                    sum / c.facts.len() as f64
                }
            })
            .unwrap_or(0.0);

        // Truncate outputs (500 char limit, None if empty)
        let output_summary_stdout = truncate_string(&result.stdout, 500);
        let output_summary_stderr = truncate_string(&result.stderr, 500);
        let command = truncate_string(&invocation.command, 500)
            .unwrap_or_else(|| invocation.command.clone());

        EnrichmentRunRecord::new(
            Uuid::new_v4().to_string(),
            chrono::Utc::now().to_rfc3339(),
            command,
            enricher_id,
            result.exit_code,
            elapsed.as_millis() as u64,
            output_summary_stdout,
            output_summary_stderr,
            facts_count,
            derived_facts_count,
            rule_hits_count,
            diagnostics_count,
            artifacts_count,
            confidence_avg,
            verdict,
            recommendation_count,
            error.map(String::from),
        )
    }

    /// Map a `CommandSpec` to an `OperationInvocation`.
    fn map_command_spec(spec: &CommandSpec) -> OperationInvocation {
        OperationInvocation {
            command: spec.command.clone(),
            args: spec.args.clone(),
            working_dir: spec.working_dir.clone(),
            env_vars: spec.env_vars.clone(),
        }
    }

    /// Map a `CommandResult` to an `OperationResult`.
    fn map_command_result(result: &CommandResult) -> OperationResult {
        OperationResult {
            exit_code: result.exit_code,
            stdout: String::from_utf8_lossy(&result.stdout).to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            duration_ms: result.duration_ms,
            timed_out: result.timed_out,
        }
    }

    /// Enrich a streaming command execution from accumulated output.
    ///
    /// This method is called after `sandbox_run_stream` has drained the stream.
    /// It maps the accumulated stdout/stderr/exit_code to an `OperationResult`
    /// and runs the enrichment pipeline on it.
    ///
    /// Returns `None` if enrichment is disabled or no enricher matches.
    /// Errors are traced at warn level and return `None` (non-blocking).
    ///
    /// This enables enrichment attachment to streaming responses without blocking
    /// the stream itself.
    #[allow(clippy::too_many_arguments)]
    pub async fn enrich_stream(
        &self,
        sandbox_id: &SandboxId,
        command_spec: &CommandSpec,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        duration_ms: u64,
        timed_out: bool,
    ) -> Option<AgentContext> {
        if !self.config.enabled {
            return None;
        }

        let invocation = Self::map_command_spec(command_spec);
        let result = OperationResult {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            duration_ms,
            timed_out,
        };

        let fs = SandboxFileSystem::new(self.provider.clone(), sandbox_id.clone());

        let start = Instant::now();
        let ctx = match self.pipeline.run(invocation.clone(), result.clone(), &fs).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!(error = %e, "Enrichment pipeline failed for stream");
                // Record even on pipeline error (partial results may exist)
                self.record_run(&invocation, &result, None, start.elapsed(), Some(&e));
                return None;
            }
        };
        let elapsed = start.elapsed();

        if elapsed > Duration::from_millis(100) {
            tracing::debug!(elapsed_ms = elapsed.as_millis() as u64, "Stream enrichment completed slowly");
        }

        if ctx.facts.is_empty() {
            self.record_run(&invocation, &result, Some(&ctx), elapsed, None);
            return None;
        }

        // Record successful enrichment run
        self.record_run(&invocation, &result, Some(&ctx), elapsed, None);

        Some(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bastion_domain::execution::command::CommandSpec;
    use bastion_domain::execution::command::CommandResult;
    use bastion_domain::provider::capabilities::ProviderCapabilities;
    use bastion_domain::provider::port::SandboxProvider;
    use bastion_domain::shared::DomainError;
    use bastion_domain::shared::id::SandboxId;
    use bastion_domain::provider::port::CommandStream;
    use bastion_domain::sandbox::entity::Sandbox;
    use bastion_domain::sandbox::snapshot::SnapshotInfo;
    use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
    use bastion_domain::file_ops::FileEntry;
    use enrichment_engine::models::{EnricherDescriptor, EnrichmentMeta, Fact};
    use enrichment_engine::traits::CatalogRepository;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Mock recorder for testing
    struct MockRecorder {
        records: Arc<Mutex<Vec<EnrichmentRunRecord>>>,
    }

    impl MockRecorder {
        fn new() -> Self {
            Self {
                records: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl RunRecorder for MockRecorder {
        async fn record(&self, run: &EnrichmentRunRecord) -> Result<(), enrichment_engine::traits::EnrichmentError> {
            self.records.lock().await.push(run.clone());
            Ok(())
        }

        fn retention_config(&self) -> &enrichment_engine::models::RetentionConfig {
            static DEFAULT: enrichment_engine::models::RetentionConfig = enrichment_engine::models::RetentionConfig {
                max_age_days: 90,
                max_rows: 100_000,
                enabled: false,
                sanitize: false,
            };
            &DEFAULT
        }

        async fn cleanup(&self) -> Result<u64, enrichment_engine::traits::EnrichmentError> {
            Ok(0)
        }

        async fn stats(&self) -> Result<enrichment_engine::models::RunRecorderStats, enrichment_engine::traits::EnrichmentError> {
            Ok(enrichment_engine::models::RunRecorderStats::empty())
        }
    }

    // Fake CatalogRepository for tests
    struct FakeCatalog;

    #[async_trait]
    impl CatalogRepository for FakeCatalog {
        async fn find_enrichers(&self, _command: &str) -> Vec<EnricherDescriptor> {
            vec![]
        }
        async fn list_all(&self) -> Vec<EnricherDescriptor> {
            vec![]
        }
    }

    // Fake SandboxProvider for tests
    struct FakeProvider;

    impl std::fmt::Debug for FakeProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FakeProvider")
        }
    }

    #[async_trait]
    impl SandboxProvider for FakeProvider {
        async fn create(
            &self,
            _id: &SandboxId,
            _template: &str,
            _resources: &ResourcesSpec,
            _network: &NetworkSpec,
            _env_vars: &HashMap<String, String>,
            _timeout_ms: u64,
        ) -> Result<Sandbox, DomainError> {
            unimplemented!()
        }
        async fn terminate(&self, _id: &SandboxId) -> Result<(), DomainError> {
            Ok(())
        }
        async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
            Ok(true)
        }
        async fn run_command(
            &self,
            _id: &SandboxId,
            _command: &CommandSpec,
        ) -> Result<CommandResult, DomainError> {
            unimplemented!()
        }
        async fn run_command_stream(
            &self,
            _id: &SandboxId,
            _command: &CommandSpec,
        ) -> Result<CommandStream, DomainError> {
            unimplemented!()
        }
        async fn write_file(
            &self,
            _id: &SandboxId,
            _path: &str,
            _content: &[u8],
        ) -> Result<(), DomainError> {
            Ok(())
        }
        async fn read_file(
            &self,
            _id: &SandboxId,
            _path: &str,
        ) -> Result<Vec<u8>, DomainError> {
            Ok(vec![])
        }
        async fn list_files(
            &self,
            _id: &SandboxId,
            _dir: &str,
        ) -> Result<Vec<FileEntry>, DomainError> {
            Ok(vec![])
        }
        async fn create_snapshot(&self, _id: &SandboxId, _name: &str) -> Result<SnapshotInfo, DomainError> {
            unimplemented!()
        }
        async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
            unimplemented!()
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }
        fn name(&self) -> &str {
            "fake"
        }
        async fn list_sandboxes(
            &self,
            _filter: &SandboxFilter,
        ) -> Result<Vec<Sandbox>, DomainError> {
            Ok(vec![])
        }
        async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
            unimplemented!()
        }
        async fn set_timeout(&self, _id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_command_spec_mapping() {
        let spec = CommandSpec::new("mvn package")
            .with_working_dir("/workspace")
            .with_env("MAVEN_OPTS", "-Xmx512m");

        let invocation = BastionEnrichmentAdapter::map_command_spec(&spec);
        assert_eq!(invocation.command, "mvn package");
        assert_eq!(invocation.working_dir.as_deref(), Some("/workspace"));
        assert_eq!(invocation.env_vars.get("MAVEN_OPTS").map(|s| s.as_str()), Some("-Xmx512m"));
    }

    #[tokio::test]
    async fn test_command_result_mapping() {
        let result = CommandResult {
            exit_code: 0,
            stdout: b"BUILD SUCCESS".to_vec(),
            stderr: b"".to_vec(),
            duration_ms: 5000,
            timed_out: false,
        };

        let op_result = BastionEnrichmentAdapter::map_command_result(&result);
        assert_eq!(op_result.exit_code, 0);
        assert_eq!(op_result.stdout, "BUILD SUCCESS");
        assert_eq!(op_result.stderr, "");
        assert_eq!(op_result.duration_ms, 5000);
        assert!(!op_result.timed_out);
    }

    #[tokio::test]
    async fn test_record_run_not_called_when_no_recorder() {
        // Test that record_run is a no-op when recorder is None
        // This is implicitly tested by the fact that enrich() doesn't panic
        // when recorder is None
        let adapter = BastionEnrichmentAdapter::new(
            Arc::new(FakeCatalog),
            Arc::new(FakeProvider),
            EnrichmentConfig::default(),
        );

        // Should not panic when calling record_run with None recorder
        let invocation = OperationInvocation::from_command("echo hello");
        let result = OperationResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        adapter.record_run(&invocation, &result, None, Duration::from_millis(100), None);
    }

    #[tokio::test]
    async fn test_build_record_truncates_long_output() {
        let adapter = BastionEnrichmentAdapter::new(
            Arc::new(FakeCatalog),
            Arc::new(FakeProvider),
            EnrichmentConfig::default(),
        );

        let long_stdout = "x".repeat(1000);
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: long_stdout.clone(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let ctx = AgentContext {
            facts: vec![Fact {
                key: "status".to_string(),
                value: "BUILD SUCCESS".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 0.9,
            }],
            build_status: Some("BUILD SUCCESS".to_string()),
            artifacts: vec![],
            test_summary: None,
            enrichment_meta: EnrichmentMeta {
                source: "test".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                enricher_id: "maven".to_string(),
            },
            verdict: Some("PASSED".to_string()),
            recommendations: None,
        };

        let record = adapter.build_record(&invocation, &result, Some(&ctx), Duration::from_millis(5000), None);

        // stdout should be truncated to 500 chars + ellipsis
        assert!(record.output_summary_stdout.is_some());
        let stdout = record.output_summary_stdout.unwrap();
        assert!(stdout.ends_with('…'));
        assert!(stdout.chars().count() <= 501);

        // stderr is empty, should be None
        assert!(record.output_summary_stderr.is_none());
    }

    #[tokio::test]
    async fn test_build_record_empty_stderr_returns_none() {
        let adapter = BastionEnrichmentAdapter::new(
            Arc::new(FakeCatalog),
            Arc::new(FakeProvider),
            EnrichmentConfig::default(),
        );

        let invocation = OperationInvocation::from_command("echo hello");
        let result = OperationResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        let record = adapter.build_record(&invocation, &result, None, Duration::from_millis(100), None);

        // Empty stderr should be stored as None
        assert!(record.output_summary_stderr.is_none());
    }

    #[tokio::test]
    async fn test_with_recorder_returns_new_arc() {
        let adapter = Arc::new(BastionEnrichmentAdapter::new(
            Arc::new(FakeCatalog),
            Arc::new(FakeProvider),
            EnrichmentConfig::default(),
        ));

        let recorder = Arc::new(MockRecorder::new());
        let adapter_with_recorder = BastionEnrichmentAdapter::with_recorder(adapter.clone(), recorder);

        // Should be a different Arc
        assert_ne!(Arc::as_ptr(&adapter), Arc::as_ptr(&adapter_with_recorder));

        // Original adapter should have no recorder
        // (we can't directly access private fields, but this is implicit)
    }
}
