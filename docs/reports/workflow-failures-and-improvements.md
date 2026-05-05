# Workflow Failures & Improvement Proposals

> **Date:** 2026-05-04 | **Context:** PetClinic build tests across multiple approaches (A-G) | **Version:** Bastion v0.1.0  
> **Re-run:** Fresh start at 16:06 UTC after gateway restart — all data in-situ

---

## 1. Executive Summary

During end-to-end testing of the Bastion MCP Gateway and its toolchain capabilities (sandbox_prepare, artifact catalog, provider comparison, snapshot), we encountered **15 distinct failure modes** across 5 categories:

| Category | Count | Severity |
|----------|-------|----------|
| MCP Connectivity | 3 | HIGH |
| Bugs (functional) | 6 | HIGH |
| Timeouts | 2 | MEDIUM |
| Image/Template Resolution | 1 | LOW |
| User/API Ergonomics | 3 | LOW |

---

## 2. Test Results Summary (Fresh Re-run)

| Enfoque | Método | Status | Tiempo | JAR | Notas |
|---------|--------|--------|--------|-----|-------|
| **A: Manual asdf** | git clone asdf → install java+maven → git clone petclinic → mvn package | ✅ | **137s** | 67MB | asdf clone 1.7s, java install 32s, maven 33s, clone+build 56s |
| **B: ToolResolver** | sandbox_prepare(jvm-build) + sandbox_run(env_ref) | ✅ | **40s** | 67MB | prep 18s (apt), clone 7s, build 15s |
| **C: ArtifactCatalog** | register_artifact + sandbox_prepare | ❌ N/A | — | — | Requiere pre-built container image digest |
| **D: Firecracker** | MicroVM + jvm-build | ❌ | — | — | Gateway solo tiene provider podman |
| **E: gVisor** | runsc + jvm-build | ❌ | — | — | Gateway solo tiene provider podman |
| **F: sandbox_sync** | push → build → pull | ⚠️ | 2s+2s+12s | — | sandbox_write→run funciona; sandbox_read roto |
| **G: Snapshot** | create → restore → verify | ⚠️ | 13s create | 67MB | create/restore ok; list roto; restore no registra en MCP |

### Comparative Performance

| Métrica | Enfoque A (asdf) | Enfoque B (ToolResolver) | Delta |
|---------|--------------|----------------------|-------|
| Setup time | 67s | 18s | **3.7x faster** |
| Build time | 56s | 22s (clone+build) | **2.5x faster** |
| **Total** | **137s** | **40s** | **3.4x faster** |
| Java version | 17.0.8 (Temurin) | 17.0.19 (Debian) | Slightly newer |
| Maven version | 3.9.5 | 3.8.7 | Older (distro) |
| JAR size | 67MB | 67MB | Identical |
| Spring Boot | 4.0.0-SNAPSHOT | 4.0.0-SNAPSHOT | Identical |

---

## 3. Detailed Failure Analysis

### F-001: MCP "Not Connected" After Gateway Crash

**Severity:** 🔴 HIGH  
**Category:** MCP Connectivity  
**Observed:** Bastion MCP tools (`bastion_sandbox_*`) return "Not connected" after gateway process termination.  
**Reproduction:**
1. Gateway crashed during judgment-day code review (stdin pipe closed)
2. OpenCode detected disconnection ("Server bastion disconnected")
3. All subsequent MCP tool calls returned "Not connected"
4. Manual verification showed binary working correctly

**Root Cause:** OpenCode's MCP client does not auto-restart `type: "local"` servers after they crash. The process manager has no watchdog or retry mechanism for local processes.

**Current Workaround:** Restart OpenCode session entirely. This is the only way to restore connectivity.

**Proposed Fix (OpenCode-level):**
- Implement auto-restart with exponential backoff for local MCP servers
- Add `"auto_restart": true` option in opencode.json server config
- Maximum 3 restart attempts within a 30-second window

**Proposed Fix (Bastion-level):**
- Add health check endpoint accessible without MCP initialization
- Implement graceful shutdown signal handler to prevent orphaned state
- Write PID file for external monitoring

---

### F-002: Stdio Transport "connection closed: initialize request"

