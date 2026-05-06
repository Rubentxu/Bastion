//! Fact normalizer.
//!
//! Deduplicates facts with the same key (max confidence wins) and
//! drops facts below a confidence threshold.

use crate::models::Fact;

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

/// Normalizes a list of facts: deduplicates by key (max confidence wins),
/// then filters by confidence threshold.
pub struct FactNormalizer {
    config: NormalizerConfig,
}

impl FactNormalizer {
    /// Create a new normalizer with the given config.
    pub fn new(config: NormalizerConfig) -> Self {
        Self { config }
    }

    /// Normalize a list of facts.
    pub fn normalize(&self, facts: Vec<Fact>) -> Vec<Fact> {
        // Group by key, keeping max confidence
        let mut best: std::collections::HashMap<String, Fact> = std::collections::HashMap::new();

        for fact in facts {
            let key = fact.key.clone();
            let confidence = fact.confidence;

            match best.get(&key) {
                Some(existing) if existing.confidence >= confidence => {
                    // Keep existing (higher or equal confidence)
                }
                _ => {
                    best.insert(key, fact);
                }
            }
        }

        // Filter by threshold and collect
        best.into_values()
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
}
