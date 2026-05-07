//! Enrichment configuration.

use std::path::PathBuf;

// Re-export RetentionConfig from enrichment-engine for use in infrastructure
pub use enrichment_engine::models::RetentionConfig;

/// Configuration for the record persistence semaphore.
#[derive(Debug, Clone, Copy)]
pub struct SemaphoreConfig {
    /// Maximum number of concurrent record persistence operations.
    pub max_concurrent_records: usize,
}

impl Default for SemaphoreConfig {
    fn default() -> Self {
        Self {
            max_concurrent_records: 64,
        }
    }
}

/// Configuration for the enrichment adapter.
#[derive(Debug, Clone)]
pub struct EnrichmentConfig {
    /// Whether enrichment is enabled.
    pub enabled: bool,
    /// Directory containing enricher descriptor files.
    pub catalog_dir: PathBuf,
    /// Retention policy for enrichment run records.
    pub retention: RetentionConfig,
    /// Semaphore configuration for record persistence backpressure.
    pub semaphore: SemaphoreConfig,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            catalog_dir: PathBuf::from(".bastion/catalog/enrichers"),
            retention: RetentionConfig::default(),
            semaphore: SemaphoreConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_config_defaults() {
        let config = RetentionConfig::default();
        assert_eq!(config.max_age_days, 90);
        assert_eq!(config.max_rows, 100_000);
        assert!(config.enabled);
        assert!(config.sanitize);
    }

    #[test]
    fn enrichment_config_defaults() {
        let config = EnrichmentConfig::default();
        assert!(config.enabled);
        assert!(config.retention.enabled);
        assert!(config.retention.sanitize);
    }
}