**Severity:** 🔴 HIGH  
**Category:** MCP Connectivity  
**Observed:** When OpenCode reconnects to a stdio-based gateway, the rmcp framework fails with:
```
ERROR bastion_gateway: MCP server error: Failed to start MCP server: connection closed: initialize request
```

**Manual Verification:** The gateway binary responds correctly to MCP initialize when piped via shell:
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize",...}' | timeout 4 bastion-gateway --transport stdio
# Response: {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05",...}}
```

**Root Cause:** Race condition between gateway startup and OpenCode's MCP initialize request. The gateway receives the initialize request before completing internal initialization (provider detection ~500ms, worker binary verification ~200ms, registry startup ~100ms).

**Proposed Fix:**
1. Start MCP server FIRST, then run slow initialization in background
2. Add readiness signal via stdout before accepting MCP on stdin
3. Switch to HTTP transport as default (eliminates race condition)

---

### F-003: HTTP Transport — MCP Tools Don't Auto-Connect

**Severity:** 🟡 MEDIUM  
**Category:** MCP Connectivity  
**Observed:** When gateway runs in HTTP mode (`--transport http`), it starts successfully on `127.0.0.1:8765`, but MCP tools still show "Not connected".

**Root Cause:** MCP protocol mismatch. OpenCode's `type: "local"` expects stdio transport. HTTP transport requires `type: "sse"` configuration.

**Proposed Fix:**
- Document both transport modes in Bastion README
- Auto-detect transport from CLI args and log appropriate connection string

---

### F-004: Image Not Found — `debian-slim`, `ubuntu`

**Severity:** 🟢 LOW  
**Category:** Image/Template Resolution  
**Observed:**
```
Error: no such image: docker.io/library/debian-slim: image not known
Error: no such image: docker.io/library/ubuntu: image not known
```

**Root Cause:** Templates must use fully qualified image names. Correct: `debian:bookworm-slim`.

**Proposed Fix:**
1. Add image name validation with helpful suggestions
2. Implement `sandbox_list_templates` tool
3. Add image alias support in gateway config

---

### F-005: sandbox_prepare Timeout on First Attempt

**Severity:** 🟡 MEDIUM  
**Category:** Timeouts  
**Observed:**
- **Attempt 1:** `sandbox_prepare(jvm-build)` → `MCP error -32001: Request timed out` (after ~10s)
- **Retry:** Exit code 100 (apt lock held by previous attempt)
- **Attempt 3:** ✅ Success in 18,122ms

**Root Cause:** The MCP request timeout (~10s) is shorter than the apt-get install operation (15-18s). On first attempt, apt-get needs to download packages. On retry, cache is partially populated but dpkg lock is held by previous attempt. Third attempt works after lock is released.

**Proposed Fix:**
1. Add `timeout_ms` parameter to `sandbox_prepare` (default: 600s)
2. Increase MCP-level request timeout for long-running tools
3. Support streaming progress notifications during preparation

---

### F-006: Maven Build Timeout With Combined Operations

**Severity:** 🟡 MEDIUM  
**Category:** Timeouts  
**Observed:** `git clone + mvn package` combined exceeds MCP timeout (~10s). Separated into two calls: clone (7s) + build (15s) = works.

**Proposed Fix:**
1. Document recommended separation of long operations
2. Increase MCP-level timeout or support streaming output
3. Auto-detect env_ref for sandbox_run after sandbox_prepare

---

### F-007: Git Clone `--depth1` Syntax Invalid

**Severity:** 🟢 LOW  
**Category:** User/API Ergonomics  
**Observed:** `git clone --depth1` → `error: unknown option 'depth1'`. Correct: `git clone --depth 1`.

**Proposed Fix:** None needed (correct git usage is user's responsibility).

---

### F-008: ToolResolver Always Chooses Apt Over Asdf

**Severity:** 🟢 LOW  
**Category:** User/API Ergonomics  
**Observed:** Both `AptAdapter` and `AsdfAdapter` report `SupportLevel::Full` for `jvm-build`. Apt is registered first, gets priority.

**Design Decision:** Intentional — `AptAdapter` is preferred on Debian because it's faster (18s vs 67s) and more reliable.

**Proposed Enhancement:** Add `strategy` parameter to override adapter priority.

---

### F-009: 🆕 AsdfAdapter Hardcodes Wrong Java Version Identifier

**Severity:** 🔴 HIGH  
**Category:** Bug (functional)  
**File:** `crates/bastion-infrastructure/src/template/adapters/asdf.rs:64`

**Observed:**
```rust
// BUG: Uses "openjdk-17.0.8+7" but the asdf-java plugin uses "adoptopenjdk-17.0.8+7"
command: format!("... && asdf install java openjdk-17.0.8+7 && asdf global java openjdk-17.0.8+7"),
```

Manual test:
```bash
$ asdf install java openjdk-17.0.8+7
Unknown release: openjdk-17.0.8+7

