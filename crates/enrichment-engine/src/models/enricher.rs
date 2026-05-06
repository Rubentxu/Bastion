//! Enricher descriptor and extractor configuration models.

use serde::{Deserialize, Serialize};

/// Configuration for an individual extractor within an enricher.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtractorConfig {
    /// Unique extractor identifier within the enricher.
    pub id: String,
    /// Extractor type: "regex" or "glob".
    pub extractor_type: String,
    /// The pattern (regex or glob) to use.
    pub pattern: String,
    /// The fact key to emit.
    pub fact_key: String,
    /// Extraction priority (lower = higher priority).
    #[serde(default)]
    pub priority: i32,
    /// Merge mode: "single" (dedupe by key, max confidence wins) or "multi" (preserve all facts).
    #[serde(default = "default_merge_mode")]
    pub merge_mode: String,
}

fn default_merge_mode() -> String {
    "single".to_string()
}

/// Descriptor for an enricher — loaded from the catalog.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnricherDescriptor {
    /// Unique enricher identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Regex patterns that activate this enricher.
    pub match_patterns: Vec<String>,
    /// Output template with `{{key}}` interpolation.
    pub template: String,
    /// Whether this enricher is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Extractors to run when this enricher is activated.
    #[serde(default)]
    pub extractors: Vec<ExtractorConfig>,
}

fn default_enabled() -> bool {
    true
}
