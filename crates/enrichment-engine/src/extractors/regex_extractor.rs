//! Regex-based fact extractor.
//!
//! Applies named-capture regular expressions to operation stdout/stderr.
//! Each named capture group produces one Fact.

use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;

use crate::models::{Fact, OperationInvocation, OperationResult, ValidatedPattern};
use crate::traits::{Extractor as ExtractorTrait, FileSystem, EnrichmentError};

/// Extractor that applies a named-capture regex to stdout/stderr.
#[derive(Debug)]
pub struct RegexExtractor {
    name: String,
    pattern: Arc<Regex>,
    fact_key: String,
    merge_mode: String,
}

impl RegexExtractor {
    /// Create a new regex extractor from a pre-compiled ValidatedPattern.
    pub fn from_validated(pattern: &ValidatedPattern) -> Self {
        Self {
            name: pattern.extractor_id.clone(),
            pattern: Arc::clone(&pattern.regex),
            fact_key: pattern.fact_key.clone(),
            merge_mode: pattern.merge_mode.clone(),
        }
    }

    /// Create a new regex extractor (fallible).
    ///
    /// Returns Err if the pattern is not a valid regex.
    pub fn new(name: &str, pattern: &str, fact_key: &str) -> Result<Self, EnrichmentError> {
        let regex = Regex::new(pattern).map_err(|e| EnrichmentError::Config(format!("Invalid regex: {}", e)))?;
        Ok(Self {
            name: name.to_string(),
            pattern: Arc::new(regex),
            fact_key: fact_key.to_string(),
            merge_mode: "single".to_string(),
        })
    }

    /// Create a new regex extractor with merge mode (fallible).
    pub fn with_merge_mode(name: &str, pattern: &str, fact_key: &str, merge_mode: &str) -> Result<Self, EnrichmentError> {
        let regex = Regex::new(pattern).map_err(|e| EnrichmentError::Config(format!("Invalid regex: {}", e)))?;
        Ok(Self {
            name: name.to_string(),
            pattern: Arc::new(regex),
            fact_key: fact_key.to_string(),
            merge_mode: merge_mode.to_string(),
        })
    }

    /// Returns true if the pattern has any named capture groups.
    pub fn has_named_captures(&self) -> bool {
        !self.pattern.capture_names().flatten().collect::<Vec<_>>().is_empty()
    }

    /// Returns the merge mode for this extractor.
    pub fn merge_mode(&self) -> &str {
        &self.merge_mode
    }
}

#[async_trait]
impl ExtractorTrait for RegexExtractor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn extract(
        &self,
        _invocation: &OperationInvocation,
        result: &OperationResult,
        _fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        let mut facts = Vec::new();

        // Try stdout first, then stderr
        let text = if result.stdout.is_empty() {
            &result.stderr
        } else {
            &result.stdout
        };

        // Use captures() instead of find() to get access to named capture groups
        let caps = match self.pattern.captures(text) {
            Some(caps) => caps,
            None => return facts,
        };

        // Get named capture groups (skip group 0 which is the full match)
        let capture_names: Vec<_> = self.pattern.capture_names().flatten().collect();

        // If there are multiple named captures, emit one fact per capture group
        // This enables parse_test_summary to work with Maven Surefire-like output
        // where we need individual facts like tests_run, tests_failed, etc.
        if capture_names.len() > 1 {
            for name in &capture_names {
                if let Some(cap) = caps.name(name) {
                    facts.push(Fact {
                        key: name.to_string(),
                        value: cap.as_str().to_string(),
                        tags: Vec::new(),
                        source_extractor: self.name.clone(),
                        confidence: 1.0,
                    });
                }
            }
        } else {
            // Single capture or no captures: emit one fact with fact_key (backward compatible)
            // Use get(0) to get the full match as a Match object
            if let Some(m) = caps.get(0) {
                facts.push(Fact {
                    key: self.fact_key.clone(),
                    value: m.as_str().to_string(),
                    tags: Vec::new(),
                    source_extractor: self.name.clone(),
                    confidence: 1.0,
                });
            }
        }

        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OperationResult;
    use crate::traits::EnrichmentError;

    struct FakeFs;

    #[async_trait::async_trait]
    impl FileSystem for FakeFs {
        async fn read_to_string(&self, _path: &str) -> Result<String, EnrichmentError> {
            Ok(String::new())
        }
        async fn glob(&self, _pattern: &str) -> Result<Vec<std::path::PathBuf>, EnrichmentError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn test_regex_extractor_happy_path() {
        // SC1: Regex extraction happy path
        // Single capture: uses fact_key for backward compatibility
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status").unwrap();
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "build_status"); // fact_key used for single capture
        assert_eq!(facts[0].value, "BUILD SUCCESS");
        assert_eq!(facts[0].confidence, 1.0);
    }

    #[tokio::test]
    async fn test_regex_extractor_empty_stdout() {
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status").unwrap();
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: "Some error".to_string(),
            duration_ms: 1000,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_regex_extractor_no_match() {
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status").unwrap();
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "Everything went well".to_string(),
            stderr: String::new(),
            duration_ms: 1000,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_pattern_returns_err() {
        // SC1: Invalid regex returns Err, does not panic
        let result = RegexExtractor::new("build_status", "[invalid", "build_status");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EnrichmentError::Config(_)));
        assert!(err.to_string().contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_regex_extractor_named_captures_for_maven_test_summary() {
        // W1 Fix: Maven test summary regex should emit individual facts per named capture
        // This enables parse_test_summary to work correctly
        let extractor = RegexExtractor::new(
            "test_results",
            r"Tests run: (?P<tests_run>\d+), Failures: (?P<tests_failed>\d+), Errors: (?P<tests_errors>\d+), Skipped: (?P<tests_skipped>\d+)",
            "test_results"
        ).unwrap();

        let invocation = OperationInvocation::from_command("mvn test");
        let result = OperationResult {
            exit_code: 0,
            stdout: "Tests run: 12, Failures: 1, Errors: 0, Skipped: 2".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;

        // Should emit 4 facts, one per named capture
        assert_eq!(facts.len(), 4);

        // Find each fact by key
        let fact_map: std::collections::HashMap<&str, &Fact> =
            facts.iter().map(|f| (f.key.as_str(), f)).collect();

        assert_eq!(fact_map.get("tests_run").unwrap().value, "12");
        assert_eq!(fact_map.get("tests_failed").unwrap().value, "1");
        assert_eq!(fact_map.get("tests_errors").unwrap().value, "0");
        assert_eq!(fact_map.get("tests_skipped").unwrap().value, "2");
    }

    #[tokio::test]
    async fn test_regex_extractor_no_named_captures_uses_fact_key() {
        // Backward compatibility: regex without named captures uses fact_key
        let extractor = RegexExtractor::new("status", r"BUILD\s+(SUCCESS|FAILURE)", "build_status").unwrap();
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "build_status"); // Uses fact_key when no named captures
        assert_eq!(facts[0].value, "BUILD SUCCESS");
    }
}
