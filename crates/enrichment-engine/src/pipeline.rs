//! Fact pipeline.
//!
//! Orchestrates the enrichment flow: intent detection → extraction →
//! normalization → composition → AgentContext.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use crate::composer::AgentContextComposer;
use crate::extractors::{CommandExtractor, GlobExtractor, RegexExtractor};
use crate::intent::IntentDetector;
use crate::models::{AgentContext, EnrichmentMeta, EnricherDescriptor, Fact, OperationInvocation, OperationResult, ValidatedPattern};
use crate::normalizer::{FactNormalizer, NormalizerConfig};
use crate::rules::RuleEvaluator;
use crate::traits::{CatalogRepository, Extractor, FileSystem, EnrichmentError};

/// Cache of pre-compiled patterns per enricher.
/// Built once at pipeline init and reused across all requests.
#[derive(Default)]
pub struct CompiledEnricherCache {
    /// Maps enricher_id -> list of validated patterns for its extractors.
    patterns: HashMap<String, Vec<ValidatedPattern>>,
}

impl CompiledEnricherCache {
    /// Build the cache from a list of enrichers.
    /// Logs and skips any extractors with invalid regex patterns.
    pub fn from_enrichers(enrichers: &[EnricherDescriptor]) -> Self {
        let mut cache = Self::default();
        // Pre-compile a dummy regex for glob extractors (not actually used for matching)
        let dummy_regex = Arc::new(regex::Regex::new(".").unwrap());
        for enricher in enrichers {
            let mut validated = Vec::new();
            for ext in &enricher.extractors {
                if ext.extractor_type == "regex" {
                    match ValidatedPattern::new(&ext.id, &ext.pattern, &ext.fact_key, &ext.merge_mode) {
                        Ok(vp) => validated.push(vp),
                        Err(e) => {
                            warn!(enricher_id = %enricher.id, extractor_id = %ext.id, pattern = %ext.pattern, error = %e, "Skipping extractor with invalid regex pattern");
                        }
                    }
                } else {
                    // For glob extractors, create a validated pattern entry (no regex compilation needed)
                    validated.push(ValidatedPattern {
                        regex: Arc::clone(&dummy_regex),
                        pattern_str: ext.pattern.clone(),
                        fact_key: ext.fact_key.clone(),
                        extractor_id: ext.id.clone(),
                        merge_mode: ext.merge_mode.clone(),
                    });
                }
            }
            cache.patterns.insert(enricher.id.clone(), validated);
        }
        cache
    }

    /// Get validated patterns for an enricher.
    pub fn get(&self, enricher_id: &str) -> Option<&Vec<ValidatedPattern>> {
        self.patterns.get(enricher_id)
    }
}

/// The main enrichment pipeline.
 pub struct FactPipeline {
     catalog: Arc<dyn CatalogRepository>,
     rule_evaluator: Option<Arc<dyn RuleEvaluator>>,
     #[allow(dead_code)]
     compiled_cache: Arc<CompiledEnricherCache>,
 }

impl FactPipeline {
    /// Create a new pipeline with the given catalog repository (no rule evaluator).
    pub fn new(catalog: Arc<dyn CatalogRepository>) -> Self {
        Self {
            catalog,
            rule_evaluator: None,
            compiled_cache: Arc::new(CompiledEnricherCache::default()),
        }
    }

    /// Create a new pipeline with catalog and optional rule evaluator.
    pub fn with_rule_evaluator(
        catalog: Arc<dyn CatalogRepository>,
        rule_evaluator: Option<Arc<dyn RuleEvaluator>>,
    ) -> Self {
        Self {
            catalog,
            rule_evaluator,
            compiled_cache: Arc::new(CompiledEnricherCache::default()),
        }
    }

    /// Create a new pipeline with a pre-built cache (for shared pipeline scenario).
    pub fn with_cache(catalog: Arc<dyn CatalogRepository>, compiled_cache: Arc<CompiledEnricherCache>) -> Self {
        Self {
            catalog,
            rule_evaluator: None,
            compiled_cache,
        }
    }

    /// Create a new pipeline with a pre-built cache and optional rule evaluator.
    pub fn with_cache_and_rules(
        catalog: Arc<dyn CatalogRepository>,
        compiled_cache: Arc<CompiledEnricherCache>,
        rule_evaluator: Option<Arc<dyn RuleEvaluator>>,
    ) -> Self {
        Self {
            catalog,
            rule_evaluator,
            compiled_cache,
        }
    }

