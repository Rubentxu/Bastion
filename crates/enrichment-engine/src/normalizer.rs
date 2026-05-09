//! Fact normalizer.
//!
//! Deduplicates facts with the same key (max confidence wins) and
//! drops facts below a confidence threshold.

use std::collections::HashMap;

use crate::models::{ExtractorConfig, Fact};

/// Configuration for the fact normalizer.
#[derive(Debug, Clone)]
pub struct NormalizerConfig {
    /// Minimum confidence threshold (0.0–1.0).
    pub threshold: f32,
}

impl Default for NormalizerConfig {
    fn default() -> Self {
        Self { threshold: 0.5 }
    }
}

/// Normalizes a list of facts: deduplicates by key (max confidence wins) when merge_mode="single",
/// preserves all facts when merge_mode="multi", then filters by confidence threshold.
pub struct FactNormalizer {
    config: NormalizerConfig,
}

impl FactNormalizer {
    /// Create a new normalizer with the given config.
    pub fn new(config: NormalizerConfig) -> Self {
        Self { config }
    }

    /// Normalize a list of facts (legacy behavior - all single-value dedup).
    pub fn normalize(&self, facts: Vec<Fact>) -> Vec<Fact> {
        self.normalize_with_config(facts, &HashMap::new())
    }

    /// Normalize a list of facts using extractor config for merge_mode.
    ///
    /// When merge_mode is "single": deduplicate by key (max confidence wins).
    /// When merge_mode is "multi": preserve all facts with same key unless value is identical.
    pub fn normalize_with_config(
        &self,
        facts: Vec<Fact>,
        extractor_config: &HashMap<String, &ExtractorConfig>,
    ) -> Vec<Fact> {
        // Group by key
        let mut by_key: HashMap<String, Vec<Fact>> = HashMap::new();
        for fact in facts {
            by_key.entry(fact.key.clone()).or_default().push(fact);
        }

        let mut result: Vec<Fact> = Vec::new();

        for (_key, facts_with_key) in by_key {
            // Determine merge_mode for this key
            let merge_mode = facts_with_key
                .first()
                .and_then(|f| extractor_config.get(&f.source_extractor))
                .map(|cfg| cfg.merge_mode.as_str())
                .unwrap_or("single");

            if merge_mode == "multi" {
                // Preserve all facts with this key
                result.extend(facts_with_key);
            } else {
                // Single-value mode: keep max confidence
                let best = facts_with_key.into_iter().max_by(|a, b| {
                    a.confidence
                        .partial_cmp(&b.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                if let Some(fact) = best {
                    result.push(fact);
                }
            }
        }

        // Filter by threshold and collect
        result
            .into_iter()
            .filter(|f| f.confidence >= self.config.threshold)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;

    fn fact(key: &str, value: &str, confidence: f32) -> Fact {
        Fact {
            key: key.to_string(),
            value: value.to_string(),
            tags: Vec::new(),
            source_extractor: "test".to_string(),
            confidence,
        }
    }

    #[test]
    fn test_deduplicate_same_key_max_confidence_wins() {
        // SC3: FactNormalizer deduplication
        let normalizer = FactNormalizer::new(NormalizerConfig { threshold: 0.5 });
        let facts = vec![
            fact("status", "SUCCESS", 0.6),
            fact("status", "FAILURE", 0.9),
        ];
        let result = normalizer.normalize(facts);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, "FAILURE");
    }

    #[test]
    fn test_threshold_filter() {
        let normalizer = FactNormalizer::new(NormalizerConfig { threshold: 0.5 });
        let facts = vec![
            fact("status", "SUCCESS", 0.3),
            fact("status", "FAILURE", 0.9),
        ];
        let result = normalizer.normalize(facts);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value, "FAILURE");
    }

    #[test]
    fn test_merge_behavior() {
        let normalizer = FactNormalizer::new(NormalizerConfig { threshold: 0.0 });
        let facts = vec![
            fact("key1", "val1", 0.5),
            fact("key2", "val2", 0.7),
            fact("key1", "val3", 0.6),
        ];
        let result = normalizer.normalize(facts);
        // key1: val3 wins (0.6 > 0.5), key2: val2 stays
        assert_eq!(result.len(), 2);
        let key1 = result.iter().find(|f| f.key == "key1").unwrap();
        assert_eq!(key1.value, "val3");
    }

    #[test]
    fn test_multi_value_preserved_when_merge_mode_multi() {
        // SC: Multi-value artifact facts preserved
        let normalizer = FactNormalizer::new(NormalizerConfig { threshold: 0.0 });

        let jar_extractor = ExtractorConfig {
            id: "jar_glob".to_string(),
            extractor_type: "glob".to_string(),
            pattern: "target/*.jar".to_string(),
            fact_key: "jar".to_string(),
            priority: 1,
            merge_mode: "multi".to_string(),
            command_extractor_policy: None,
            ..Default::default()
        };

        let mut config_map: HashMap<String, &ExtractorConfig> = HashMap::new();
        config_map.insert("jar_glob".to_string(), &jar_extractor);

        let facts = vec![
            fact_with_extractor("jar", "target/a.jar", 1.0, "jar_glob"),
            fact_with_extractor("jar", "target/b.jar", 1.0, "jar_glob"),
        ];

        let result = normalizer.normalize_with_config(facts, &config_map);
        // Both jar facts should be preserved since merge_mode is "multi"
        assert_eq!(result.len(), 2);
        let jars: Vec<_> = result.iter().filter(|f| f.key == "jar").collect();
        assert_eq!(jars.len(), 2);
    }

    fn fact_with_extractor(
        key: &str,
        value: &str,
        confidence: f32,
        source_extractor: &str,
    ) -> Fact {
        Fact {
            key: key.to_string(),
            value: value.to_string(),
            tags: Vec::new(),
            source_extractor: source_extractor.to_string(),
            confidence,
        }
    }
}
