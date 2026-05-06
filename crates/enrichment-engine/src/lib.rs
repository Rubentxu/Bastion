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
    AgentContext, EnricherDescriptor, EnrichmentMeta, ExtractorConfig, Fact,
    OperationInvocation, OperationResult, RuleAction, RuleConfig, RuleOutput, TestSummary,
};
pub use crate::traits::{CatalogRepository, EnrichmentError, Extractor, FactStore, FileSystem, RuleRepository};
pub use crate::rules::{DefaultRuleEvaluator, Expr, ParseError, RuleEvaluator};

// ─── Modules ─────────────────────────────────────────────────────────────────

pub mod composer;
pub mod enrichers;
pub mod extractors;
pub mod intent;
pub mod models;
pub mod normalizer;
pub mod pipeline;
pub mod rules;
pub mod traits;
