//! Intent detector.
//!
//! Pattern-matches a command string against enricher match_patterns
//! to determine which enricher(s) should be activated.

use std::collections::HashMap;
use std::sync::Arc;

use crate::models::EnricherDescriptor;
use regex::Regex;

/// Cache of pre-compiled match patterns for intent detection.
#[derive(Default)]
pub struct IntentCache {
    /// Maps pattern string -> compiled regex (Arc for cheap cloning).
    compiled: HashMap<String, Arc<Regex>>,
}

impl IntentCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            compiled: HashMap::new(),
        }
    }

    /// Compile a match pattern, caching the result.
    fn get_or_compile(&mut self, pattern: &str) -> Option<Arc<Regex>> {
        if let Some(cached) = self.compiled.get(pattern) {
            return Some(Arc::clone(cached));
        }
        let regex = Regex::new(pattern).ok()?;
        let arc = Arc::new(regex);
        self.compiled.insert(pattern.to_string(), Arc::clone(&arc));
        Some(arc)
    }
}

/// Detects which enricher(s) match a given command string.
pub struct IntentDetector {
    cache: IntentCache,
}

impl IntentDetector {
    /// Create a new IntentDetector with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: IntentCache::default(),
        }
    }

    /// Return enricher descriptors that match the given command.
    ///
    /// Matches by checking if any of the enricher's `match_patterns` regexes
    /// match the command string. Uses cached compiled patterns to avoid
    /// recompilation per call.
    pub fn detect<'a>(
        &mut self,
        command: &str,
        enrichers: &'a [EnricherDescriptor],
    ) -> Vec<&'a EnricherDescriptor> {
        enrichers
            .iter()
            .filter(|e| {
                e.enabled
                    && e.match_patterns.iter().any(|p| {
                        self.cache
                            .get_or_compile(p)
                            .map(|re| re.is_match(command))
                            .unwrap_or(false)
                    })
            })
            .collect()
    }
}

impl Default for IntentDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enricher(id: &str, patterns: Vec<&str>) -> EnricherDescriptor {
        EnricherDescriptor {
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0".to_string(),
            match_patterns: patterns.into_iter().map(String::from).collect(),
            template: String::new(),
            enabled: true,
            extractors: Vec::new(),
        }
    }

    #[test]
    fn test_detect_maven_commands() {
        let enrichers = vec![
            enricher(
                "maven",
                vec![r"^mvn\s+(package|install|verify|test|compile|clean|deploy)"],
            ),
            enricher("gradle", vec![r"^gradle\s+\w+"]),
        ];

        let mut detector = IntentDetector::new();
        assert_eq!(detector.detect("mvn package", &enrichers)[0].id, "maven");
        assert_eq!(detector.detect("gradle build", &enrichers)[0].id, "gradle");
        assert!(detector.detect("echo hello", &enrichers).is_empty());
    }

    #[test]
    fn test_disabled_enricher_not_matched() {
        let mut enricher = enricher("maven", vec![r"^mvn\s+"]);
        enricher.enabled = false;
        let enrichers = vec![enricher];
        let mut detector = IntentDetector::new();
        assert!(detector.detect("mvn package", &enrichers).is_empty());
    }
}
