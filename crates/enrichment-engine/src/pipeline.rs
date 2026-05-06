//! Fact pipeline.
//!
//! Orchestrates the enrichment flow: intent detection → extraction →
//! normalization → composition → AgentContext.

use std::sync::Arc;

use crate::composer::AgentContextComposer;
use crate::extractors::{GlobExtractor, RegexExtractor};
use crate::intent::IntentDetector;
use crate::models::{AgentContext, EnrichmentMeta, EnricherDescriptor, Fact, OperationInvocation, OperationResult};
use crate::normalizer::{FactNormalizer, NormalizerConfig};
use crate::traits::{CatalogRepository, Extractor, FileSystem};

/// The main enrichment pipeline.
pub struct FactPipeline {
    catalog: Arc<dyn CatalogRepository>,
}

impl FactPipeline {
    /// Create a new pipeline with the given catalog repository.
    pub fn new(catalog: Arc<dyn CatalogRepository>) -> Self {
        Self { catalog }
    }

    /// Run the full enrichment pipeline.
    ///
    /// 1. Detect intent (find matching enrichers)
    /// 2. Run each enricher's extractors
    /// 3. Normalize facts (dedupe, threshold)
    /// 4. Compose AgentContext
    pub async fn run(
        &self,
        invocation: OperationInvocation,
        result: OperationResult,
        fs: &dyn FileSystem,
    ) -> Result<AgentContext, String> {
        // Step 1: Find matching enrichers
        let all_enrichers = self.catalog.find_enrichers(&invocation.command).await;
        let matched = IntentDetector::detect(&invocation.command, &all_enrichers);

        if matched.is_empty() {
            // SC5: No enricher matched
            return Ok(AgentContext {
                facts: Vec::new(),
                build_status: None,
                artifacts: Vec::new(),
                test_summary: None,
                enrichment_meta: EnrichmentMeta {
                    source: "enrichment-engine".to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    enricher_id: String::new(),
                },
            });
        }

        // Take the first matched enricher (MVP: single enricher per command)
        let enricher = matched[0];

        // Step 2: Run extractors
        let mut all_facts = Vec::new();
        for ext_config in &enricher.extractors {
            let facts = Self::run_extractor(ext_config, &invocation, &result, fs).await;
            all_facts.extend(facts);
        }

        // Step 3: Normalize
        let normalizer = FactNormalizer::new(NormalizerConfig::default());
        let normalized = normalizer.normalize(all_facts);

        // Step 4: Compose AgentContext
        let context = Self::compose_context(&invocation, &result, &normalized, enricher);

        Ok(context)
    }

    async fn run_extractor(
        config: &crate::models::ExtractorConfig,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        match config.extractor_type.as_str() {
            "regex" => {
                let extractor = RegexExtractor::new(&config.id, &config.pattern, &config.fact_key);
                extractor.extract(invocation, result, fs).await
            }
            "glob" => {
                let extractor = GlobExtractor::new(&config.id, &config.pattern, &config.fact_key);
                extractor.extract(invocation, result, fs).await
            }
            _ => Vec::new(),
        }
    }

    fn compose_context(
        _invocation: &OperationInvocation,
        _result: &OperationResult,
        facts: &[Fact],
        enricher: &EnricherDescriptor,
    ) -> AgentContext {
        // Extract build status
        let build_status = AgentContextComposer::get_fact(facts, "build_status")
            .map(String::from);

        // Extract artifacts (facts with key containing "artifact")
        let artifacts: Vec<Fact> = facts
            .iter()
            .filter(|f| f.key.contains("artifact"))
            .cloned()
            .collect();

        // Extract test summary if available
        let test_summary = Self::parse_test_summary(facts);

        AgentContext {
            facts: facts.to_vec(),
            build_status,
            artifacts,
            test_summary,
            enrichment_meta: EnrichmentMeta {
                source: "enrichment-engine".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                enricher_id: enricher.id.clone(),
            },
        }
    }

