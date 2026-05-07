//! Glob-based fact extractor.
//!
//! Matches a glob pattern against the file system, returning one Fact
//! per matched file with path and size information.

use async_trait::async_trait;

use crate::models::{Fact, OperationInvocation, OperationResult};
use crate::traits::{Extractor, FileSystem};

/// Extractor that glob-matches files and produces a Fact per matched file.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GlobExtractor {
    name: String,
    pattern: String,
    fact_key: String,
    merge_mode: String,
}

impl GlobExtractor {
    /// Create a new glob extractor with default merge_mode.
    pub fn new(name: &str, pattern: &str, fact_key: &str) -> Self {
        Self {
            name: name.to_string(),
            pattern: pattern.to_string(),
            fact_key: fact_key.to_string(),
            merge_mode: "single".to_string(),
        }
    }

    /// Create a new glob extractor with explicit merge_mode.
    pub fn with_merge_mode(name: &str, pattern: &str, fact_key: &str, merge_mode: &str) -> Self {
        Self {
            name: name.to_string(),
            pattern: pattern.to_string(),
            fact_key: fact_key.to_string(),
            merge_mode: merge_mode.to_string(),
        }
    }
}

#[async_trait]
impl Extractor for GlobExtractor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn extract(
        &self,
        _invocation: &OperationInvocation,
        _result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        match fs.glob(&self.pattern).await {
            Ok(paths) => paths
                .iter()
                .map(|p| Fact {
                    key: self.fact_key.clone(),
                    value: p.to_string_lossy().to_string(),
                    tags: Vec::new(),
                    source_extractor: self.name.clone(),
                    confidence: 1.0,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OperationInvocation;
    use crate::traits::{EnrichmentError, FileSystem};
    use glob::Pattern;
    use std::path::PathBuf;

    struct FakeFs {
        files: Vec<PathBuf>,
    }

    impl FakeFs {
        fn new(files: Vec<PathBuf>) -> Self {
            Self { files }
        }
    }

    #[async_trait::async_trait]
    impl FileSystem for FakeFs {
        async fn read_to_string(&self, _path: &str) -> Result<String, EnrichmentError> {
            Ok(String::new())
        }
        async fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>, EnrichmentError> {
            let pat =
                Pattern::new(pattern).map_err(|e| EnrichmentError::FileSystem(e.to_string()))?;
            let matches: Vec<PathBuf> = self
                .files
                .iter()
                .filter(|p| {
                    let p_str = p.to_string_lossy();
                    pat.matches(&p_str)
                })
                .cloned()
                .collect();
            Ok(matches)
        }
    }

    #[tokio::test]
    async fn test_glob_extractor_with_files() {
        // SC2: Glob extraction finds artifacts
        let extractor = GlobExtractor::new("jar_artifacts", "target/*.jar", "jar_artifact");
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };
        let fake_fs = FakeFs::new(vec![
            PathBuf::from("target/app.jar"),
            PathBuf::from("target/lib.jar"),
        ]);
        let facts = extractor.extract(&invocation, &result, &fake_fs).await;
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].key, "jar_artifact");
    }

    #[tokio::test]
    async fn test_glob_extractor_no_files_found() {
        let extractor = GlobExtractor::new("jar_artifacts", "target/*.jar", "jar_artifact");
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };
        let fake_fs = FakeFs::new(Vec::new());
        let facts = extractor.extract(&invocation, &result, &fake_fs).await;
        assert!(facts.is_empty());
    }
}
