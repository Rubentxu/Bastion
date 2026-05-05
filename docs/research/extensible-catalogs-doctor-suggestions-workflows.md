# Extensible catalogs for doctor, suggestions, and workflows

> **Date:** 2026-05-05  
> **Status:** Architecture research  
> **Scope:** Avoid hardcoded workflows and suggestions in Bastion by designing
> catalog-driven, user-extensible, and community-shareable abstractions.

## Executive summary

Your concern is valid: if Bastion implements suggestions and workflows as Rust
`if maven { ... }` branches, it will become rigid, hard to evolve, and hostile
to community contributions. The right direction is to treat suggestions,
doctor checks, workflows, and log patterns as **catalog entries** with a small
set of built-in primitives.

The core engine should be stable and generic. The knowledge of Maven, npm,
Podman, Firecracker, or gVisor should live mostly in external descriptors that
users, agents, teams, and the community can publish, pin, override, and compose.

The recommended model is:

```text
Bastion core
  ├── Executes primitive actions safely
  ├── Loads signed/versioned catalogs
  ├── Evaluates conditions and templates
  ├── Produces structured evidence
  └── Exposes MCP tools

Catalogs
  ├── Provider doctors
  ├── Suggestions/advice rules
  ├── Workflow definitions
  ├── Log pattern detectors
  └── Artifact detectors
```

---

## External references and lessons

### Dagger: reusable modules and typed functions

Dagger models workflows as **functions** with typed inputs and typed outputs.
Functions can be packaged in modules, shared through Git, pinned by version,
and composed across languages.

Relevant lessons for Bastion:

- Use **typed inputs/outputs**, not only shell strings.
- Treat reusable logic as **modules** or catalog entries.
- Make Git the source of truth for community modules.
- Pin catalog versions by commit/tag to avoid supply-chain drift.
- Enable composition: a workflow can call another workflow.
- Capture execution as a graph/spans for observability.

Design implication:

```text
Bastion workflows should be typed declarative definitions that expand into
primitive MCP actions and return structured outputs.
```

### Git: configurable advice messages

Git has `advice.*` configuration keys. Advice is optional, user-facing,
individually disableable, and can be globally silenced with `GIT_ADVICE=0`.
Git advice is tied to specific situations, such as detached HEAD, non-fast-
forward pushes, or missing submodule initialization.

Relevant lessons for Bastion:

- Advice should be **situational**, not generic noise.
- Each advice rule should have a stable ID.
- Users and agents must be able to disable advice globally or by rule.
- Advice should explain how to silence it.
- Machine consumers should be able to request `advice=off` or structured-only.

Design implication:

```text
Bastion suggestions should be rule-based, configurable, and individually
toggleable: advice.sync.deadContainer=false, advice.maven.missingPom=true.
```

### GitHub Actions: reusable workflows and composite actions

GitHub Actions supports reusable workflows and composite actions. Reusable
workflows define inputs, secrets, outputs, and can be referenced by repository
and version. Composite actions package a sequence of steps behind one action.

Relevant lessons for Bastion:

- Separate **workflow orchestration** from **primitive actions**.
- Define `inputs`, `secrets`, and `outputs` explicitly.
- Allow local references and remote references.
- Prefer pinned versions for safety.
- Support composition but prevent loops.
- Support outputs from steps and workflows.

Design implication:

```text
Bastion workflow catalogs should define inputs, secrets, steps, outputs, and
uses references with version pins.
```

### Backstage Scaffolder: templates, custom actions, and dry-run

Backstage Software Templates use YAML descriptors with parameters, steps,
actions, conditional execution, templating expressions, outputs, custom action
plugins, examples, and dry-run testing.

Relevant lessons for Bastion:

- Use descriptors with JSON Schema-like parameters.
- Use namespaced action IDs (`provider:entity:verb`).
- Separate built-in actions from community actions.
- Provide dry-run validation before executing.
- Store outputs and links as first-class artifacts.
- Allow feature flags and conditional steps.
- Provide examples and tests for custom actions.

Design implication:

```text
Bastion catalogs should have schema validation, dry-run, examples, conditions,
outputs, and namespaced primitive action IDs.
```

---

## Design principles for Bastion

### Principle 1: Core primitives, external knowledge

Bastion core should know how to execute safe primitives:

- `sandbox:create`
- `sandbox:run`
- `sandbox:runStream`
- `sandbox:prepare`
- `sandbox:sync`
- `sandbox:doctorCheck`
- `log:match`
- `artifact:detect`

