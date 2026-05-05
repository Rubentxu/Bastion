# PetClinic E2E Validation — Fase 013

> **Date:** 2026-05-05 | **Status:** 7/8 steps passed, 1 bug found (sandbox_run missing), sync issue after cancel

---

## Environment

| Component | Value |
|-----------|-------|
| Gateway | `bastion-gateway` HTTP daemon on `127.0.0.1:18765` |
| Pool | Enabled (min_idle=2, max_idle=20, max_total=200) |
| Keep-alive | 30 minutes (up from 5 min default) |
| Transport | Streamable HTTP (MCP 2025-03-26) |
| Base image | `debian:bookworm-slim` |
| Test client | Python Streamable HTTP (`/tmp/petclinic_e2e_fase013.py`) |

### Fixes Applied Since Fase 012
- **Pool enabled** by default (`pool_enabled: true`)
- **Keep-alive** extended to 30 min
- **sandbox_cancel** tool added (SIGTERM → SIGKILL)
- **Per-client rate limiting** (100 burst / 20 req/s per client)
- **Background expiration enforcer** (60s interval)
- **asdf bootstrap** fix (prereq install + source fix)
- **sandbox_run restored** — tool was accidentally removed during refactoring

---

## Test Results

### ✅ P1: sandbox_create (Pool Evaluation)

| Metric | Value |
|--------|-------|
| Time | **0.01s** |
| `from_pool` | `true` |
| Status | `running` |

**Evaluation:** Pool works perfectly. Sandbox creation went from ~5s (cold) to **0.01s** (from pool). This is a **500x improvement** in sandbox startup time. The pool had 2 idle sandboxes pre-warmed and ready.

### ✅ P2: apt-get install git (sandbox_run)

| Metric | Value |
|--------|-------|
| Time | **14.59s** |
| Exit code | `0` |
| Note | `debconf: delaying package configuration` (benign) |

**Evaluation:** `sandbox_run` works correctly after being restored. The tool was accidentally removed during the high-performance refactoring — this is a **regression bug** (see Bugs Found).

### ✅ P3: sandbox_prepare jvm-build (apt strategy)

| Metric | Value |
|--------|-------|
| Time | **55.16s** |
| Adapter | `apt` |
| Tools installed | openjdk-17-jdk, maven |
| env_ref | `registry:{sandbox_id}:jvm-build` |

**Evaluation:** JVM build environment prepared successfully via apt strategy in 55s. The `env_ref` mechanism correctly stores the environment for later injection into `sandbox_run` and `sandbox_run_stream`.

### ✅ P4: git clone PetClinic

| Metric | Value |
|--------|-------|
| Time | **1.06s** |
| Exit code | `0` |
| Clone method | `--depth 1` (shallow) |

**Evaluation:** Git clone works as expected. Shallow clone keeps the time under 2s.

### ✅ P5: mvn package (Streaming + Keep-alive)

| Metric | Value |
|--------|-------|
| Time | **70.13s** |
| Exit code | `0` |
| Chunks received | `2` |
| Result | **BUILD SUCCESS** |
| env_ref injection | ✅ `JAVA_HOME` and `MAVEN_HOME` auto-injected |

**Evaluation:** Maven build completed successfully in 70s with streaming output. Keep-alive held the full 70s without disconnection — the 30-min keep-alive fix works. The `env_ref` from P3 was correctly auto-injected, making `mvn` available in the command.

### ✅ P6: Verify JAR Artifact

| Metric | Value |
|--------|-------|
| Time | **0.18s** |
| Files in target/ | `12` |
| JAR found | ✅ `spring-petclinic-4.0.0-SNAPSHOT.jar` |
| JAR size | **67MB** |
| Total target/ size | **69MB** |

**Evaluation:** `sandbox_list_files` correctly lists files in the container. JAR artifact verified at expected location and size.

### ✅ P7: sandbox_cancel

| Metric | Value |
|--------|-------|
| Cancel response time | **3.1s** (grace_period: 3000ms) |
| Sleep total duration | **20.1s** (2s start + 3s grace + 15s SIGKILL) |
| Cancel mechanism | SIGTERM → container alive → SIGKILL |
| Exit code of cancelled process | `137` (SIGKILL received) |

**Evaluation:** `sandbox_cancel` works correctly — SIGTERM was sent, grace period waited, then SIGKILL. The cancel prevented a 300s sleep from running to completion. However, the total duration (20s) was longer than expected because:
1. The `sleep 300` process was in a container that doesn't handle SIGTERM for PID > 1 properly
2. The grace period (3s) plus SIGKILL escalation is working as designed
3. The MCP session still waited for the `sandbox_run` thread to return (blocked on read)

**Gateway log evidence:**
```
SIGTERM sent, waiting grace period sandbox_id=c20c2f09...
Container still alive after SIGTERM, sending SIGKILL sandbox_id=c20c2f09...
```

### ❌ P8: sandbox_sync pull

| Metric | Value |
|--------|-------|
| Time | **0.12s** |
| Backend | `tar` |
| Error | `Sync failed: ` (empty error message) |
| Host files received | **0** |

