# Bastion functional improvements action plan

> **Date:** 2026-05-05  
> **Source:** PetClinic E2E Fase 013, high-performance MCP work, and recent design discussions  
> **Status:** Proposed roadmap  
> **Primary goal:** Turn Bastion from a command executor into a guided,
> observable, provider-aware workflow platform for AI agents and CI/CD-style
> pipelines.

## Executive summary

Bastion now proves the core execution path: a client calls MCP tools, the
gateway resolves a provider, and the provider launches real sandbox work. The
latest PetClinic validation reached a working Java/Maven build with a pooled
sandbox in **0.01s startup time**, **70s Maven build time**, and **141s total
pipeline time**. The test also exposed several product-level gaps:

- Missing or regressed MCP tools are not caught automatically.
- Provider availability is not diagnosed in a user-friendly way.
- Error responses lack actionable remediation.
- Logs contain useful evidence, but filtering and correlation are ad hoc.
- Workflows such as Maven builds are still expressed as raw shell commands.

This plan groups the proposed improvements into phases that compound: first
stabilize regressions and errors, then add diagnostics and suggestions, then
introduce workflow intelligence.

---

## Current evidence

### PetClinic Fase 013 results

| Capability | Evidence | Status |
|------------|----------|--------|
| MCP execution path | All test actions used MCP `tools/call` | ✅ |
| Provider path | MCP → gateway → `PodmanProvider` → Podman API | ✅ |
| Pooling | `sandbox_create` from pool in **0.01s** | ✅ |
| Long-running command | Maven build ran **70s** without disconnect | ✅ |
| Artifact creation | 67MB PetClinic JAR produced | ✅ |
| Cancellation | `sleep 300` stopped via SIGTERM → SIGKILL | ✅ |
| Sync after cancel | Failed because the container was killed | ⚠️ |

### Bugs and findings from recent validation

| ID | Finding | Severity | Proposed area |
|----|---------|----------|---------------|
| F-024 | `sandbox_run` was accidentally removed from MCP tool registry | Critical | Regression tests |
| F-025 | `sandbox_sync` returns `Sync failed: ` with empty error on dead container | Medium | Error handling |
| F-026 | Pool stats log every 5s at `DEBUG` and create noise | Low | Observability |
| F-027 | No `doctor` command exists for provider readiness | High | Diagnostics |
| F-028 | No central next-action suggestions like Git | Medium | Agent ergonomics |
| F-029 | Maven/npm workflows are raw shell commands, not typed workflows | Medium | Workflow engine |

---

## Improvement themes

### 1. Reliability and regression protection

The gateway must prevent accidental removal of important MCP tools and detect
broken states before running expensive operations.

**Proposals:**

1. Add a tool registry regression test.
2. Check sandbox health before `sandbox_sync`.
3. Return clear errors for dead or missing containers.
4. Add provider-specific preflight checks before creating sandboxes.

### 2. Provider diagnostics: `sandbox_doctor`

Provider readiness should be explicit. A user or agent should be able to ask:

```json
{
  "tool": "sandbox_doctor",
  "arguments": { "provider": "firecracker" }
}
```

And receive:

```json
{
  "provider": "firecracker",
  "status": "unavailable",
  "missing": ["/dev/kvm", "firecracker binary"],
  "suggestions": [
    "sudo modprobe kvm",
    "Install firecracker from the official release page",
    "Run bastion-gateway --provider=firecracker after setup"
  ]
}
```

**Provider checks to support:**

| Provider | Checks |
|----------|--------|
| `podman` | socket path, API ping, image availability, worker binary path |
| `local` | `DANGEROUS_ALLOW_LOCAL=1`, base directory writable |
| `wasm` | feature enabled, wasmtime availability, WASI support |
| `gvisor` | `runsc` binary, runtime configuration, image compatibility |
| `firecracker` | `/dev/kvm`, firecracker binary, kernel/rootfs availability |

### 3. Git-like next recommended actions

Bastion responses should guide agents toward the next valid action.

Example after `sandbox_prepare`:

