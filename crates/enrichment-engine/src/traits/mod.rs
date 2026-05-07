//! Core traits for the enrichment engine.
//!
//! These traits define the ports through which the host-agnostic core
//! interacts with external systems (catalog storage, file system, fact store).

use async_trait::async_trait;
use std::path::PathBuf;

use crate::models::{EnricherDescriptor, OperationInvocation, OperationResult, Fact, RuleConfig, EnrichmentRunRecord, RunRecorderStats};

/// Errors that can occur in enrichment operations.
#[derive(Debug, thiserror::Error)]
pub enum EnrichmentError {
    #[error("File system error: {0}")]
    FileSystem(String),
    #[error("Catalog error: {0}")]
    Catalog(String),
    #[error("Extraction error: {0}")]
    Extraction(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("Recorder error: {0}")]
    Recorder(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Repository for enricher descriptors.
#[async_trait]
pub trait CatalogRepository: Send + Sync {
    /// Find enrichers that match the given command string.
    async fn find_enrichers(&self, command: &str) -> Vec<EnricherDescriptor>;

    /// List all available enrichers.
    async fn list_all(&self) -> Vec<EnricherDescriptor>;
}

/// Persistent store for extracted facts.
#[async_trait]
pub trait FactStore: Send + Sync {
    /// Store facts for a command invocation.
    async fn store(&self, invocation: &OperationInvocation, facts: &[Fact]) -> Result<(), EnrichmentError>;

    /// Query stored facts for a command invocation.
    async fn query(&self, invocation: &OperationInvocation) -> Result<Vec<Fact>, EnrichmentError>;
}

/// File system abstraction for use by extractors.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Read a file as a string.
    async fn read_to_string(&self, path: &str) -> Result<String, EnrichmentError>;

    /// Find files matching a glob pattern.
    async fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>, EnrichmentError>;
}

/// Trait for fact extractors.
#[async_trait]
pub trait Extractor: Send + Sync {
    /// Human-readable name of this extractor.
    fn name(&self) -> &str;

    /// Extract facts from an operation result.
    ///
    /// The `fs` parameter allows extractors to perform additional file system
    /// lookups (e.g., glob for artifacts).
    async fn extract(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Vec<Fact>;
}

/// Repository for rule configurations.
#[async_trait]
pub trait RuleRepository: Send + Sync {
    /// Find all enabled rules for the given enricher, ordered by priority (ascending).
    async fn find_rules(&self, enricher_id: &str) -> Vec<RuleConfig>;

    /// List all rules across all enrichers.
    async fn list_all_rules(&self) -> Vec<RuleConfig>;
}

/// Recorder trait for persisting enrichment run records.
///
/// Implementations are responsible for storing `EnrichmentRunRecord`s
/// to a backing store (e.g., SQLite, PostgreSQL, etc.).
///
/// The recorder is fire-and-forget in the adapter — failures are logged
/// but do not block the enrichment pipeline.
#[async_trait]
pub trait RunRecorder: Send + Sync {
    /// Record an enrichment run.
    ///
    /// The implementation should persist the record to its backing store.
    /// Errors should be logged but not propagated — the adapter handles
    /// failure reporting via `tracing::warn!`.
    async fn record(&self, run: &EnrichmentRunRecord) -> Result<(), EnrichmentError>;

    /// Get the current retention configuration.
    ///
    /// Returns the retention policy that controls time-based and row-count
    /// based cleanup of recorded data.
    fn retention_config(&self) -> &crate::models::RetentionConfig;

    /// Clean up old records based on retention policy.
    ///
    /// Deletes records older than `max_age_days` and reduces row count to `max_rows`.
    /// This method is idempotent — safe to call multiple times.
    ///
    /// Returns the number of rows deleted.
    async fn cleanup(&self) -> Result<u64, EnrichmentError>;

    /// Get current statistics about the recorded runs.
    ///
    /// Returns row count and timestamp bounds (oldest/newest records).
    /// This allows monitoring and introspection without exposing
    /// storage implementation details.
    async fn stats(&self) -> Result<RunRecorderStats, EnrichmentError>;
}
