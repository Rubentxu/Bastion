//! # Enrichment Engine
//!
//! Host-agnostic enrichment engine: models, traits, extractors, and the FactPipeline.
//!
//! All core types are framework-free (no Bastion, no MCP) and serde-serializable.
//!
//! ## Architecture
//!
//! - [`models`] — Domain models: `OperationInvocation`, `OperationResult`, `Fact`,
//!   `AgentContext`, `EnricherDescriptor`, `ExtractorConfig`, `RuleConfig`, `RuleAction`, `RuleOutput`
//! - [`traits`] — Ports: `CatalogRepository`, `FactStore`, `FileSystem`, `Extractor`, `RuleRepository`
//! - [`extractors`] — Implementations: `RegexExtractor`, `GlobExtractor`
//! - [`pipeline`] — Orchestration: `FactPipeline`
//! - [`composer`] — Template rendering: `AgentContextComposer`
//! - [`normalizer`] — Fact normalization: `FactNormalizer`
//! - [`rules`] — CEL-lite rule engine: lexer, AST, evaluator
//!
//! ## Example
//!
//! ```ignore
//! use enrichment_engine::pipeline::FactPipeline;
//! use enrichment_engine::models::{OperationInvocation, OperationResult};
//! use std::sync::Arc;
//!
//! let catalog = Arc::new(MyCatalogRepository::new());
//! let pipeline = FactPipeline::new(catalog);
//! let invocation = OperationInvocation::from_command("mvn package");
//! let result = OperationResult { exit_code: 0, stdout: "BUILD SUCCESS".into(), ..Default::default() };
//! let ctx = pipeline.run(invocation, result, fs).await?;
//! ```

// ─── Public re-exports ────────────────────────────────────────────────────────

pub use crate::models::{
    AgentContext, CommandExtractorPolicy, EnricherDescriptor, EnrichmentMeta,     EnrichmentRunRecord,
    ExtractorConfig, Fact, OperationInvocation, OperationResult, RuleAction, RuleConfig, RuleOutput,
    TestSummary, UtilityMetrics,
};
pub use crate::traits::{CatalogRepository, EnrichmentError, Extractor, FactStore, FileSystem, RuleRepository, RunRecorder};
pub use crate::rules::{DefaultRuleEvaluator, Expr, ParseError, RuleEvaluator};
pub use crate::optimizer::{OptimizerReport, EnricherScore, OptimizationRecommendation, RecAction, OptimizerRepository, AggregateStats};
pub use crate::sanitizer::sanitize_command;

// ─── Modules ─────────────────────────────────────────────────────────────────

pub mod composer;
pub mod enrichers;
pub mod extractors;
pub mod intent;
pub mod models;
pub mod normalizer;
pub mod optimizer;
pub mod pipeline;
pub mod rules;
pub mod sanitizer;
pub mod traits;
pub mod truncate;