But it should not hardcode Maven, npm, pytest, Firecracker installation, or
provider-specific remediation in Rust unless it is truly infrastructural.

### Principle 2: Catalog entries are data, not code by default

The first extensibility layer should be declarative TOML/YAML/JSON. This keeps
catalogs safe, reviewable, shareable, and editable by AI agents.

Executable plugin code can come later, behind a stricter trust boundary.

### Principle 3: Versioned and overrideable catalogs

Catalogs should load from multiple scopes with deterministic precedence:

```text
1. Built-in catalog embedded in Bastion
2. System catalog: /etc/bastion/catalogs
3. User catalog: ~/.bastion/catalogs
4. Project catalog: .bastion/catalogs
5. Remote pinned catalog: git+https://...@sha
6. Agent-provided ephemeral catalog for one session
```

Later scopes override earlier scopes by ID.

### Principle 4: Suggestions are structured actions

Suggestions should not be prose only. They should include executable MCP tool
calls, shell commands, documentation links, and safety metadata.

### Principle 5: Workflows emit evidence

Every workflow step should emit structured evidence:

- start/end timestamp
- inputs after templating
- command or primitive action
- exit code
- matched log patterns
- detected artifacts
- next recommendations

This enables agents to evaluate each step and propose corrections.

### Principle 6: Safety and trust are explicit

Catalog entries can suggest commands. Some commands are safe, others are
privileged or destructive. Every action should carry a risk classification.

---

## Proposed catalog architecture

### Directory layout

```text
.bastion/catalogs/
├── catalog.toml
├── providers/
│   ├── podman.doctor.toml
│   ├── local.doctor.toml
│   ├── gvisor.doctor.toml
│   └── firecracker.doctor.toml
├── advice/
│   ├── sync.advice.toml
│   ├── maven.advice.toml
│   └── provider.advice.toml
├── workflows/
│   ├── java.maven.package.toml
│   ├── node.npm.build.toml
│   └── python.pytest.toml
├── patterns/
│   ├── maven.patterns.toml
│   ├── npm.patterns.toml
│   └── podman.patterns.toml
└── artifacts/
    ├── java.jar.toml
    └── node.dist.toml
```

### Catalog manifest

```toml
[catalog]
id = "bastion-community/java"
version = "0.1.0"
description = "Java/Maven workflows, patterns, and advice for Bastion"
license = "Apache-2.0"
min_bastion_version = "0.2.0"

[[catalog.sources]]
type = "git"
url = "https://github.com/bastion-catalogs/java"
ref = "v0.1.0"
sha = "..."

[trust]
signed = true
allow_shell = true
allow_privileged = false
```

---

## Doctor catalog design

Provider doctor checks should be declarative when possible. They should support
files, commands, environment variables, sockets, features, and permissions.

### Example: Podman doctor

```toml
[doctor]
id = "provider.podman"
provider = "podman"
description = "Check whether Podman is available for Bastion."

[[checks]]
id = "podman.binary"
type = "command_exists"
command = "podman"
severity = "critical"

[[checks]]
id = "podman.socket"
type = "unix_socket"
path = "/run/user/{{ uid }}/podman/podman.sock"
severity = "critical"

[[checks]]
id = "podman.ping"
type = "command"
command = "podman info --format json"
timeout_ms = 5000
severity = "critical"

[[remediations]]
when = "podman.socket.failed"
title = "Start the user Podman socket"
risk = "low"
commands = [
  "systemctl --user enable --now podman.socket",
  "systemctl --user status podman.socket"
]
```

### Example: Firecracker doctor

```toml
[doctor]
id = "provider.firecracker"
provider = "firecracker"

[[checks]]
id = "kvm.device"
type = "path_exists"
path = "/dev/kvm"
severity = "critical"

[[checks]]
id = "firecracker.binary"
type = "command_exists"
command = "firecracker"
severity = "critical"

[[remediations]]
when = "kvm.device.failed"
title = "Enable KVM"
risk = "privileged"
commands = ["sudo modprobe kvm"]
manual_steps = [
  "Verify virtualization is enabled in BIOS/UEFI.",
  "Add your user to the kvm group if needed."
]
```

---

## Advice catalog design

Advice rules should match structured events, not raw strings only. Events can
include tool name, provider, error code, exit code, stderr, stdout patterns,
sandbox status, and capability.

### Example: dead container sync advice

