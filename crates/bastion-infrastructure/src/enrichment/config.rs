//! Enrichment configuration.

use std::path::PathBuf;

/// Configuration for the enrichment adapter.
#[derive(Debug, Clone)]
pub struct EnrichmentConfig {
    /// Whether enrichment is enabled.
    pub enabled: bool,
    /// Directory containing enricher descriptor files.
    pub catalog_dir: PathBuf,
    /// Retention policy for enrichment run records.
    pub retention: RetentionConfig,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            catalog_dir: PathBuf::from(".bastion/catalog/enrichers"),
            retention: RetentionConfig::default(),
        }
    }
}

/// Retention policy configuration for enrichment run records.
///
/// Controls time-based and row-count-based cleanup of the `enrichment_runs.db`.
#[derive(Debug, Clone)]
pub struct RetentionConfig {
    /// Maximum age of records in days. Records older than this are deleted.
    pub max_age_days: u32,
    /// Maximum number of rows to retain. Oldest rows are deleted when exceeded.
    pub max_rows: u64,
    /// Whether cleanup is enabled. When false, cleanup() returns immediately.
    pub enabled: bool,
    /// Whether sanitization is enabled. When true, commands are sanitized before persistence.
    pub sanitize: bool,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            max_age_days: 90,
            max_rows: 100_000,
            enabled: true,
            sanitize: true,
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
