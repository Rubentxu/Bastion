//! Static-value fact extractor.
//!
//! Emits a fixed set of facts from configuration — no pattern matching, no file I/O.
//! Used for enricher metadata (e.g., tool name, version, build system).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::models::{Fact, OperationInvocation, OperationResult};
use crate::traits::{Extractor, FileSystem};

/// A single static fact definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaticFact {
    /// The fact key (e.g., "build_tool").
    pub key: String,
    /// The fact value (e.g., "maven").
    pub value: String,
    /// Confidence score (0.0–1.0).
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    1.0
}

/// Extractor that emits static facts from configuration.
///
/// Unlike regex/glob/command extractors, this extractor does not inspect
/// the invocation or result — it always emits the same facts. Useful for
/// annotating enrichment with build system identity, default tool versions, etc.
#[derive(Debug, Clone)]
pub struct StaticExtractor {
    name: String,
    facts: Vec<StaticFact>,
}

impl StaticExtractor {
    /// Create a new static extractor from a list of static facts.
    pub fn new(name: &str, facts: Vec<StaticFact>) -> Self {
        Self {
            name: name.to_string(),
            facts,
        }
    }
}

#[async_trait]
impl Extractor for StaticExtractor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn extract(
        &self,
        _invocation: &OperationInvocation,
        _result: &OperationResult,
        _fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        self.facts
            .iter()
            .map(|sf| Fact {
                key: sf.key.clone(),
                value: sf.value.clone(),
                tags: Vec::new(),
                source_extractor: self.name.clone(),
                confidence: sf.confidence,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OperationInvocation;
    use crate::traits::EnrichmentError;

    struct FakeFs;

    #[async_trait::async_trait]
    impl FileSystem for FakeFs {
        async fn read_to_string(&self, _path: &str) -> Result<String, EnrichmentError> {
            Ok(String::new())
        }
        async fn glob(
            &self,
            _pattern: &str,
        ) -> Result<Vec<std::path::PathBuf>, EnrichmentError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn test_static_extractor_emits_all_facts() {
        let facts = vec![
            StaticFact {
                key: "build_tool".to_string(),
                value: "maven".to_string(),
                confidence: 1.0,
            },
            StaticFact {
                key: "build_tool_version".to_string(),
                value: "3.9.0".to_string(),
                confidence: 0.8,
            },
        ];
        let extractor = StaticExtractor::new("build_info", facts);

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].key, "build_tool");
        assert_eq!(facts[0].value, "maven");
        assert_eq!(facts[0].confidence, 1.0);
        assert_eq!(facts[1].key, "build_tool_version");
        assert_eq!(facts[1].value, "3.9.0");
        assert_eq!(facts[1].confidence, 0.8);
    }

    #[tokio::test]
    async fn test_static_extractor_empty_facts() {
        let extractor = StaticExtractor::new("empty", vec![]);
        let invocation = OperationInvocation::from_command("any");
        let result = OperationResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        let facts = extractor.extract(&invocation, &result, &FakeFs).await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_static_extractor_name() {
        let extractor = StaticExtractor::new("my_static", vec![]);
        assert_eq!(extractor.name(), "my_static");
    }
}
