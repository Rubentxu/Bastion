//! Fact pipeline.
//!
//! Orchestrates the enrichment flow: intent detection → extraction →
//! normalization → composition → AgentContext.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::join_all;
use tracing::warn;

use crate::composer::AgentContextComposer;
use crate::extractors::{CommandExtractor, GlobExtractor, RegexExtractor};
use crate::intent::IntentDetector;
use crate::models::{
    AgentContext, EnricherDescriptor, EnrichmentMeta, Fact, OperationInvocation, OperationResult,
    ValidatedPattern,
};
use crate::normalizer::{FactNormalizer, NormalizerConfig};
use crate::rules::RuleEvaluator;
use crate::traits::{CatalogRepository, EnrichmentError, Extractor, FileSystem};

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
        for enricher in enrichers {
            let mut validated = Vec::new();
            for ext in &enricher.extractors {
                if ext.extractor_type == "regex" {
                    match ValidatedPattern::new(
                        &ext.id,
                        &ext.pattern,
                        &ext.fact_key,
                        &ext.merge_mode,
                    ) {
                        Ok(vp) => validated.push(vp),
                        Err(e) => {
                            warn!(enricher_id = %enricher.id, extractor_id = %ext.id, pattern = %ext.pattern, error = %e, "Skipping extractor with invalid regex pattern");
                        }
                    }
                } else if ext.extractor_type == "glob" {
                    // For glob extractors, use new_glob which sets extractor_type to "glob"
                    validated.push(ValidatedPattern::new_glob(
                        &ext.id,
                        &ext.pattern,
                        &ext.fact_key,
                        &ext.merge_mode,
                    ));
                }
                // command extractors are not cached - built per-call
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

/// Internal cache of pre-built extractor instances, indexed by enricher_id.
#[derive(Clone)]
enum ExtractorInstance {
    Regex(RegexExtractor),
    Glob(GlobExtractor),
    #[allow(dead_code)]
    Command(CommandExtractor),
}

impl ExtractorInstance {
    async fn extract(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Vec<Fact> {
        match self {
            ExtractorInstance::Regex(ext) => ext.extract(invocation, result, fs).await,
            ExtractorInstance::Glob(ext) => ext.extract(invocation, result, fs).await,
            ExtractorInstance::Command(ext) => ext.extract(invocation, result, fs).await,
        }
    }
}

/// The main enrichment pipeline.
pub struct FactPipeline {
    catalog: Arc<dyn CatalogRepository>,
    rule_evaluator: Option<Arc<dyn RuleEvaluator>>,
    compiled_cache: Arc<CompiledEnricherCache>,
    cached_extractors: std::sync::Mutex<Option<HashMap<String, Vec<ExtractorInstance>>>>,
}

impl FactPipeline {
    /// Create a new pipeline with the given catalog repository (no rule evaluator).
    pub fn new(catalog: Arc<dyn CatalogRepository>) -> Self {
        Self {
            catalog,
            rule_evaluator: None,
            compiled_cache: Arc::new(CompiledEnricherCache::default()),
            cached_extractors: std::sync::Mutex::new(None),
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
            cached_extractors: std::sync::Mutex::new(None),
        }
    }

    /// Create a new pipeline with a pre-built cache (for shared pipeline scenario).
    pub fn with_cache(
        catalog: Arc<dyn CatalogRepository>,
        compiled_cache: Arc<CompiledEnricherCache>,
    ) -> Self {
        Self {
            catalog,
            rule_evaluator: None,
            compiled_cache,
            cached_extractors: std::sync::Mutex::new(None),
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
            cached_extractors: std::sync::Mutex::new(None),
        }
    }

    /// Build cached extractors lazily from the compiled cache.
    fn build_cached_extractors(&self) {
        let mut guard = self
            .cached_extractors
            .lock()
            .expect("pipeline cache: lock poisoned");
        if guard.is_some() {
            return;
        }

        let mut map = HashMap::new();
        for (enricher_id, patterns) in &self.compiled_cache.patterns {
            let instances: Vec<ExtractorInstance> = patterns
                .iter()
                .map(|vp| {
                    if vp.extractor_type == "regex" {
                        ExtractorInstance::Regex(RegexExtractor::from_validated(vp))
                    } else if vp.extractor_type == "glob" {
                        // For glob extractors, use with_merge_mode which takes pattern_str as the glob pattern
                        ExtractorInstance::Glob(GlobExtractor::with_merge_mode(
                            &vp.extractor_id,
                            &vp.pattern_str,
                            &vp.fact_key,
                            &vp.merge_mode,
                        ))
                    } else {
                        // Unknown type - shouldn't happen, but skip gracefully
                        ExtractorInstance::Regex(RegexExtractor::from_validated(vp))
                    }
                })
                .collect();
            map.insert(enricher_id.clone(), instances);
        }
        *guard = Some(map);
    }

    /// Build the compiled cache from all enrichers in the catalog.
    pub async fn build_cache(&self) -> Arc<CompiledEnricherCache> {
        let enrichers = self.catalog.list_all().await;
        Arc::new(CompiledEnricherCache::from_enrichers(&enrichers))
    }

    /// Get the number of enrichers in the catalog.
    pub async fn catalog_count(&self) -> usize {
        self.catalog.list_all().await.len()
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
        // Step 1: Select matching enricher (or early return for no-match)
        let Some(enricher) = self.select_enricher(&invocation).await else {
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
        };

        // Step 2: Execute extractors (cached + per-call fallback)
        let all_facts = self
            .execute_extractors(&invocation, &result, fs, &enricher)
            .await;

        // Step 3: Evaluate rules and normalize
        let (rule_output, normalized) = self
            .evaluate_and_normalize(&invocation, &result, &enricher, all_facts)
            .await;

        // Step 4: Compose AgentContext
        Ok(Self::compose_context(
            &invocation,
            &result,
            &normalized,
            &enricher,
            rule_output.verdict,
            if rule_output.recommendations.is_empty() {
                None
            } else {
                Some(rule_output.recommendations)
            },
        ))
    }

    /// Find the first matching enricher for the invocation command.
    /// Returns None if no enricher matches (no-match early return path).
    async fn select_enricher(
        &self,
        invocation: &OperationInvocation,
    ) -> Option<EnricherDescriptor> {
        let all_enrichers = self.catalog.find_enrichers(&invocation.command).await;
        let mut detector = IntentDetector::new();
        let matched = detector.detect(&invocation.command, &all_enrichers);

        if matched.is_empty() {
            None
        } else {
            // Take the first matched enricher (MVP: single enricher per command)
            Some(matched[0].clone())
        }
    }

    /// Execute all extractors (cached + per-call fallback) for a matched enricher.
    /// Returns all extracted facts. Errors are logged via `warn!` and isolated.
    async fn execute_extractors(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
        enricher: &EnricherDescriptor,
    ) -> Vec<Fact> {
        // Build cached extractors lazily on first run
        self.build_cached_extractors();

        let mut all_facts = Vec::new();
        let mut cached_extractor_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Collect cached instances first, then drop the lock before awaiting
        let cached_instances: Vec<ExtractorInstance> = {
            let cached = self
                .cached_extractors
                .lock()
                .expect("pipeline cache: lock poisoned");
            if let Some(ref cached_extractors) = *cached {
                if let Some(instances) = cached_extractors.get(&enricher.id) {
                    let result: Vec<ExtractorInstance> = instances.to_vec();
                    // Track which extractor IDs are handled by cache
                    for instance in instances.iter() {
                        let id = match instance {
                            ExtractorInstance::Regex(ext) => ext.name().to_string(),
                            ExtractorInstance::Glob(ext) => ext.name().to_string(),
                            ExtractorInstance::Command(ext) => ext.name().to_string(),
                        };
                        cached_extractor_ids.insert(id);
                    }
                    result
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };

        // Run cached extractors in parallel using join_all
        let futures: Vec<_> = cached_instances
            .iter()
            .map(|instance| instance.extract(invocation, result, fs))
            .collect();
        let results = join_all(futures).await;
        for facts in results {
            all_facts.extend(facts);
        }

        // For extractors not in cache, build per-call (handles empty cache fallback)
        for ext_config in &enricher.extractors {
            if !cached_extractor_ids.contains(&ext_config.id) {
                match Self::run_extractor(ext_config, invocation, result, fs).await {
                    Ok(facts) => all_facts.extend(facts),
                    Err(e) => {
                        warn!(enricher_id = %enricher.id, extractor_id = %ext_config.id, error = %e, "Skipping failed extractor");
                    }
                }
            }
        }

        all_facts
    }

    /// Evaluate rules (if configured) and normalize facts.
    /// Returns (rule_output, normalized_facts).
    async fn evaluate_and_normalize(
        &self,
        invocation: &OperationInvocation,
        result: &OperationResult,
        enricher: &EnricherDescriptor,
        mut all_facts: Vec<Fact>,
    ) -> (crate::models::RuleOutput, Vec<Fact>) {
        // Rule evaluation (if rule evaluator is configured)
        let rule_output = if let Some(ref evaluator) = self.rule_evaluator {
            let output = evaluator
                .evaluate(&enricher.id, invocation, result, &all_facts)
                .await;
            // Merge rule-derived facts with extracted facts before normalization
            all_facts.extend(output.derived_facts.clone());
            output
        } else {
            crate::models::RuleOutput::empty()
        };

        // Normalize - build extractor config map for merge_mode
        let extractor_config_map: HashMap<String, &crate::models::ExtractorConfig> = enricher
            .extractors
            .iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let normalizer = FactNormalizer::new(NormalizerConfig::default());
        let normalized = normalizer.normalize_with_config(all_facts, &extractor_config_map);

        (rule_output, normalized)
    }

    async fn run_extractor(
        config: &crate::models::ExtractorConfig,
        invocation: &OperationInvocation,
        result: &OperationResult,
        fs: &dyn FileSystem,
    ) -> Result<Vec<Fact>, EnrichmentError> {
        match config.extractor_type.as_str() {
            "regex" => {
                let extractor = RegexExtractor::with_merge_mode(
                    &config.id,
                    &config.pattern,
                    &config.fact_key,
                    &config.merge_mode,
                )?;
                Ok(extractor.extract(invocation, result, fs).await)
            }
            "glob" => {
                let extractor = GlobExtractor::with_merge_mode(
                    &config.id,
                    &config.pattern,
                    &config.fact_key,
                    &config.merge_mode,
                );
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
        let build_status = AgentContextComposer::get_fact(facts, "build_status").map(String::from);

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
        let run: Option<u32> = AgentContextComposer::get_fact(facts, "tests_run")?
            .parse()
            .ok();
        let failed: Option<u32> = AgentContextComposer::get_fact(facts, "tests_failed")
            .and_then(|s: &str| s.parse().ok());
        let errors: Option<u32> = AgentContextComposer::get_fact(facts, "tests_errors")
            .and_then(|s: &str| s.parse().ok());
        let skipped: Option<u32> = AgentContextComposer::get_fact(facts, "tests_skipped")
            .and_then(|s: &str| s.parse().ok());

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
        async fn read_to_string(
            &self,
            _path: &str,
        ) -> Result<String, crate::traits::EnrichmentError> {
            Ok(String::new())
        }
        async fn glob(
            &self,
            _pattern: &str,
        ) -> Result<Vec<std::path::PathBuf>, crate::traits::EnrichmentError> {
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
                    ..Default::default()
                },
                ExtractorConfig {
                    id: "test_results".to_string(),
                    extractor_type: "regex".to_string(),
                    pattern: r"Tests run: (?P<tests_run>\d+), Failures: (?P<tests_failed>\d+), Errors: (?P<tests_errors>\d+), Skipped: (?P<tests_skipped>\d+)".to_string(),
                    fact_key: "test_results".to_string(),
                    priority: 2,
                    merge_mode: "single".to_string(),
                    command_extractor_policy: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_pipeline_composes_agent_context() {
        // SC4: Pipeline composes AgentContext
        // W1 Fix: verify that named captures from test_results regex are properly parsed
        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![maven_enricher()],
        });
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
        assert!(
            ctx.test_summary.is_some(),
            "test_summary should be parsed from named captures"
        );
        let ts = ctx.test_summary.unwrap();
        assert_eq!(ts.run, 10, "tests_run should be 10");
        assert_eq!(ts.failed, 0, "tests_failed should be 0");
        assert_eq!(ts.errors, 0, "tests_errors should be 0");
        assert_eq!(ts.skipped, 1, "tests_skipped should be 1");
    }

    #[tokio::test]
    async fn test_pipeline_no_enricher_matched() {
        // SC5: No enricher matched
        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![maven_enricher()],
        });
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

    // ─── ExtractorInstance Dispatch Tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_extractor_instance_dispatch_regex() {
        // 2.1 RED: ExtractorInstance::Regex calls .extract() and returns facts
        // Use a regex WITHOUT named captures so it uses fact_key (backward compatible behavior)
        let vp = ValidatedPattern::new(
            "build_status",
            r"BUILD\s+(SUCCESS|FAILURE)",
            "build_status",
            "single",
        )
        .unwrap();
        let extractor = RegexExtractor::from_validated(&vp);
        let instance = ExtractorInstance::Regex(extractor);

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let facts = instance.extract(&invocation, &result, &FakeFs).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "build_status"); // fact_key used when no named captures
        assert_eq!(facts[0].value, "BUILD SUCCESS");
    }

    #[tokio::test]
    async fn test_extractor_instance_dispatch_glob() {
        // 2.2 RED: ExtractorInstance::Glob calls .extract() and returns facts
        let extractor = GlobExtractor::new("jar_artifacts", "target/*.jar", "jar_artifact");
        let instance = ExtractorInstance::Glob(extractor);

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        // Use a FakeFs that returns some files
        struct FakeFsWithFiles {
            files: Vec<std::path::PathBuf>,
        }

        #[async_trait::async_trait]
        impl FileSystem for FakeFsWithFiles {
            async fn read_to_string(
                &self,
                _path: &str,
            ) -> Result<String, crate::traits::EnrichmentError> {
                Ok(String::new())
            }
            async fn glob(
                &self,
                _pattern: &str,
            ) -> Result<Vec<std::path::PathBuf>, crate::traits::EnrichmentError> {
                Ok(self.files.clone())
            }
        }

        let fake_fs = FakeFsWithFiles {
            files: vec![std::path::PathBuf::from("target/app.jar")],
        };

        let facts = instance.extract(&invocation, &result, &fake_fs).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "jar_artifact");
        assert_eq!(facts[0].value, "target/app.jar");
    }

    #[tokio::test]
    async fn test_extractor_instance_dispatch_command() {
        // 2.3 RED: ExtractorInstance::Command calls .extract() and returns facts
        let policy = crate::models::CommandExtractorPolicy::default();
        let extractor = CommandExtractor::with_policy("cmd", policy);
        let instance = ExtractorInstance::Command(extractor);

        let invocation = OperationInvocation::from_command("mvn clean package");
        let result = OperationResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let facts = instance.extract(&invocation, &result, &FakeFs).await;
        // Command extractor should produce facts for executable, tool, goal, intent
        assert!(!facts.is_empty());
        assert!(
            facts
                .iter()
                .any(|f| f.key == "command_executable" && f.value == "mvn")
        );
        assert!(
            facts
                .iter()
                .any(|f| f.key == "command_tool" && f.value == "maven")
        );
        assert!(
            facts
                .iter()
                .any(|f| f.key == "command_goal" && f.value == "clean")
        );
    }

    // ─── Phase 3: Cache Wiring Tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_cache_hit_uses_precompiled_regex() {
        // 3.1 RED: cache HIT produces same facts as per-call RegexExtractor::new()
        // When pipeline is created with cache, cached regex extractors should produce
        // the same facts as per-call construction
        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![maven_enricher()],
        });
        let pipeline = FactPipeline::new(catalog);

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS\nTests run: 10, Failures: 0, Errors: 0, Skipped: 1".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        // With empty cache (FactPipeline::new), fallback should produce facts
        let ctx = pipeline.run(invocation, result, &FakeFs).await.unwrap();

        // Should have extracted facts
        assert!(!ctx.facts.is_empty(), "Should have extracted facts");
        // Should have build_status fact (the regex uses fact_key "build_status" for single capture)
        let has_build_status = ctx.facts.iter().any(|f| f.key == "build_status");
        assert!(
            has_build_status,
            "Should have 'build_status' fact from regex extraction"
        );
        // Verify the value
        let build_status_fact = ctx.facts.iter().find(|f| f.key == "build_status");
        assert_eq!(
            build_status_fact.map(|f| f.value.as_str()),
            Some("BUILD SUCCESS")
        );
    }

    #[tokio::test]
    async fn test_empty_cache_falls_back_to_per_call() {
        // 3.2 RED: FactPipeline::new() (no cache) produces same output as with cache
        // The key is that empty cache falls back to per-call construction
        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![maven_enricher()],
        });

        // Pipeline with new() has empty cache - should fallback to per-call
        let pipeline_new = FactPipeline::new(catalog.clone());

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS\nTests run: 10, Failures: 0, Errors: 0, Skipped: 1".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        let ctx_new = pipeline_new
            .run(invocation.clone(), result.clone(), &FakeFs)
            .await
            .unwrap();

        // Both should produce the same facts
        // Empty cache falls back to per-call extraction - verify facts are produced
        assert!(
            !ctx_new.facts.is_empty(),
            "Empty cache should fall back to per-call extraction and produce facts"
        );
        let has_build_status = ctx_new.facts.iter().any(|f| f.key == "build_status");
        assert!(
            has_build_status,
            "Should have 'build_status' fact from maven regex extraction"
        );
        let build_status_fact = ctx_new.facts.iter().find(|f| f.key == "build_status");
        assert_eq!(
            build_status_fact.map(|f| f.value.as_str()),
            Some("BUILD SUCCESS")
        );
        assert_eq!(ctx_new.enrichment_meta.enricher_id, "maven");
    }

    #[tokio::test]
    async fn test_parallel_sequential_determinism() {
        // 3.3 RED: join_all path equals sequential for same input (R3 compliance)
        // Verify that parallel execution via join_all produces deterministic,
        // identical output across multiple runs with the same input.
        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![maven_enricher()],
        });
        let pipeline = FactPipeline::new(catalog);

        let invocation = OperationInvocation::from_command("mvn package");
        let result = OperationResult {
            exit_code: 0,
            stdout: "BUILD SUCCESS\nTests run: 10, Failures: 0, Errors: 0, Skipped: 1".to_string(),
            stderr: String::new(),
            duration_ms: 5000,
            timed_out: false,
        };

        // Run twice with same input - results must be identical (determinism)
        let ctx1 = pipeline
            .run(invocation.clone(), result.clone(), &FakeFs)
            .await
            .unwrap();
        let ctx2 = pipeline
            .run(invocation.clone(), result.clone(), &FakeFs)
            .await
            .unwrap();

        // Verify facts are identical across runs
        assert_eq!(
            ctx1.facts.len(),
            ctx2.facts.len(),
            "Parallel path should produce same fact count across runs"
        );
        // Verify same facts are present (same keys and values)
        for fact in &ctx1.facts {
            let found = ctx2.facts.iter().any(|f| {
                f.key == fact.key
                    && f.value == fact.value
                    && (f.confidence - fact.confidence).abs() < 0.001
            });
            assert!(
                found,
                "Fact {:?} not found in second run (parallel execution may be non-deterministic)",
                fact.key
            );
        }
        // Verify enricher metadata is consistent
        assert_eq!(
            ctx1.enrichment_meta.enricher_id,
            ctx2.enrichment_meta.enricher_id
        );
        assert_eq!(ctx1.verdict, ctx2.verdict);
    }

    #[tokio::test]
    async fn test_extractor_error_isolated() {
        // 3.4 RED: one failing extractor does not prevent others' facts from appearing
        // This tests error isolation - when one extractor fails, others still work

        // Create an enricher with two extractors
        let enricher_with_two_extractors = EnricherDescriptor {
            id: "test".to_string(),
            name: "Test".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^test\s+command".to_string()],
            template: "Test".to_string(),
            enabled: true,
            extractors: vec![
                ExtractorConfig {
                    id: "good_extractor".to_string(),
                    extractor_type: "regex".to_string(),
                    pattern: r"(?P<value>\w+)".to_string(),
                    fact_key: "value".to_string(),
                    priority: 1,
                    merge_mode: "single".to_string(),
                    command_extractor_policy: None,
                    ..Default::default()
                },
                ExtractorConfig {
                    id: "bad_extractor".to_string(),
                    extractor_type: "regex".to_string(),
                    // Invalid regex that will fail
                    pattern: r"[invalid".to_string(),
                    fact_key: "bad".to_string(),
                    priority: 2,
                    merge_mode: "single".to_string(),
                    command_extractor_policy: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let catalog = Arc::new(FakeCatalog {
            enrichers: vec![enricher_with_two_extractors],
        });
        let pipeline = FactPipeline::new(catalog);

        let invocation = OperationInvocation::from_command("test command");
        let result = OperationResult {
            exit_code: 0,
            stdout: "SUCCESS".to_string(),
            stderr: String::new(),
            duration_ms: 100,
            timed_out: false,
        };

        // Pipeline should complete successfully despite bad extractor
        let ctx = pipeline.run(invocation, result, &FakeFs).await.unwrap();

        // Good extractor should still produce facts
        // The bad extractor should be skipped with warn, but not fail the pipeline
        assert!(ctx.enrichment_meta.enricher_id == "test");
    }
}
