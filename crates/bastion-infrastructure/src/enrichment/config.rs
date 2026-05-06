//! Enrichment configuration.

use std::path::PathBuf;

/// Configuration for the enrichment adapter.
#[derive(Debug, Clone)]
pub struct EnrichmentConfig {
    /// Whether enrichment is enabled.
    pub enabled: bool,
    /// Directory containing enricher descriptor files.
    pub catalog_dir: PathBuf,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            catalog_dir: PathBuf::from(".bastion/catalog/enrichers"),
        }
    }
}