**Evaluation:** Sync failed because P7 killed the container with SIGKILL. The worker process inside the container is dead, so `podman exec` (used by tar sync) cannot execute. The empty error message is a bug — `stderr` was empty because the container was already stopped.

---

## Performance Comparison: Fase 012 vs Fase 013

| Step | Fase 012 (no pool) | Fase 013 (pool) | Improvement |
|------|--------------------|-----------------|-------------|
| sandbox_create | ~5.0s | **0.01s** | **500x faster** |
| apt-get install git | 14.6s | 14.6s | Same |
| sandbox_prepare (apt) | 56s | 55.2s | Same |
| git clone | 1.1s | 1.1s | Same |
| mvn package | 91.6s | **70.1s** | **23% faster** |
| **Total pipeline** | **~164s** | **~141s** | **14% faster** |

**Note:** Maven build was 23% faster (70s vs 92s) likely due to Maven artifact caching in the container's `.m2/repository`.

---

## Bugs Found

### 🐛 F-024: sandbox_run tool accidentally removed

**Severity:** Critical
**Symptom:** `sandbox_run` MCP tool missing from `tools/list`. Calling it returns `"tool not found"`.
**Root Cause:** During the high-performance Tier 1+2 implementation, the `sandbox_run` tool method was not included when server.rs was refactored. The `SandboxRunParams` struct exists but the `#[tool]` method was removed.
**Fix:** Restored `sandbox_run` tool method in `server.rs` with rate limiting, env_ref injection, and secret resolution.
**Files:** `crates/bastion-gateway/src/server.rs`

### 🐛 F-025: sandbox_sync empty error message on dead container

**Severity:** Medium
**Symptom:** `sandbox_sync` returns `{"error": "Sync failed: "}` with empty message when container is dead.
**Root Cause:** The error handler uses `stderr.lines().next().unwrap_or("unknown error")` but stderr is empty because the container is stopped. The `lines()` iterator returns `Some("")` for an empty string.
**Fix needed:** Check for empty stderr before using it as error message:
```rust
let stderr = String::from_utf8_lossy(&output.stderr);
let msg = if stderr.trim().is_empty() { "container not running".to_string() } else { stderr.lines().next().unwrap_or("unknown error").to_string() };
```
**Files:** `crates/bastion-gateway/src/server.rs:1542`

### 🐛 F-026: Pool stats spamming logs every 5s

**Severity:** Low
**Symptom:** `Pool stats active=X idle=Y` logged every 5 seconds, filling up log files.
**Root Cause:** `PoolManager::start()` uses a 5s refill interval and logs stats on every tick.
**Fix needed:** Change log level from `DEBUG` to `TRACE`, or reduce frequency to 60s when pool is stable.
**Files:** `crates/bastion-infrastructure/src/pool/manager.rs`

---

## Improvement Proposals

### Proposal 1: sandbox_cancel should return clearer response

**Current:** Returns `{"sandbox_id":"...","status":"cancelled"}` even when the cancel returns `Ok(false)` (no running command).
**Proposed:** Return `{"status":"cancelled"}` only when SIGTERM was actually sent. Return `{"status":"no_running_command"}` when the provider confirms no command was running.

### Proposal 2: sandbox_sync should check container health before sync

**Current:** Attempts `podman exec` regardless of container state, resulting in cryptic errors.
**Proposed:** Before sync, call `is_alive()` to check container health. Return a clear error: `"Container is not running. Cannot sync files."`

### Proposal 3: sandbox_run should support cancellation natively

**Current:** `sandbox_run` is synchronous (`cmd.output()`) and blocks until completion. It cannot be cancelled mid-execution.
**Proposed:** Track running commands with a `DashMap<SandboxId, ChildProcess>` so `sandbox_cancel` can signal them. This is especially important for `PodmanProvider` where `exec` creates a separate process.

### Proposal 4: Reduce pool stats log noise

**Current:** `DEBUG` level log every 5s.
**Proposed:** Change to `TRACE` level, or implement a "stable pool" mode where stats are only logged when pool state changes.

### Proposal 5: Add sandbox_run tool to CI regression tests

**Current:** The removal of `sandbox_run` was not caught by any test.
**Proposed:** Add an integration test that calls `tools/list` and asserts all expected tools are present. This would catch accidental tool removal.

---

## Session Evidence

### Gateway Log Summary
- Pool initialized with `idle=2`
- Container `c20c2f09` checked out from pool in 0.01s
- JVM prepare via apt: 55s
- Maven streaming build: 70s (keep-alive held)
- Cancel: SIGTERM → grace 3s → SIGKILL
- Sync failed (container dead after SIGKILL)
- Terminate returned `{"status":"pooled"}` (sandbox returned to pool)

### MCP Session
- Session ID: `9b6496b0-8f0...`
- Total active time: ~2 minutes 41 seconds
- Keep-alive: No disconnection ✅

---

## Next Steps
1. Fix F-025 (sync empty error) and F-026 (pool log spam)
2. Implement Proposal 2 (container health check before sync)
3. Implement Proposal 5 (tool registry integration test)
4. Re-run P8 sync on a live container (without prior cancel)
5. Test asdf strategy with bootstrap fix (previously failed)
6. Test snapshot lifecycle (sandbox_snapshot create/restore/list)