    /// Build the compiled cache from all enrichers in the catalog.
    pub async fn build_cache(&self) -> Arc<CompiledEnricherCache> {
        let enrichers = self.catalog.list_all().await;
        Arc::new(CompiledEnricherCache::from_enrichers(&enrichers))
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
        let mut detector = IntentDetector::new();
        let matched = detector.detect(&invocation.command, &all_enrichers);

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
                verdict: None,
                recommendations: None,
            });
        }

        // Take the first matched enricher (MVP: single enricher per command)
        let enricher = matched[0];

        // Step 2: Run extractors (skipping any with invalid patterns)
        let mut all_facts = Vec::new();
        for ext_config in &enricher.extractors {
            match Self::run_extractor(ext_config, &invocation, &result, fs).await {
                Ok(facts) => all_facts.extend(facts),
                Err(e) => {
                    warn!(enricher_id = %enricher.id, extractor_id = %ext_config.id, error = %e, "Skipping failed extractor");
                }
            }
        }

        // Step 2.5: Rule evaluation (if rule evaluator is configured)
        let rule_output = if let Some(ref evaluator) = self.rule_evaluator {
            let output = evaluator
                .evaluate(&enricher.id, &invocation, &result, &all_facts)
                .await;
            // Merge rule-derived facts with extracted facts before normalization
            all_facts.extend(output.derived_facts.clone());
            output
        } else {
            crate::models::RuleOutput::empty()
        };

        // Step 3: Normalize - build extractor config map for merge_mode
        let extractor_config_map: HashMap<String, &crate::models::ExtractorConfig> = enricher
            .extractors
            .iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let normalizer = FactNormalizer::new(NormalizerConfig::default());
        let normalized = normalizer.normalize_with_config(all_facts, &extractor_config_map);

        // Step 4: Compose AgentContext
        let context = Self::compose_context(
            &invocation,
            &result,
            &normalized,
            enricher,
            rule_output.verdict,
            if rule_output.recommendations.is_empty() {
                None
            } else {
                Some(rule_output.recommendations)
            },
        );

        Ok(context)
    }

    async fn run_extractor(
        config: &crate::models::ExtractorConfig,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Result<Vec<Fact>, EnrichmentError> {
        match config.extractor_type.as_str() {
            "regex" => {
                let extractor = RegexExtractor::with_merge_mode(&config.id, &config.pattern, &config.fact_key, &config.merge_mode)?;
                Ok(extractor.extract(invocation, result, fs).await)
            }
            "glob" => {
                let extractor = GlobExtractor::with_merge_mode(&config.id, &config.pattern, &config.fact_key, &config.merge_mode);
                Ok(extractor.extract(invocation, result, fs).await)
            }
            "command" => {
                let policy = config.command_extractor_policy.clone().unwrap_or_default();
                let extractor = CommandExtractor::with_policy(&config.id, policy);
                Ok(extractor.extract(invocation, result, fs).await)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn compose_context(
        _invocation: &OperationInvocation,
        _result: &OperationResult,
        facts: &[Fact],
        enricher: &EnricherDescriptor,
        verdict: Option<String>,
        recommendations: Option<Vec<String>>,
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
            verdict,
            recommendations,
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
                    merge_mode: "single".to_string(),
                    command_extractor_policy: None,
                },
                ExtractorConfig {
                    id: "test_results".to_string(),
                    extractor_type: "regex".to_string(),
                    pattern: r"Tests run: (?P<tests_run>\d+), Failures: (?P<tests_failed>\d+), Errors: (?P<tests_errors>\d+), Skipped: (?P<tests_skipped>\d+)".to_string(),
                    fact_key: "test_results".to_string(),
                    priority: 2,
                    merge_mode: "single".to_string(),
                    command_extractor_policy: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_pipeline_composes_agent_context() {
        // SC4: Pipeline composes AgentContext
        // W1 Fix: verify that named captures from test_results regex are properly parsed
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
        // Note: build_status comes from fact with key "status" (named capture from regex)
        assert_eq!(ctx.build_status.as_deref(), Some("BUILD SUCCESS"));
        assert_eq!(ctx.enrichment_meta.enricher_id, "maven");

        // W1 Fix: verify parse_test_summary correctly extracts named captures
        // The test_results regex has named captures: tests_run, tests_failed, tests_errors, tests_skipped
        assert!(ctx.test_summary.is_some(), "test_summary should be parsed from named captures");
        let ts = ctx.test_summary.unwrap();
        assert_eq!(ts.run, 10, "tests_run should be 10");
        assert_eq!(ts.failed, 0, "tests_failed should be 0");
        assert_eq!(ts.errors, 0, "tests_errors should be 0");
        assert_eq!(ts.skipped, 1, "tests_skipped should be 1");
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
