# Enrichment Engine — API Reference

> **Project**: Bastion MCP Gateway
> **Crate**: `crates/enrichment-engine`
> **Edition**: Rust 2024, MSRV 1.85
> **Status**: Production

The enrichment engine is a **host-agnostic** library: zero Bastion types, zero MCP types in the core. All models are serde-serializable.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     Host Application                          │
│  (BastionGateway → BastionEnrichmentAdapter → Pipeline)     │
└──────────────────────────┬──────────────────────────────────┘
                           │ OperationInvocation + OperationResult
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                    FactPipeline                              │
│  ┌─────────────┐  ┌──────────┐  ┌────────────┐            │
│  │  Intent    │→ │ Extractor│→ │  Normalizer │            │
│  │  Detector  │  │  Chain   │  │             │            │
│  └─────────────┘  └──────────┘  └──────┬─────┘            │
│                                         │                   │
│                         ┌───────────────┴──────────────┐   │
│                         ▼                              ▼   │
│                   ┌──────────┐              ┌──────────┐   │
│                   │  Rules   │              │ Composer │   │
│                   │Evaluator │              │          │   │
│                   └──────────┘              └──────────┘   │
│                         │                              │   │
│                         └──────────────┬───────────────┘   │
└─────────────────────────────────────────┼─────────────────┘
                                          │ AgentContext
                                          ▼
                              ┌───────────────────────────┐
                              │   AgentContextComposer    │
                              │  (verdict + recommendations)│
                              └───────────────────────────┘
```

### Data Flow

1. `CommandSpec` + `CommandResult` → `OperationInvocation` + `OperationResult`
2. Pipeline detects intent → selects enrichers → runs extractors → normalizes facts
3. Rules engine evaluates CEL-lite expressions → emits verdict + recommendations
4. Composer renders `AgentContext` with facts, build status, artifacts, and advice

---

## Public Types

All types below are re-exported from `enrichment_engine::*`.

### Models

| Type | Description |
|------|-------------|
| `AgentContext` | Final output: facts, build_status, artifacts, test_summary, verdict, recommendations |
| `EnrichmentRunRecord` | Telemetry record for Meta-Harness optimizer |
| `EnrichmentMeta` | Metadata injected into each fact: source, timestamp, enricher_id |
| `EnricherDescriptor` | Enricher definition: id, name, extractors, rules, enabled |
| `ExtractorConfig` | Extractor configuration: type, pattern, merge_mode, fact_key |
| `OperationInvocation` | Command description: executable, goals, flags, targets, intent |
| `OperationResult` | Command result: exit_code, stdout, stderr, duration_ms, timed_out |
| `Fact` | A key-value-similarity triple with optional source metadata |
| `RuleConfig` | CEL-lite rule: expression, action, priority |
| `RuleAction` | Rule evaluation result: `Hot` / `Warn` / `Info` |
| `RuleOutput` | Rule hit output: verdict, recommendations, confidence |
| `TestSummary` | Parsed Maven test summary: counts, duration, failures |
| `UtilityMetrics` | Harness metrics for optimizer scoring |
| `RetentionConfig` | Retention policy: `max_age_days`, `max_rows`, `enabled`, `sanitize` |

### Traits (Ports)

| Trait | Description |
|-------|-------------|
| `CatalogRepository` | Find enrichers by command pattern |
| `Extractor` | Extract facts from `OperationInvocation` + `OperationResult` |
| `FactStore` | Persist/query extracted facts |
| `FileSystem` | File read and glob operations for extractors |
| `RuleEvaluator` | Evaluate CEL-lite expressions against facts |
| `RuleRepository` | Load rule configs for an enricher |
| `RunRecorder` | Persist `EnrichmentRunRecord` for optimizer telemetry |
| `OptimizerRepository` | Read records for optimizer report generation |

### Optimizer Types

| Type | Description |
|------|-------------|
| `OptimizerReport` | Full analysis: scores per enricher, recommendations |
| `EnricherScore` | Per-enricher metrics: runs, artifact_yield, latency, accuracy |
| `OptimizationRecommendation` | Action suggestion with confidence |
| `AggregateStats` | Aggregated telemetry for an enricher |

---

## Usage Examples

### Basic Pipeline Setup

```rust
use enrichment_engine::pipeline::FactPipeline;
use enrichment_engine::models::{OperationInvocation, OperationResult};
use enrichment_engine::traits::{CatalogRepository, FileSystem};
use enrichment_engine::sanitizer::sanitize_command;
use std::sync::Arc;

struct MyCatalog;
struct MyFileSystem;

#[enrichment_engine::async_trait]
impl CatalogRepository for MyCatalog {
    async fn find_enrichers(&self, cmd: &str) -> Vec<EnricherDescriptor> { todo!() }
    async fn list_all(&self) -> Vec<EnricherDescriptor> { todo!() }
}

#[enrichment_engine::async_trait]
impl FileSystem for MyFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String, EnrichmentError> { todo!() }
    async fn glob(&self, pattern: &str) -> Result<Vec<std::path::PathBuf>, EnrichmentError> { todo!() }
}

let catalog: Arc<dyn CatalogRepository> = Arc::new(MyCatalog);
let fs: Arc<dyn FileSystem> = Arc::new(MyFileSystem);
let pipeline = FactPipeline::new(catalog, fs);

let invocation = OperationInvocation::from_command("mvn test");
let result = OperationResult {
    exit_code: 0,
    stdout: "Tests run: 42, Failures: 0, Errors: 0".into(),
    stderr: "".into(),
    duration_ms: 12_000,
    timed_out: false,
};

