//! Enrichment infrastructure module.
//!
//! Provides the Bastion-specific adapter that wires the host-agnostic
//! `enrichment-engine` crate into the Bastion gateway.

pub mod adapter;
pub mod config;
pub mod fs;
pub mod sqlite_optimizer_repo;
pub mod sqlite_repo;
pub mod sqlite_recorder;

pub use adapter::BastionEnrichmentAdapter;
pub use config::{EnrichmentConfig, RetentionConfig};
pub use sqlite_optimizer_repo::SqliteOptimizerRepository;
pub use sqlite_repo::{SqliteCatalogRepository, YamlCatalogImporter};
pub use sqlite_recorder::SqliteRunRecorder;