```toml
[advice]
id = "advice.sync.dead_container"
default_enabled = true
audience = ["human", "agent"]

[match]
tool = "sandbox_sync"
error_code = "container_not_running"

[[actions]]
kind = "mcp_tool"
title = "Inspect sandbox state"
tool = "sandbox_info"
arguments = { sandbox_id = "{{ sandbox_id }}" }
risk = "safe"

[[actions]]
kind = "mcp_tool"
title = "Create a replacement sandbox"
tool = "sandbox_create"
arguments = { template = "{{ template | default('debian:bookworm-slim') }}" }
risk = "safe"
```

### Example: Maven missing POM advice

```toml
[advice]
id = "advice.maven.missing_pom"
default_enabled = true

[match]
tool = "sandbox_run_stream"
stderr_regex = "POM file .* does not exist"

[[actions]]
kind = "mcp_tool"
title = "List workspace files"
tool = "sandbox_list_files"
arguments = { sandbox_id = "{{ sandbox_id }}", path = "/workspace" }
risk = "safe"

[[actions]]
kind = "message"
title = "Use Maven -f with the detected pom.xml path"
message = "Run Maven with -f /workspace/<project>/pom.xml."
```

### Git-like configuration

```toml
[advice]
enabled = true

[advice.rules]
"advice.sync.dead_container" = true
"advice.maven.missing_pom" = true
"advice.provider.firecracker.kvm" = false
```

Environment override:

```bash
BASTION_ADVICE=0
```

MCP request override:

```json
{
  "advice": "off"
}
```

---

## Workflow catalog design

Workflows should be declarative, typed, and composable. They should not be only
named shell scripts. Each workflow declares inputs, preconditions, steps,
outputs, artifact detectors, failure advice, and examples.

### Example: `java.maven.package`

```toml
[workflow]
id = "java.maven.package"
version = "0.1.0"
description = "Build a Maven project and collect JAR artifacts."
requires = ["capability:jvm-build"]

[inputs]
sandbox_id = { type = "string", required = true }
project_dir = { type = "string", required = true, default = "/workspace" }
skip_tests = { type = "boolean", default = true }

[[preconditions]]
id = "pom.exists"
type = "file_exists"
path = "{{ project_dir }}/pom.xml"
on_failure_advice = ["advice.maven.missing_pom"]

[[steps]]
id = "prepareJvm"
action = "sandbox:prepare"
input = {
  sandbox_id = "{{ sandbox_id }}",
  capability = "jvm-build",
  strategy = "system_package"
}

[[steps]]
id = "package"
action = "sandbox:runStream"
input = {
  sandbox_id = "{{ sandbox_id }}",
  env_ref = "{{ steps.prepareJvm.output.env_ref }}",
  command = "mvn package {{ '-DskipTests' if skip_tests }} -f {{ project_dir }}/pom.xml"
}
success_patterns = ["maven.build_success"]
failure_patterns = ["maven.build_failure", "maven.missing_pom"]

[[outputs]]
id = "jar_artifacts"
type = "artifact_set"
detector = "java.jar"
path = "{{ project_dir }}/target/*.jar"

[[next_recommended]]
when = "success"
tool = "sandbox_sync"
arguments = {
  sandbox_id = "{{ sandbox_id }}",
  mode = "pull",
  source = "{{ project_dir }}/target/",
  target = "./bastion-artifacts/{{ workflow_run_id }}/"
}
```

### Workflow design rules

- Workflows must declare inputs with types.
- Workflows must declare required capabilities.
- Workflows may call other workflows, but cycles are rejected.
- Workflows must support dry-run validation.
- Workflow steps should reference action IDs, not Rust function names.
- Outputs should be typed and usable by later steps.
- Workflows should include examples for discovery and testing.

---

## Pattern catalog design

Log and output pattern detectors should also be catalogs. This avoids hardcoding
`BUILD SUCCESS` or npm-specific messages.

### Example: Maven patterns

```toml
[patterns]
id = "maven"

[[pattern]]
id = "maven.build_success"
regex = "BUILD SUCCESS"
severity = "info"

[[pattern]]
id = "maven.build_failure"
regex = "BUILD FAILURE"
severity = "error"

[[pattern]]
id = "maven.missing_pom"
regex = "POM file .* does not exist"
severity = "error"
advice = ["advice.maven.missing_pom"]
```

---

## Artifact detector catalog design

Artifact detection should not be hardcoded per workflow. Workflows can refer to
artifact detectors.

### Example: Java JAR detector

```toml
[artifact_detector]
id = "java.jar"
description = "Detect Maven/Gradle JAR artifacts."

[[rules]]
glob = "target/*.jar"
exclude = ["*-sources.jar", "*-javadoc.jar"]
kind = "java.jar"
metadata = {
  build_tool = "maven_or_gradle"
}
```