let ctx = pipeline.run(invocation, result).await?;
println!("Facts: {} items, Verdict: {:?}", ctx.facts.len(), ctx.verdict);
```

### Sanitizing Commands

```rust
use enrichment_engine::sanitize_command;

// Redacts: Bearer tokens, api_key=, --password, AWS keys, etc.
let clean = sanitize_command("curl -H 'Authorization: Bearer secret123' https://api.example.com");
// Returns: "curl -H 'Authorization: ***' https://api.example.com"
```

### Retention Policy

```rust
use enrichment_engine::models::RetentionConfig;

let config = RetentionConfig {
    max_age_days: 90,
    max_rows: 100_000,
    enabled: true,
    sanitize: true,
};
```

---

## Error Handling

### EnrichmentError Variants

```rust
pub enum EnrichmentError {
    FileSystem(String),    // File read or glob failed
    Catalog(String),       // Enricher lookup failed
    Extraction(String),    // Extractor panicked or returned error
    Config(String),        // Invalid configuration
    Recorder(String),      // Persistence failed (fire-and-forget)
    Io(std::io::Error),   // Low-level I/O error
}
```

### Error Semantics

- **Extraction errors**: Logged via `tracing::warn!`, pipeline continues with empty facts
- **Recorder errors**: Fire-and-forget — logged via `tracing::warn!`, never propagated
- **Catalog errors**: Return `None` from `enrich()`, gateway degrades gracefully
- **Enrichment disabled**: `enrich()` returns `None` immediately, no overhead

### MCP Tool Error JSON Shapes

When the optimizer or recorder is not configured, tools return:

```json
// enrichment_optimizer_report
{ "error": "enrichment recorder not configured" }
{ "error": "enrichment optimizer repository not configured" }

// enrichment_retention_info / enrichment_retention_cleanup
{ "error": "enrichment recorder not configured" }
```

On success, tools return the full JSON payload described in the tool docstrings.

---

## Backward Compatibility

### Additive-Only Contract

**No existing API, schema, or behavior changes** without a major version bump. The finalization suite is strictly additive:

| Change | Impact |
|--------|--------|
| New `schema_version` table | Existing DBs unaffected — append-only migrations |
| New `sandbox_id` column | Old rows get `NULL` — no data loss |
| New MCP tools | Existing tools unaffected — additive registration |
| New `RunRecorder` methods | Default impl available, existing recorders compile unchanged |

### Schema Migration Guarantees

- Every migration is **transactional** (`BEGIN IMMEDIATE` + `COMMIT` / `ROLLBACK`)
- Migrations are **idempotent** — checking `MAX(version)` skips already-applied
- **Non-destructive** — only `ADD COLUMN`, never `DROP COLUMN`
- DB backup on first migration: `enrichment_runs.db.backup`

### Secret Sanitization

Commands are sanitized before recording when `RetentionConfig.sanitize = true`:

| Pattern | Redacted |
|---------|----------|
| `Bearer <token>` | `Bearer ***` |
| `api_key=<value>` | `api_key=***` |
| `--password <value>` | `--password ***` |
| AWS keys (`AKIA...`) | `***` |
| `--secret` flags | `--secret ***` |
| `--api-key` flags | `--api-key ***` |
| GitHub tokens (`ghp_`, `gho_`, `ght_`, `ghs_` + 8+ chars) | `ghp_***` |
| OpenAI keys (`sk-` 48+ chars, `sk-proj-`, `sk-svcacct-`, `sk-admin-` 16+ chars) | `sk-***` |
| JWT tokens (`eyJ...` with x.y.z structure, 50-1200 chars) | `eyJ***` |

### Observability

#### Metrics (`EnrichmentMetrics`)

Thread-safe counters for pipeline telemetry (zero-alloc on hot path):

- `total_success` — Successful enrichment runs
- `total_failure` — Failed enrichment runs
- `saturation_drops` — Records dropped due to backpressure
- `facts_total` — Total facts extracted
- `p50_latency_ms` / `p99_latency_ms` — Percentile latencies

#### Health (`EnrichmentHealth`)

Operational snapshot via `BastionEnrichmentAdapter::health()`:

- `enabled` — Whether enrichment is active
- `catalog_enricher_count` — Number of configured enrichers
- `recent_runs_5min` — Approximate total runs
- `saturation_events` — Backpressure drop count
- `db_row_count` — Database size (if recorder wired)
- `recorder_available` — Recorder presence

---

## Cross-References

| Topic | Location |
|-------|----------|
| Optimizer scoring algorithm | `crates/enrichment-engine/src/optimizer.rs` |
| CEL-lite rule evaluator | `crates/enrichment-engine/src/rules/` |
| SQLite run recorder | `crates/bastion-infrastructure/src/enrichment/sqlite_recorder.rs` |
| Retention cleanup | `crates/bastion-infrastructure/src/enrichment/sqlite_recorder.rs` |
| Optimizer read-side | `crates/bastion-infrastructure/src/enrichment/sqlite_optimizer_repo.rs` |
| Schema migration | `crates/bastion-infrastructure/src/enrichment/schema_migration.rs` |
| Gateway wiring | `crates/bastion-gateway/src/main.rs` |
| MCP tools | `crates/bastion-gateway/src/enrichment_tools.rs` |
| Design document | `docs/planes/enrichment-engine/README.md` |