    fn parse_test_summary(facts: &[Fact]) -> Option<crate::models::TestSummary> {
        let run: Option<u32> = AgentContextComposer::get_fact(facts, "tests_run")?.parse().ok();
        let failed: Option<u32> = AgentContextComposer::get_fact(facts, "tests_failed").and_then(|s: &str| s.parse().ok());
        let errors: Option<u32> = AgentContextComposer::get_fact(facts, "tests_errors").and_then(|s: &str| s.parse().ok());
        let skipped: Option<u32> = AgentContextComposer::get_fact(facts, "tests_skipped").and_then(|s: &str| s.parse().ok());

        Some(crate::models::TestSummary {
            run: run.unwrap_or(0),
            failed: failed.unwrap_or(0),
            errors: errors.unwrap_or(0),
            skipped: skipped.unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ExtractorConfig;
    use crate::traits::CatalogRepository;
    use std::sync::Arc;

    struct FakeCatalog {
        enrichers: Vec<EnricherDescriptor>,
    }

    #[async_trait::async_trait]
    impl CatalogRepository for FakeCatalog {
        async fn find_enrichers(&self, _command: &str) -> Vec<EnricherDescriptor> {
            self.enrichers.clone()
        }
        async fn list_all(&self) -> Vec<EnricherDescriptor> {
            self.enrichers.clone()
        }
    }

    struct FakeFs;

    #[async_trait::async_trait]
    impl FileSystem for FakeFs {
        async fn read_to_string(&self, _path: &str) -> Result<String, crate::traits::EnrichmentError> {
            Ok(String::new())
        }
        async fn glob(&self, _pattern: &str) -> Result<Vec<std::path::PathBuf>, crate::traits::EnrichmentError> {
            Ok(Vec::new())
        }
    }

    fn maven_enricher() -> EnricherDescriptor {
        EnricherDescriptor {
            id: "maven".to_string(),
            name: "Maven".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^mvn\s+(package|install|verify|test|compile|clean|deploy)".to_string()],
            template: "Build {{status}}".to_string(),
            enabled: true,
            extractors: vec![
                ExtractorConfig {
                    id: "build_status".to_string(),
                    extractor_type: "regex".to_string(),
                    pattern: r"(?P<status>BUILD\s+(SUCCESS|FAILURE))".to_string(),
                    fact_key: "build_status".to_string(),
                    priority: 1,
                },
                ExtractorConfig {
                    id: "test_results".to_string(),
                    extractor_type: "regex".to_string(),
                    pattern: r"Tests run: (?P<tests_run>\d+), Failures: (?P<tests_failed>\d+), Errors: (?P<tests_errors>\d+), Skipped: (?P<tests_skipped>\d+)".to_string(),
                    fact_key: "test_results".to_string(),
                    priority: 2,
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_pipeline_composes_agent_context() {
        // SC4: Pipeline composes AgentContext
        let catalog = Arc::new(FakeCatalog { enrichers: vec![maven_enricher()] });
        let pipeline = FactPipeline::new(catalog);
        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS\nTests run: 10, Failures: 0, Errors: 0, Skipped: 1".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let ctx = pipeline.run(invocation, result, &FakeFs).await.unwrap();
        assert_eq!(ctx.build_status.as_deref(), Some("BUILD SUCCESS"));
        assert_eq!(ctx.enrichment_meta.enricher_id, "maven");
    }

    #[tokio::test]
    async fn test_pipeline_no_enricher_matched() {
        // SC5: No enricher matched
        let catalog = Arc::new(FakeCatalog { enrichers: vec![maven_enricher()] });
        let pipeline = FactPipeline::new(catalog);
        let invocation = OperationInvocation::from_command("echo hello");
        let result = OperationResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        let ctx = pipeline.run(invocation, result, &FakeFs).await.unwrap();
        assert!(ctx.facts.is_empty());
        assert!(ctx.enrichment_meta.enricher_id.is_empty());
    }
}