---

## Plugin levels

Use multiple extension levels instead of jumping straight to executable plugins.

### Level 1: Declarative catalogs

Safe by default. TOML/YAML descriptors define checks, advice, workflows,
patterns, and artifact detectors.

### Level 2: Scripted actions

Catalogs can reference scripts, but only from trusted catalogs and with explicit
permissions.

```toml
[action]
id = "acme:license:scan"
kind = "script"
interpreter = "bash"
script = "scripts/license-scan.sh"
permissions = ["read_workspace"]
```

### Level 3: WASM plugins

Portable executable plugins running in a constrained WASI sandbox. Good for
community extension without native code loading.

### Level 4: Native Rust plugins

Most powerful and most risky. Should be optional and signed.

---

## MCP tools for catalog-driven Bastion

Suggested MCP surface:

| Tool | Purpose |
|------|---------|
| `catalog_list` | List loaded catalogs and versions |
| `catalog_install` | Install a catalog from Git/path/URL |
| `catalog_validate` | Validate schemas and trust policy |
| `sandbox_doctor` | Run provider or environment diagnostics |
| `advice_list` | List available advice rules |
| `advice_configure` | Enable/disable advice rules |
| `workflow_list` | List available workflows |
| `workflow_describe` | Show inputs, outputs, examples, and risks |
| `workflow_dry_run` | Expand and validate a workflow without execution |
| `workflow_run` | Execute a workflow and emit structured evidence |
| `patterns_list` | List available pattern detectors |

---

## Trust and supply-chain model

Community catalogs are powerful. Bastion should treat them like code-adjacent
artifacts.

Recommended trust controls:

1. Prefer pinned Git refs by SHA.
2. Support catalog signatures.
3. Show catalog source and version in every workflow run.
4. Require explicit opt-in for shell/scripted/native actions.
5. Separate safe MCP actions from privileged host commands.
6. Allow organizations to maintain allowlists and blocklists.
7. Record catalog ID and version in evidence output.

---

## Proposed core Rust abstractions

### Catalog registry

```rust
pub trait CatalogSource {
    async fn load(&self) -> Result<CatalogBundle, CatalogError>;
}

pub struct CatalogRegistry {
    doctors: DoctorRegistry,
    advice: AdviceRegistry,
    workflows: WorkflowRegistry,
    patterns: PatternRegistry,
    artifacts: ArtifactDetectorRegistry,
}
```

### Action executor

```rust
pub trait ActionExecutor {
    async fn execute(
        &self,
        action_id: &ActionId,
        input: serde_json::Value,
        ctx: ExecutionContext,
    ) -> Result<ActionOutput, ActionError>;
}
```

### Advice engine

```rust
pub trait AdviceEngine {
    fn evaluate(&self, event: &ExecutionEvent, ctx: &AdviceContext) -> Vec<SuggestedAction>;
}
```

### Workflow engine

```rust
pub trait WorkflowEngine {
    fn dry_run(&self, workflow: &WorkflowId, input: Value) -> Result<WorkflowPlan, WorkflowError>;
    async fn run(&self, workflow: &WorkflowId, input: Value) -> Result<WorkflowRunResult, WorkflowError>;
}
```

### Doctor engine

```rust
pub trait DoctorEngine {
    async fn run(&self, target: DoctorTarget) -> Result<DoctorReport, DoctorError>;
}
```

---

## Recommended implementation phases

### Phase A: Catalog foundation

- Define schemas for catalog manifest, advice, patterns, and artifacts.
- Load built-in + project catalogs.
- Add schema validation and deterministic override rules.

### Phase B: Advice engine first

- Implement declarative advice rules.
- Add `next_recommended` using rules, not hardcoded branches.
- Add Git-like config: `advice.*` and `BASTION_ADVICE=0`.

### Phase C: Doctor catalogs

- Implement generic check primitives.
- Add provider doctor catalog entries.
- Expose `sandbox_doctor`.

### Phase D: Pattern and artifact catalogs

- Add pattern matching over stdout/stderr/logs.
- Add artifact detection rules.
- Use these in PetClinic validation.

### Phase E: Workflow engine

- Implement `workflow_dry_run` and `workflow_run`.
- Add `java.maven.package` as the first workflow.
- Keep shell commands inside descriptors, not Rust code.

### Phase F: Community catalogs

