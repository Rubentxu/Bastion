# Catalog runtime implementation plan

> **Date:** 2026-05-05  
> **Status:** Implementation roadmap  
> **Depends on:** `docs/research/extensible-catalogs-doctor-suggestions-workflows.md`  
> **Goal:** Implement Bastion's catalog-first, agent-first extensibility model
> incrementally without over-hardcoding workflows, doctor checks, suggestions,
> or assertions.

## Executive summary

Bastion should implement catalog extensibility in small, verifiable layers. The
first MVP must be narrow: capture structured experience, evaluate cataloged
assertions, and produce advice from data. Only after this foundation should
Bastion add doctor checks, workflows, community catalogs, or executable plugins.

The recommended sequence is:

```text
Experience records → Assertion engine → Advice engine → Doctor engine →
Pattern/artifact detectors → Workflow engine → Self-improvement → Community catalogs
```

This order avoids the main failure mode: creating a large, flexible workflow
system before Bastion has a reliable validation vocabulary.

---

## Design goals

### Primary goals

1. Keep Bastion core generic and stable.
2. Move domain knowledge into catalogs.
3. Let users and agents extend behavior without recompiling Bastion.
4. Capture evidence so suggestions and workflows are grounded in real runs.
5. Allow catalog improvements through reviewable proposals, not direct mutation.

### Non-goals for the first implementation

1. No native plugin loading.
2. No remote community marketplace yet.
3. No arbitrary script execution from untrusted catalogs.
4. No full workflow engine in the first MVP.
5. No autonomous catalog writes without user approval.

---

## Current codebase fit

Bastion already has a suitable Clean Architecture structure:

```text
crates/
├── bastion-domain          # catalog types and pure validation models
├── bastion-application     # use cases: evaluate, validate, propose, run
├── bastion-infrastructure  # TOML loaders, SQLite stores, filesystem catalogs
└── bastion-gateway         # MCP tools exposing catalog functionality
```

Existing related modules:

- `bastion-domain/src/template/*` — capabilities, artifacts, toolchains.
- `bastion-infrastructure/src/template/capability_registry.rs` — TOML loading
  pattern to reuse.
- `bastion-gateway/src/server.rs` — MCP tool surface.
- `SqliteSandboxRepository` — persistence precedent for experience storage.

### Recommended module placement

```text
crates/bastion-domain/src/catalog/
├── mod.rs
├── manifest.rs
├── assertion.rs
├── advice.rs
├── doctor.rs
├── pattern.rs
├── artifact_detector.rs
├── workflow.rs
├── experience.rs
├── proposal.rs
└── policy.rs

crates/bastion-application/src/catalog/
├── evaluate_assertion.rs
├── evaluate_advice.rs
├── record_experience.rs
├── validate_catalog.rs
├── suggest_improvement.rs
└── dry_run_candidate.rs

crates/bastion-infrastructure/src/catalog/
├── fs_catalog_loader.rs
├── toml_catalog_parser.rs
├── sqlite_experience_store.rs
├── sqlite_candidate_store.rs
└── schema_validation.rs

crates/bastion-gateway/src/catalog_tools.rs
```

`server.rs` is already large. New catalog MCP tools should be moved into a
separate `catalog_tools.rs` module to avoid further growth.

---

## Weak points and mitigations

### Weak point 1: Scope explosion

Catalogs can cover assertions, advice, doctors, workflows, patterns, artifacts,
plugins, and self-improvement. Building all at once would stall the project.

**Mitigation:** implement one vertical slice first:

```text
experience record → assertion evaluation → advice suggestion
```

### Weak point 2: Unsafe user/agent-generated commands

Catalogs can suggest shell commands. An agent could propose dangerous commands.

**Mitigation:** every action has a risk level and capability requirement:

```toml
risk = "safe" | "writes_sandbox" | "host_read" | "host_write" | "privileged"
requires_approval = true
```

MVP should only allow safe MCP actions, not host shell remediation.

### Weak point 3: Overfitting from one failed run

An agent may create advice that only matches one accidental condition.

**Mitigation:** require evidence references, confidence, scope, and dry-run
against saved experiences. Mark single-run proposals as `experimental`.

### Weak point 4: Descriptor language becomes too powerful

If templates support arbitrary expressions too early, the system becomes hard
to secure and debug.

**Mitigation:** start with simple variable interpolation only:

```text
{{ sandbox_id }}
{{ project_dir }}
{{ steps.prepare.output.env_ref }}
```

Avoid loops and custom functions in the MVP.

### Weak point 5: Duplicate concepts with existing capability registry

Bastion already has `capabilities/*.toml`. A new catalog system could overlap.

**Mitigation:** treat current capability registry as a legacy/specialized
catalog. Do not replace it immediately. Add adapters later:

```text
CapabilityRegistry → CatalogRegistry adapter
```

### Weak point 6: `server.rs` becomes unmaintainable

New tools can easily bloat the gateway.

**Mitigation:** split MCP handlers by bounded context:

```text
server.rs           # core gateway state
sandbox_tools.rs    # sandbox MCP tools
catalog_tools.rs    # catalog MCP tools
workflow_tools.rs   # future workflow MCP tools
```