```json
{
  "status": "prepared",
  "env_ref": "registry:...:jvm-build",
  "next_recommended": [
    {
      "tool": "sandbox_run_stream",
      "arguments": {
        "sandbox_id": "...",
        "command": "mvn package -DskipTests -f /workspace/pom.xml",
        "env_ref": "registry:...:jvm-build"
      },
      "reason": "JVM build environment is ready. Maven is available."
    }
  ]
}
```

Example after `sandbox_sync` fails on a stopped container:

```json
{
  "error": "Container is not running. Cannot sync files.",
  "next_recommended": [
    {
      "tool": "sandbox_info",
      "arguments": { "sandbox_id": "..." },
      "reason": "Confirm sandbox lifecycle state."
    },
    {
      "tool": "sandbox_create",
      "arguments": { "template": "debian:bookworm-slim" },
      "reason": "Create a new sandbox if the previous one was killed."
    }
  ]
}
```

### 4. Observability and log control

Evidence collection works, but it currently depends on manual `grep` and log
inspection. Bastion should make traceability first-class.

**Proposals:**

1. Add CLI flags:
   - `--log-filter`
   - `--log-json`
   - `--log-file`
2. Add optional `trace_id` to command-like MCP tools:
   - `sandbox_run`
   - `sandbox_run_stream`
   - `sandbox_prepare`
   - `sandbox_sync`
3. Wrap each tool call in a `tracing::span!` containing:
   - `trace_id`
   - `sandbox_id`
   - `provider`
   - `tool`
   - `command` where applicable
4. Move stable pool stats from `DEBUG` to `TRACE`, or log only on state change.
5. Add a log analysis helper script:

```bash
scripts/analyze-gateway-log.py \
  --file /tmp/bastion-gw.log \
  --sandbox-id c20c2f09 \
  --patterns "ERROR,WARN,SIGKILL,BUILD FAILURE,Sync failed"
```

### 5. Workflow intelligence

Raw commands are flexible, but common build systems encode conventions. Bastion
can help agents by modeling these conventions as workflows.

Example workflow call:

```json
{
  "tool": "sandbox_workflow_run",
  "arguments": {
    "workflow": "java.maven.package",
    "sandbox_id": "...",
    "project_dir": "/workspace/petclinic",
    "skip_tests": true
  }
}
```

The workflow engine would expand this into:

1. Check `/workspace/petclinic/pom.xml` exists.
2. Prepare `jvm-build` if needed.
3. Inject the right `env_ref`.
4. Run `mvn package -DskipTests -f /workspace/petclinic/pom.xml`.
5. Detect `BUILD SUCCESS` or `BUILD FAILURE`.
6. Find artifacts in `target/*.jar`.
7. Suggest `sandbox_sync pull`.

Candidate workflow registry entries:

| Workflow | Purpose |
|----------|---------|
| `java.maven.package` | Build Maven project and collect JARs |
| `java.maven.test` | Run Maven tests and parse Surefire output |
| `node.npm.build` | Install dependencies and run npm build |
| `node.npm.test` | Run npm tests and parse common failures |
| `python.pytest` | Run pytest and collect reports |

---

## Roadmap

### Phase 0 — Immediate stabilization

**Goal:** Prevent repeated regressions and improve current error quality.

| Task | Description | Priority |
|------|-------------|----------|
| P0.1 | Add `tools/list` regression test for required MCP tools | Critical |
| P0.2 | Fix `sandbox_sync` empty error message | High |
| P0.3 | Check `provider.is_alive()` before sync | High |
| P0.4 | Reduce pool stats log noise | Medium |
| P0.5 | Re-run PetClinic sync before cancel to validate live-container sync | Medium |

**Exit criteria:**

- Test fails if `sandbox_run`, `sandbox_run_stream`, or `sandbox_cancel` vanish.
- Dead-container sync returns a clear, actionable error.
- Pool logs are useful but not noisy.

### Phase 1 — Provider doctor

**Goal:** Let users and agents diagnose provider readiness before they fail.

| Task | Description | Priority |
|------|-------------|----------|
| P1.1 | Add domain model for doctor findings and suggestions | High |
| P1.2 | Add `ProviderDoctor` trait or equivalent diagnostic interface | High |
| P1.3 | Implement Podman doctor | High |
| P1.4 | Implement Local doctor | High |
| P1.5 | Add partial doctors for Wasm, gVisor, and Firecracker | Medium |
| P1.6 | Expose `sandbox_doctor` MCP tool | High |
| P1.7 | Add CLI `bastion-gateway doctor --provider <name>` if CLI mode fits | Medium |