- Add `catalog_install` from Git URL.
- Add pinning, metadata, signatures, examples, and trust policy.

---

## Key recommendation

Do **not** implement `sandbox_doctor`, `next_recommended`, or Maven workflows as
hardcoded product logic. Implement them as the first consumers of a catalog
runtime:

```text
Catalog runtime first, Maven/Podman catalogs second.
```

This keeps Bastion flexible for AI agents, user customization, and future
community contributions.

---

## Agent-first self-improvement loop

Bastion is an MCP gateway for AI agents, so the catalog system should not only
be user-extensible. It should be **agent-improvable**. Agents should be able to
turn execution experience into proposed catalog improvements, while Bastion
keeps safety, validation, provenance, and user approval in the loop.

The key idea is:

```text
Experience → Evidence → Hypothesis → Catalog proposal → Dry-run → Review → Install
```

Agents should help users by noticing repeated patterns, failed commands,
missing doctor checks, useful remediation steps, and successful workflows. They
should then propose catalog entries that encode that knowledge for future runs.

### Why this matters

Without self-improvement, every agent repeats the same learning process:

1. Try a command.
2. Observe an error.
3. Search logs.
4. Discover the fix.
5. Apply the fix manually.
6. Lose the learning after the session.

With self-improvement, the agent can convert the learning into catalog entries:

- An advice rule.
- A doctor check.
- A workflow precondition.
- A log pattern detector.
- An artifact detector.
- A reusable workflow.

This makes Bastion progressively better at helping future users and agents.

---

## Self-improvement primitives

### 1. Experience records

Every meaningful tool execution should produce an experience record. This is
not necessarily a full log; it is a structured summary of what happened.

```json
{
  "experience_id": "exp_01H...",
  "trace_id": "petclinic-fase013",
  "tool": "sandbox_run_stream",
  "provider": "podman",
  "sandbox_id": "c20c2f09",
  "inputs": {
    "command": "mvn package -DskipTests -f /workspace/petclinic/pom.xml"
  },
  "result": {
    "exit_code": 0,
    "duration_ms": 70130
  },
  "patterns": ["maven.build_success"],
  "artifacts": [
    {
      "kind": "java.jar",
      "path": "/workspace/petclinic/target/spring-petclinic-4.0.0-SNAPSHOT.jar",
      "size_bytes": 70254592
    }
  ]
}
```

Failed experiences should capture error context and candidate evidence:

```json
{
  "experience_id": "exp_01H...",
  "tool": "sandbox_sync",
  "provider": "podman",
  "result": {
    "error_code": "sync_failed",
    "message": "Sync failed: "
  },
  "evidence": [
    "container was killed by sandbox_cancel before sync",
    "stderr was empty",
    "provider.is_alive would have detected the dead container"
  ],
  "candidate_improvements": [
    "Add advice.sync.dead_container",
    "Add precondition provider.is_alive before sandbox_sync",
    "Improve empty stderr error formatting"
  ]
}
```

### 2. Improvement candidates

Agents should not install catalog changes directly. They should create
`ImprovementCandidate` records with evidence, confidence, scope, and risk.

```json
{
  "candidate_id": "cand_01H...",
  "kind": "advice_rule",
  "title": "Suggest sandbox_info when sync fails because container is dead",
  "derived_from": ["exp_01H..."],
  "confidence": 0.86,
  "risk": "safe",
  "scope": "project",
  "proposed_catalog_entry": {
    "advice": {
      "id": "advice.sync.dead_container",
      "match": {
        "tool": "sandbox_sync",
        "error_code": "container_not_running"
      },
      "actions": [
        {
          "kind": "mcp_tool",
          "tool": "sandbox_info",
          "arguments": { "sandbox_id": "{{ sandbox_id }}" }
        }
      ]
    }
  }
}
```

### 3. Catalog proposals

Catalog proposals are patch-like objects. They can be reviewed, diffed,
validated, and applied.

```json
{
  "proposal_id": "prop_01H...",
  "target_catalog": ".bastion/catalogs/advice/sync.advice.toml",
  "operation": "add_or_update",
  "entry_id": "advice.sync.dead_container",
  "rationale": "Observed sandbox_sync returning an empty error after the container was killed.",
  "evidence_refs": ["exp_01H..."],
  "diff_preview": "...",
  "requires_user_approval": true
}
```

### 4. Dry-run and validation

Before a proposal can be installed, Bastion should validate it:

- Schema validity.
- ID collision and override behavior.
- Template variables are defined.
- Referenced tools exist.
- Referenced advice/pattern/artifact IDs exist.
- Risk policy allows the proposed actions.
- Dry-run can match at least one saved experience.

