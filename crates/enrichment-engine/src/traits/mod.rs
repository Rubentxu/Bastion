//! Core traits for the enrichment engine.
//!
//! These traits define the ports through which the host-agnostic core
//! interacts with external systems (catalog storage, file system, fact store).

use async_trait::async_trait;
use std::path::PathBuf;

use crate::models::{EnricherDescriptor, OperationInvocation, OperationResult, Fact};

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
