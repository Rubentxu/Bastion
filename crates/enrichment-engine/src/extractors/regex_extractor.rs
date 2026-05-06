//! Regex-based fact extractor.
//!
//! Applies named-capture regular expressions to operation stdout/stderr.
//! Each named capture group produces one Fact.

use async_trait::async_trait;
use regex::Regex;

use crate::models::{Fact, OperationInvocation, OperationResult};
use crate::traits::{Extractor, FileSystem};

/// Extractor that applies a named-capture regex to stdout/stderr.
pub struct RegexExtractor {
    name: String,
    pattern: Regex,
    fact_key: String,
}

impl RegexExtractor {
    /// Create a new regex extractor.
    ///
    /// # Panics
    ///
    /// Panics if `pattern` is not a valid regex or contains no named capture groups.
    pub fn new(name: &str, pattern: &str, fact_key: &str) -> Self {
        let regex = Regex::new(pattern).expect("Invalid regex pattern");
        Self {
            name: name.to_string(),
            pattern: regex,
            fact_key: fact_key.to_string(),
        }
    }

    /// Returns true if the pattern has any named capture groups.
    pub fn has_named_captures(&self) -> bool {
        !self.pattern.capture_names().flatten().collect::<Vec<_>>().is_empty()
    }
}

#[async_trait]
impl Extractor for RegexExtractor {
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

        // Always use fact_key as the fact key and the full match as the value.
        // Named capture groups are used only to determine if there's a match;
        // the full match (group 0) provides the value.
        if let Some(m) = self.pattern.find(text) {
            facts.push(Fact {
                key: self.fact_key.clone(),
                value: m.as_str().to_string(),
                tags: Vec::new(),
                source_extractor: self.name.clone(),
                confidence: 1.0,
            });
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
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status");
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
        assert_eq!(facts[0].key, "build_status");
        assert_eq!(facts[0].value, "BUILD SUCCESS");
        assert_eq!(facts[0].confidence, 1.0);
    }

    #[tokio::test]
    async fn test_regex_extractor_empty_stdout() {
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status");
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
        let extractor = RegexExtractor::new("build_status", r"(?P<status>BUILD\s+(SUCCESS|FAILURE))", "build_status");
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
}