This makes catalog self-improvement safe and auditable.

---

## MCP tools for agent-assisted self-improvement

The following tools would make Bastion agent-first without giving agents unsafe
direct write privileges.

| Tool | Purpose |
|------|---------|
| `experience_list` | List recent structured execution experiences |
| `experience_get` | Get one experience with evidence and logs summary |
| `experience_summarize` | Compress a run into lessons learned |
| `improvement_suggest` | Ask Bastion to generate candidate catalog improvements from experiences |
| `improvement_validate` | Validate candidate schema, references, and safety |
| `improvement_dry_run` | Test a candidate against saved experiences |
| `improvement_apply` | Apply approved candidate to project/user catalog |
| `catalog_diff` | Show catalog changes before applying |
| `catalog_test` | Run catalog examples and regression fixtures |
| `catalog_publish` | Package and publish a catalog for sharing |

Example agent workflow:

```text
1. Run PetClinic workflow.
2. Observe sandbox_sync failure.
3. Call experience_summarize(trace_id="petclinic-fase013").
4. Call improvement_suggest(kind="advice_rule").
5. Bastion proposes advice.sync.dead_container.
6. Agent calls improvement_validate.
7. Agent calls improvement_dry_run against the failing trace.
8. User approves.
9. Agent calls improvement_apply(scope="project").
10. Future sync failures now include the advice automatically.
```

---

## Experience-to-catalog mapping

Agents need clear guidance on what type of catalog entry to propose.

| Experience pattern | Best catalog improvement |
|--------------------|--------------------------|
| Same error appears repeatedly | Advice rule |
| Missing executable, socket, env var, permission | Doctor check |
| Successful command sequence repeated | Workflow definition |
| Repeated stdout/stderr phrase predicts outcome | Pattern detector |
| Repeated output file shape | Artifact detector |
| Manual remediation command works | Remediation action |
| Expensive setup repeated | Cache/materialization hint |
| Provider-specific failure | Provider doctor or provider advice |

Examples from recent PetClinic validation:

| Observation | Proposed catalog entry |
|-------------|------------------------|
| `POM file ... does not exist` | `advice.maven.missing_pom` |
| `BUILD SUCCESS` marks Maven success | `pattern.maven.build_success` |
| JAR appears in `target/*.jar` | `artifact_detector.java.jar` |
| Podman socket required | `doctor.provider.podman.socket` |
| `sandbox_sync` after SIGKILL fails | `advice.sync.dead_container` |

---

## Agent roles in catalog improvement

Different agents can participate at different confidence levels.

### Observer agent

Collects execution evidence and writes summaries. It cannot propose catalog
patches.

### Research agent

Searches documentation or previous experiences and proposes improvement
candidates with references.

### Catalog author agent

Creates valid catalog descriptors from evidence and schemas.

### Reviewer agent

Checks whether the proposed catalog entry is too specific, unsafe, duplicative,
or likely to overfit one run.

### Maintainer/user

Approves installation, publication, or rejection.

This keeps agency high but preserves user control.

---

## Anti-overfitting rules

Agents will be tempted to encode one-off fixes. Bastion should help avoid this.

Recommended guardrails:

1. Require evidence references for every proposed catalog entry.
2. Require a scope: `session`, `project`, `user`, `organization`, or
   `community`.
3. Mark low-confidence proposals as `experimental`.
4. Prefer narrow match conditions over broad regexes.
5. Require dry-run against positive and negative examples.
6. Warn if a rule matches too many unrelated experiences.
7. Prefer adding pattern/advice before adding workflow automation.
8. Never auto-apply privileged remediation commands.

---

## Catalog memory model

Bastion should store self-improvement state in three layers:

### 1. Experience store

Short-to-medium term execution evidence. This can be SQLite-backed locally and
exportable for sharing.

### 2. Candidate store

Pending improvements. Candidates can be accepted, rejected, superseded, or
promoted.

### 3. Catalog store

Accepted, versioned, validated knowledge. This is what future executions use.

```text
experience_store → candidate_store → catalog_store
```

Engram can complement this by storing session-level lessons, but Bastion should
not depend on Engram for its product-level catalog memory.

---

## Autoresearch

Some improvements need external knowledge. Bastion can support autoresearch
without baking a web search provider into the core.

Recommended design:

```toml
[research]
enabled = true
providers = ["agent"]

[[research.questions]]
id = "provider.gvisor.install"
prompt = "Find official installation instructions for gVisor runsc on this OS."
allowed_domains = ["gvisor.dev", "github.com/google/gvisor"]
```

The agent performs research and submits a `ResearchNote`:

```json
{
  "question_id": "provider.gvisor.install",
  "summary": "gVisor requires runsc installed and configured as an OCI runtime.",
  "sources": ["https://gvisor.dev/docs/user_guide/install/"],
  "candidate_improvements": ["doctor.provider.gvisor.runsc"]
}
```

Bastion validates the proposed catalog entry but does not need to own the web
search integration.

---

## User and agent customization

Users and agents should be able to create local catalogs incrementally.

### User-level override

```toml
# ~/.bastion/catalogs/advice/local-overrides.toml
[advice.rules]
"advice.maven.missing_pom" = false
"advice.sync.dead_container" = true
```

### Agent-created session catalog

```text
.bastion/catalogs/.agent/session-2026-05-05/
├── advice/generated.toml
├── patterns/generated.toml
└── proposals.jsonl
```

Session catalogs are temporary until accepted by the user.

---

## Recommended implementation path

### Phase 1: Experience capture

- Add `trace_id` and structured `ExperienceRecord` for every tool call.
- Store experiences in SQLite.
- Expose `experience_list` and `experience_get`.

### Phase 2: Catalog proposal format

- Define `ImprovementCandidate` and `CatalogProposal` schemas.
- Add `improvement_validate` and `catalog_diff`.

### Phase 3: Advice self-improvement

- Implement `improvement_suggest(kind="advice_rule")`.
- Support dry-run against saved experiences.
- Let users apply accepted advice to project catalog.

### Phase 4: Pattern and artifact learning

- Let agents propose pattern detectors from stdout/stderr.
- Let agents propose artifact detectors from repeated file outputs.

### Phase 5: Workflow learning

- Detect repeated successful command sequences.
- Propose workflow descriptors.
- Require stricter review and dry-run before install.

### Phase 6: Community sharing

- Package accepted catalogs.
- Add catalog metadata, examples, tests, and signatures.
- Publish to Git or a future Bastion catalog index.

---

## Assertion catalogs as validation primitives

Testing libraries in Rust provide a useful design inspiration for Bastion, but
Bastion should not simply embed test-only assertions. Instead, Bastion should
define a catalogable **assertion layer**: reusable validation primitives that
can verify provider readiness, workflow success, artifact existence, command
effects, and feature behavior.

This would let users and agents express expectations declaratively:

```text
Run an action → collect evidence → evaluate assertions → produce diagnosis
```

### Why assertions help Bastion

Assertions give Bastion a common validation language across several features:

- Doctor checks can assert host capabilities.
- Workflows can assert preconditions and postconditions.
- Advice rules can assert that a suggested fix actually worked.
- Catalog proposals can include regression fixtures.
- Agents can validate hypotheses before promoting catalog improvements.
- E2E scenarios like PetClinic can be stored as reproducible validation suites.

This shifts Bastion from “command executed” to “capability verified.”

### Rust testing inspiration

Rust testing libraries offer useful patterns:

| Rust ecosystem pattern | Bastion adaptation |
|------------------------|-------------------|
| `assert_eq!`, `assert!` | Basic equality, existence, and boolean assertions |
| `assert_cmd` | Validate command exit code, stdout, stderr, timeout |
| `predicates` | Composable conditions over strings, paths, JSON, logs |
| `insta` snapshots | Snapshot expected tool responses or doctor reports |
| `proptest` | Generate variations of inputs for catalog validation |
| `trycmd` | File-based command examples as executable documentation |
| `rstest` | Parameterized validation cases for providers/workflows |

Bastion does not need to expose these exact crates to users, but its validation
DSL can borrow their concepts.

### Assertion descriptor example

Assertions should live in catalogs and be referenced by doctors, workflows,
advice, and tests.

```toml
[assertion]
id = "maven.build.succeeds"
description = "Maven package command completed successfully."

[[checks]]
type = "exit_code"
equals = 0

[[checks]]
type = "stdout_contains"
value = "BUILD SUCCESS"

[[checks]]
type = "file_exists"
path = "{{ project_dir }}/target/*.jar"
```

Another example for provider readiness:

```toml
[assertion]
id = "provider.podman.ready"

[[checks]]
type = "command_exists"
command = "podman"

[[checks]]
type = "unix_socket_exists"
path = "/run/user/{{ uid }}/podman/podman.sock"

[[checks]]
type = "command_success"
command = "podman info --format json"
timeout_ms = 5000
```