### Weak point 7: Community catalogs and supply chain risk

Remote catalogs can be malicious or stale.

**Mitigation:** remote install comes late. Start with local/project catalogs.
When remote catalogs arrive, require pinned refs, signatures, metadata, and
allowlists.

### Weak point 8: Assertions may duplicate tests

Assertions could blur the line between runtime validation and test suites.

**Mitigation:** define assertions as runtime validation primitives. Tests can
reuse them, but assertions are not Rust unit tests.

---

## Core data model

### `ExperienceRecord`

Represents one observed tool execution.

```rust
pub struct ExperienceRecord {
    pub id: ExperienceId,
    pub trace_id: Option<String>,
    pub tool: String,
    pub provider: Option<String>,
    pub sandbox_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub status: ExperienceStatus,
    pub evidence: Vec<EvidenceItem>,
}
```

MVP storage: SQLite table `experiences` with JSON payloads.

### `AssertionDescriptor`

```rust
pub struct AssertionDescriptor {
    pub id: String,
    pub description: String,
    pub checks: Vec<AssertionCheck>,
    pub advice_on_failure: Vec<String>,
}
```

Initial check types:

```rust
pub enum AssertionCheck {
    ExitCodeEquals { value: i32 },
    StdoutContains { value: String },
    StderrContains { value: String },
    OutputJsonPathEquals { path: String, value: serde_json::Value },
    FileExists { path: String },
    SandboxAlive,
}
```

MVP should avoid regex until pattern catalogs are introduced.

### `AdviceRule`

```rust
pub struct AdviceRule {
    pub id: String,
    pub default_enabled: bool,
    pub match_condition: AdviceMatch,
    pub actions: Vec<SuggestedAction>,
}
```

Initial match types:

```rust
pub enum AdviceMatch {
    FailedAssertion { assertion_id: String },
    ToolErrorCode { tool: String, error_code: String },
    ToolNameAndExitCode { tool: String, exit_code: i32 },
}
```

### `SuggestedAction`

```rust
pub struct SuggestedAction {
    pub title: String,
    pub kind: SuggestedActionKind,
    pub risk: RiskLevel,
    pub reason: String,
}

pub enum SuggestedActionKind {
    McpTool { tool: String, arguments: serde_json::Value },
    Message { body: String },
    Documentation { url: String },
}
```

MVP should only execute none of these automatically. They are recommendations.

### `CatalogProposal`

```rust
pub struct CatalogProposal {
    pub id: ProposalId,
    pub kind: ProposalKind,
    pub derived_from: Vec<ExperienceId>,
    pub confidence: f32,
    pub risk: RiskLevel,
    pub scope: ProposalScope,
    pub proposed_entry: serde_json::Value,
    pub status: ProposalStatus,
}
```

---

## Catalog file formats for MVP

### Assertion catalog

```toml
[[assertions]]
id = "command.exit_code.zero"
description = "Command exits successfully."

[[assertions.checks]]
type = "exit_code_equals"
value = 0

[[assertions]]
id = "maven.build.success"
description = "Maven reports BUILD SUCCESS."

[[assertions.checks]]
type = "stdout_contains"
value = "BUILD SUCCESS"
```

### Advice catalog

```toml
[[advice]]
id = "advice.maven.missing_pom"
default_enabled = true

[advice.match]
type = "failed_assertion"
assertion_id = "maven.pom.exists"

[[advice.actions]]
kind = "mcp_tool"
title = "List workspace files"
tool = "sandbox_list_files"
risk = "safe"
arguments = { sandbox_id = "{{ sandbox_id }}", path = "/workspace" }
```

### Minimal manifest

```toml
[catalog]
id = "project.local"
version = "0.1.0"
min_bastion_version = "0.2.0"
```

---

## MVP roadmap

### MVP 0 — Stabilize current gateway

**Purpose:** fix obvious issues before adding catalog runtime.

Tasks:

1. Add tool registry regression test for required MCP tools.
2. Fix empty `sandbox_sync` error when stderr is empty.
3. Check sandbox/provider liveness before sync.
4. Move pool stats log from `DEBUG` to `TRACE` or log on state change.

Verification:

- `tools/list` includes `sandbox_run`, `sandbox_run_stream`, `sandbox_cancel`,
  `sandbox_prepare`, and `sandbox_sync`.
- Sync on dead container returns clear error.

### MVP 1 — Experience capture

**Purpose:** create the evidence layer.

Tasks:

1. Add `ExperienceRecord` domain type.
2. Add `ExperienceStore` trait.
3. Add SQLite implementation.
4. Record experiences for `sandbox_run`, `sandbox_run_stream`,
   `sandbox_prepare`, `sandbox_sync`, and `sandbox_cancel`.
5. Add `trace_id` optional param to command-like tools.
6. Expose MCP tools:
   - `experience_list`
   - `experience_get`

Verification:

- PetClinic run produces queryable experience records by `trace_id`.
- Experience output contains tool, sandbox, timing, exit code/error, and summary.

### MVP 2 — Assertion catalog engine

**Purpose:** add reusable validation primitives.

Tasks:

1. Add assertion descriptor domain types.
2. Add TOML parser for assertion catalogs.
3. Add `AssertionEngine` for saved experiences.
4. Add MCP tools:
   - `assertion_list`
   - `assertion_run`
   - `assertion_dry_run`
5. Add built-in assertions:
   - `command.exit_code.zero`
   - `maven.build.success`
   - `sandbox.container.alive`

Verification:

- `assertion_run(maven.build.success, experience_id=...)` passes for PetClinic.
- `assertion_run(command.exit_code.zero, failed_experience)` fails with
  structured output.

### MVP 3 — Advice engine

**Purpose:** produce Git-like next recommended actions from catalog rules.

Tasks:

1. Add advice descriptor domain types.
2. Add TOML parser for advice catalogs.
3. Add `AdviceEngine` over `ExperienceRecord` and `AssertionResult`.
4. Add `next_recommended` to selected tool responses.
5. Add advice configuration:
   - `advice.enabled`
   - per-rule enable/disable
   - `BASTION_ADVICE=0`
6. Add MCP tools:
   - `advice_list`
   - `advice_configure`

Verification:

- Missing POM experience produces Maven advice.
- Dead sync experience produces `sandbox_info` recommendation.
- Advice can be disabled globally.

### MVP 4 — Doctor engine

**Purpose:** diagnose provider readiness through assertions.

Tasks:

1. Add doctor descriptor that references assertion packs.
2. Add host-level assertion checks:
   - command exists
   - path exists
   - env var present
   - unix socket exists
   - command succeeds
3. Add provider doctor catalogs for Podman and Local.
4. Expose `sandbox_doctor`.

Verification:

- `sandbox_doctor(provider=podman)` reports socket, binary, and ping status.
- `sandbox_doctor(provider=local)` reports `DANGEROUS_ALLOW_LOCAL` status.

### MVP 5 — Pattern and artifact catalogs

**Purpose:** decouple build-system knowledge from workflows.

Tasks:

1. Add pattern descriptor and matcher.
2. Add artifact detector descriptor and evaluator.
3. Add Maven patterns:
   - `BUILD SUCCESS`
   - `BUILD FAILURE`
   - missing POM
4. Add Java JAR artifact detector.
5. Attach pattern/artifact results to experiences.

Verification:

- PetClinic Maven experience records `maven.build.success`.
- JAR detector finds the 67MB artifact.

### MVP 6 — Improvement candidate pipeline

**Purpose:** let agents propose catalog improvements safely.

Tasks:

1. Add `CatalogProposal` and `ImprovementCandidate` domain types.
2. Add candidate store.
3. Add MCP tools:
   - `experience_summarize`
   - `improvement_suggest`
   - `improvement_validate`
   - `improvement_dry_run`
   - `catalog_diff`
4. Limit MVP suggestions to advice and assertions.
5. Require user approval for `improvement_apply`.

Verification:

- From dead sync experience, agent can generate an advice proposal.
- Proposal validates and dry-runs against the saved failing experience.

### MVP 7 — Workflow engine, Maven first

**Purpose:** automate conventional workflows after validation primitives exist.

Tasks:

1. Add workflow descriptor domain types.
2. Add workflow dry-run planner.
3. Add primitive action executor for existing MCP operations.
4. Add `workflow_list`, `workflow_describe`, `workflow_dry_run`,
   `workflow_run`.
5. Implement `java.maven.package` catalog workflow.

Verification:

- PetClinic can be built through one `workflow_run` call.
- Workflow emits step evidence, assertions, artifacts, and next recommendations.

### MVP 8 — Community catalogs

**Purpose:** share and install catalogs beyond the project.

Tasks:

1. Add catalog packaging.
2. Add local path install.
3. Add Git source install with pinned SHA.
4. Add catalog examples and `catalog_test`.
5. Add optional signing later.

Verification:

- A Maven catalog can be installed from a local path.
- Catalog tests pass before activation.

---

## Recommended SDD change split

Use small SDD changes rather than one giant feature:

1. `stabilize-tools-sync-logs`
2. `experience-records-trace-id`
3. `assertion-catalog-engine`
4. `advice-catalog-engine`
5. `doctor-from-assertions`
6. `pattern-artifact-catalogs`
7. `catalog-improvement-pipeline`
8. `workflow-engine-maven-first`
9. `community-catalog-install`

Each change should include PetClinic or provider-doctor verification scenarios.

---

## Implementation order recommendation

Start with:

```text
stabilize-tools-sync-logs → experience-records-trace-id → assertion-catalog-engine
```

Do **not** start with workflows. Workflows are the most visible feature, but
they depend on evidence, assertions, advice, patterns, artifacts, and safety
policy to be reliable.

---

## Acceptance criteria for the first milestone

The first milestone should be considered complete when this is possible:

1. Run PetClinic with `trace_id = "petclinic-fase014"`.
2. Query `experience_list(trace_id)`.
3. Run assertions against the Maven build experience.
4. Get structured pass/fail assertion results.
5. Trigger one advice recommendation from a failed experience.
6. Disable that advice rule and confirm it no longer appears.

This validates the foundation before building doctor and workflow automation.