**Exit criteria:**

- `sandbox_doctor` explains why each provider is available or unavailable.
- Each finding includes evidence and next actions.

### Phase 2 — Actionable suggestions

**Goal:** Make MCP responses self-guiding for agents.

| Task | Description | Priority |
|------|-------------|----------|
| P2.1 | Define `SuggestedAction` response schema | High |
| P2.2 | Add `next_recommended` to success responses | High |
| P2.3 | Add `next_recommended` to error responses | High |
| P2.4 | Add suggestions for image not found, missing file, dead container, and missing capability | High |
| P2.5 | Add suggestions after `sandbox_prepare` for Maven/npm/pytest commands | Medium |

**Exit criteria:**

- Common failures include at least one executable next action.
- Success states guide the user toward the next meaningful workflow step.

### Phase 3 — Observability controls

**Goal:** Make evidence collection systematic and machine-readable.

| Task | Description | Priority |
|------|-------------|----------|
| P3.1 | Add `--log-filter` CLI flag | High |
| P3.2 | Add `--log-json` CLI flag | High |
| P3.3 | Add `--log-file` CLI flag | Medium |
| P3.4 | Add optional `trace_id` to command-like MCP params | High |
| P3.5 | Wrap tool execution in structured spans | High |
| P3.6 | Add `scripts/analyze-gateway-log.py` | Medium |

**Exit criteria:**

- A test run can be reconstructed by `trace_id`.
- Logs can be filtered by sandbox, provider, tool, severity, and known patterns.

### Phase 4 — Workflow registry

**Goal:** Encode conventional build workflows as first-class Bastion operations.

| Task | Description | Priority |
|------|-------------|----------|
| P4.1 | Define workflow descriptor schema | High |
| P4.2 | Add `sandbox_workflow_list` | Medium |
| P4.3 | Add `sandbox_workflow_run` | High |
| P4.4 | Implement `java.maven.package` workflow | High |
| P4.5 | Implement artifact detection for Maven JARs | High |
| P4.6 | Add `node.npm.build` workflow | Medium |
| P4.7 | Add workflow-level `next_recommended` | Medium |

**Exit criteria:**

- PetClinic can be built with one workflow call after sandbox creation.
- The workflow emits structured step evidence and artifact suggestions.

### Phase 5 — Advanced cancellation and lifecycle safety

**Goal:** Make command cancellation precise and avoid damaging reusable pooled
sandboxes unnecessarily.

| Task | Description | Priority |
|------|-------------|----------|
| P5.1 | Track provider-level command handles where possible | Medium |
| P5.2 | Cancel Podman exec process instead of killing whole container | High |
| P5.3 | Return `cancelled`, `no_running_command`, `container_killed`, or `unsupported` clearly | High |
| P5.4 | Mark killed pooled sandboxes as non-reusable | High |
| P5.5 | Add lifecycle tests for cancel → sync → terminate | Medium |

**Exit criteria:**

- Cancelling a command does not kill the whole sandbox unless necessary.
- Pooled sandboxes are not returned to the pool after destructive cancellation.

---

## Recommended implementation order

1. **Phase 0** — protect the existing gateway from repeated regressions.
2. **Phase 1** — add `sandbox_doctor` so provider availability becomes explicit.
3. **Phase 3** — add log controls and trace IDs before large workflow work.
4. **Phase 2** — add Git-like suggestions once doctor and errors are structured.
5. **Phase 4** — add Maven workflow automation.
6. **Phase 5** — refine cancellation semantics and pool safety.

This order keeps the system stable while gradually increasing product
intelligence.

---

## Suggested SDD changes

To implement this roadmap cleanly, split work into separate SDD changes:

1. `stabilize-mcp-tools-and-sync-errors`
2. `provider-doctor-diagnostics`
3. `observability-log-controls`
4. `git-like-next-actions`
5. `workflow-registry-maven-first`
6. `safe-command-cancellation`

Each change should include a verification scenario based on the PetClinic E2E
test so improvements remain grounded in real pipeline behavior.