### Assertion result model

Assertion results should be structured, not just pass/fail text.

```json
{
  "assertion_id": "maven.build.succeeds",
  "status": "failed",
  "checks": [
    {
      "type": "exit_code",
      "expected": 0,
      "actual": 1,
      "status": "failed"
    },
    {
      "type": "stdout_contains",
      "expected": "BUILD SUCCESS",
      "status": "failed"
    }
  ],
  "evidence_refs": ["exp_01H..."],
  "suggested_advice": ["advice.maven.missing_pom"]
}
```

### Assertion scopes

Assertions should support multiple scopes:

| Scope | Example |
|-------|---------|
| Host | `podman` binary exists |
| Provider | Podman API ping succeeds |
| Sandbox | `/workspace` exists and is writable |
| Command | Exit code is 0 and stdout contains expected pattern |
| Artifact | JAR exists and is larger than 1MB |
| Log | No `ERROR` pattern appeared during workflow |
| Workflow | All required steps completed and outputs exist |
| Catalog | Example fixture still passes |

### Assertion packs

Assertions should be grouped into packs that catalogs can share.

```text
.bastion/catalogs/assertions/
├── core.assertions.toml
├── providers.assertions.toml
├── maven.assertions.toml
├── npm.assertions.toml
└── python.assertions.toml
```

Example pack:

```toml
[assertion_pack]
id = "bastion.maven"
version = "0.1.0"

assertions = [
  "maven.pom.exists",
  "maven.build.succeeds",
  "maven.jar.exists",
  "maven.surefire.reports.exist"
]
```

### Workflow integration

Workflows can reference assertions directly:

```toml
[[steps]]
id = "package"
action = "sandbox:runStream"
input = {
  command = "mvn package -DskipTests -f {{ project_dir }}/pom.xml"
}

assertions = [
  "command.exit_code.zero",
  "maven.build.succeeds",
  "maven.jar.exists"
]
```

This makes success criteria explicit and configurable. If a user wants a
different standard, they override the assertions without changing Bastion code.

### Doctor integration

Doctor checks can be assertions too. A doctor report becomes the result of
running an assertion pack against the host and provider environment.

```toml
[doctor]
id = "provider.podman"
assertions = ["provider.podman.ready"]
```

### Advice integration

Advice can be triggered by failed assertions instead of brittle string matches.

```toml
[advice]
id = "advice.maven.missing_pom"

[match]
failed_assertion = "maven.pom.exists"
```

### Self-improvement integration

Agents can propose new assertions from experience:

| Experience | Proposed assertion |
|------------|--------------------|
| Build success always includes `BUILD SUCCESS` | `maven.build.succeeds` |
| JAR produced in `target/*.jar` | `maven.jar.exists` |
| Sync fails if container is dead | `sandbox.container.alive_before_sync` |
| Local provider needs env var | `provider.local.env_enabled` |

Assertions are often safer first contributions than full workflows. An agent
can propose an assertion to validate a known behavior before proposing a
workflow that depends on it.

### MCP tools for assertions

Suggested tools:

| Tool | Purpose |
|------|---------|
| `assertion_list` | List available catalog assertions |
| `assertion_describe` | Show checks, inputs, examples, and risk |
| `assertion_run` | Run an assertion or assertion pack |
| `assertion_dry_run` | Evaluate against saved experience without executing commands |
| `assertion_suggest` | Propose assertions from experiences |
| `assertion_validate` | Validate assertion schema and references |

### Benefits

Assertion catalogs would give Bastion:

1. A shared validation language across doctor, workflows, advice, and tests.
2. Better agent feedback: “this assertion failed” is more actionable than raw
   stderr.
3. Safer self-improvement: agents can propose small assertions before larger
   workflows.
4. Reusable community validation packs.
5. Executable documentation for catalog behavior.
6. Stronger regression protection for MCP tools and provider features.

### Recommendation

Add assertions as a first-class catalog type before implementing a full workflow
engine. The suggested order becomes:

```text
experience records → assertions → advice → doctor → patterns/artifacts → workflows
```

This lets Bastion validate what it observes before it automates what it learns.

---

## Key recommendation for self-improvement

Bastion should treat AI agents as **catalog contributors**, not just command
callers. But agents should contribute through a controlled pipeline:

```text
observe → propose → validate → dry-run → user approve → install → publish
```

This lets agents help users and future agents without turning every session
learning into unsafe, hardcoded, or overfitted behavior.