$ asdf list-all java | grep "17\.0\.8"
adoptopenjdk-17.0.8+7   # ← Correct identifier
```

**Impact:** If ToolResolver chose AsdfAdapter, it would crash on Step 3 ("Install Java 17 via asdf") with "Unknown release".

**Fix:** Change `openjdk-17.0.8+7` → `adoptopenjdk-17.0.8+7` on lines 64 and 66.

---

### F-010: 🆕 sandbox_read Broken — Base64 Decoding Failure

**Severity:** 🔴 HIGH  
**Category:** Bug (functional)  
**File:** `crates/bastion-gateway/src/server.rs` (sandbox_read handler)

**Observed:** `sandbox_read` fails for ALL files (text and binary):
```
Error: Failed to decode base64: Invalid symbol 10, offset 76.
```
```
Error: Failed to decode base64: Invalid symbol 10, offset 1304.
```

"Symbol 10" is LF (newline character \n). The base64 encoding includes newlines in the MCP response, and the base64 decoder chokes on them.

**Impact:** Cannot pull files from sandbox. Read-only operation broken.

**Proposed Fix:** Strip whitespace/newlines from base64 string before decoding, or use a streaming binary transport.

---

### F-011: 🆕 sandbox_sync Is a Stub

**Severity:** 🔴 HIGH  
**Category:** Bug (functional)  
**File:** `crates/bastion-gateway/src/server.rs:935-959`

**Observed:**
```rust
return serde_json::json!({
    "error": "sandbox sync requires provider-specific implementation",
    "hint": "Use sandbox_write/sandbox_read for file operations"
}).to_string();
```

**Impact:** The `sandbox_sync` MCP tool is not implemented. Files can be pushed/pulled via `sandbox_write`/`sandbox_read`, but `sandbox_read` is also broken (F-010).

**Current Workaround:** `sandbox_write` → `sandbox_run` (compilar) works. But no way to pull results back.

---

### F-012: 🆕 snapshot restore Doesn't Register in MCP Repository

**Severity:** 🔴 HIGH  
**Category:** Bug (functional)  
**File:** `crates/bastion-gateway/src/server.rs:885-898`

**Observed:**
1. `sandbox_snapshot(restore)` → returns `{"sandbox_id": "90b1d3a7-...", "status": "restored"}`
2. `sandbox_run(sandbox_id="90b1d3a7-...")` → `"Sandbox not found"`
3. `podman ps` → Container EXISTS and running

**Root Cause:** The snapshot handler calls `snapshot_manager.restore_snapshot()` which creates a podman container directly, but never registers it in `self.repository` (the gateway's sandbox tracking system).

**Impact:** Restored sandboxes are invisible to all other MCP operations.

**Proposed Fix:** After `restore_snapshot()` returns, register the sandbox in `self.repository` (similar to `sandbox_create`).

---

### F-013: 🆕 snapshot list Returns Empty Despite Existing Snapshots

**Severity:** 🟡 MEDIUM  
**Category:** Bug (functional)  
**File:** `crates/bastion-infrastructure/src/template/snapshot.rs:175-209`

**Observed:**
- `sandbox_snapshot(list)` → `{"snapshots":[], "count": 0}`
- `podman images bastion-snap-*` → shows 4 images
- Images have `localhost/` prefix: `localhost/bastion-snap-jvm-build-snapshot:latest`

**Root Cause:** The `podman images` wildcard filter `bastion-snap-*` doesn't match images with `localhost/` prefix in the Rust `tokio::process::Command` execution context.

**Proposed Fix:** Use explicit filter or strip the `localhost/` prefix:
```rust
.args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
// Then filter in Rust code
```

---

### F-014: 🆕 Only Podman Provider Supported via MCP

**Severity:** 🟡 MEDIUM  
**Category:** Feature Gap  
**File:** `crates/bastion-gateway/src/main.rs:181-182`

**Observed:** Firecracker (`/home/rubentxu/.local/bin/firecracker`) and gVisor (`/usr/local/bin/runsc`) are installed and available, but:
```rust
let mut factory = ProviderFactory::new("podman");
```
Only podman provider is registered. No provider selection in `sandbox_create`.

**Impact:** Cannot test providers D (Firecracker) and E (gVisor) via MCP.

**Proposed Fix:** Add `provider` parameter to `SandboxCreateParams` and register all available providers.

---

### F-015: 🆕 Snapshot Create Timeout on First Attempt

**Severity:** 🟡 MEDIUM  
**Category:** Timeouts  
**Observed:**
- **Attempt 1:** `sandbox_snapshot(create)` → `MCP error -32001: Request timed out`
- **Attempt 2:** ✅ Success (cached layers)

**Root Cause:** `podman commit` takes 13s, which exceeds MCP timeout (~10s).

---

## 4. Workflow Improvement Proposals

### W-001: Readiness Protocol
Implement a readiness signal via stdout before starting MCP serve to prevent race conditions.

### W-002: Timeout Configuration
Expose `--mcp-timeout` CLI flag (default: 120s). Support per-tool timeout hints.

### W-003: Template Discovery
Add `sandbox_list_templates` tool for image discovery.

### W-004: Error Message Quality
Improve error messages with suggestions (e.g., "Did you mean `debian:bookworm-slim`?").

### W-005: env_ref Auto-Injection
Auto-inject the last `env_ref` from `sandbox_prepare` into `sandbox_run`.

### W-006: Graceful Degradation for MCP Disconnects
PID file, auto-restart, health endpoint independent of MCP.

---

## 5. 🆕 Sandbox Registry — Orphaned Containers

**Problem:** 6 containers exist in podman but MCP reports 0 sandboxes. These are orphaned from previous sessions.

```
7b06189d  - 6h old (original PetClinic test)
1c21d889  - ~1h old
0f61d690  - 50m old
5d4a52f8  - 48m old
upbeat_thompson  - 40m old
bastion-snap-test - 40m old
```

**Root Cause:** The gateway's in-memory repository is lost on restart. Containers persist in podman but the gateway has no way to re-discover them.

**Proposed Fix:** On gateway startup, scan podman for containers with bastion naming convention and sync `self.repository`.

---

## 6. Action Items (Priority-Ordered)

| # | Action | Severity | Effort | Category |
|---|--------|----------|--------|----------|
| 1 | Fix AsdfAdapter java version (F-009) | 🔴 | Tiny | Bug |
| 2 | Fix sandbox_read base64 decoding (F-010) | 🔴 | Small | Bug |
| 3 | Fix snapshot restore registration (F-012) | 🔴 | Small | Bug |
| 4 | Implement sandbox_sync (F-011) | 🔴 | Large | Feature |
| 5 | Fix snapshot list (F-013) | 🟡 | Small | Bug |
| 6 | Add multi-provider support (F-014) | 🟡 | Large | Feature |
| 7 | Fix stdio transport race condition (F-002) | 🔴 | Medium | Bug |
| 8 | Add readiness protocol (W-001) | 🔴 | Small | Improvement |
| 9 | Increase MCP timeout for long operations (F-005) | 🟡 | Small | Configuration |
| 10 | Add sandbox_list_templates tool (W-003) | 🟢 | Small | Feature |
| 11 | Improve error messages (W-004) | 🟢 | Small | UX |
| 12 | Add auto-restart for MCP servers (F-001) | 🔴 | Large | OpenCode |
| 13 | Support strategy override in sandbox_prepare (F-008) | 🟢 | Small | Feature |
| 14 | Document env_ref behavior and usage (F-006) | 🟡 | Small | Docs |
| 15 | Sync orphaned containers on startup (Section 5) | 🟡 | Medium | Feature |
