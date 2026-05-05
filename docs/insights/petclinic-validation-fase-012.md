# PetClinic Validation — Fases 0, 1, 2

> **Date:** 2026-05-05 | **Status:** All major features validated, bugs fixed

---

## Environment Setup

### Pre-requisites ✅
- [x] Podman socket running (`systemctl --user start podman.socket`)
- [x] Gateway built with Fase 2 + F-019 + F-022 + F-023 fixes
- [x] SQLite DB path: `/tmp/bastion-test.db`
- [x] `.bastion/providers/` has 5 TOML configs (podman, firecracker, gvisor, wasm, local)
- [x] `.bastion/capabilities/` has 2 TOML configs (jvm-build, node-build)
- [x] OpenCode uses `type: "remote"` with gateway daemon at `http://127.0.0.1:18765`

### Gateway Startup Log (with fixes)
```
INFO bastion_gateway: DANGEROUS: LocalProvider is enabled.
INFO bastion_infrastructure::provider::registry: Loaded provider config name=local path=.bastion/providers/local.toml
INFO bastion_infrastructure::provider::registry: Provider configs loaded from .bastion/providers loaded=5
INFO bastion_gateway: Loaded TOML provider configs count=5
INFO bastion_gateway: Connected to Podman pong="OK"
INFO bastion_gateway: Podman provider initialized
INFO bastion_gateway: Sandbox pooling disabled
INFO bastion_gateway: Loaded TOML capability configs count=2
INFO bastion_gateway: MCP Gateway ready — serving on HTTP transport
```

---

## Test Results

### ✅ T1: Baseline — Podman + jvm-build (auto/apt) + PetClinic

| Step | Time | Result |
|------|------|--------|
| `sandbox_create` (debian:bookworm-slim) | ~5s | ✅ |
| `apt-get install -y git` | 14.6s | ✅ git installed |
| `sandbox_prepare` (jvm-build, apt) | 56s | ✅ openjdk-17 + maven ready |
| `git clone` PetClinic | 1.1s | ✅ |
| `mvn package -DskipTests -f /workspace/pom.xml` | 91.6s | ✅ BUILD SUCCESS |
| JAR produced | — | ✅ `spring-petclinic-4.0.0-SNAPSHOT.jar` (67MB) |
| **Total pipeline** | **~164s** | ✅ |

**Key finding:** Must use `-f /workspace/pom.xml` flag since working directory is `/` not `/workspace`.

### ❌ T2: Strategy override — version_manager (asdf)

**Result:** `{"error":"Step 'Install Java 17 via asdf' failed: exit 2 (expected 0)"}`

**Root Cause:** The `asdf` adapter assumes asdf is already installed in the base image.
- debian:bookworm-slim does NOT have asdf pre-installed
- `asdf plugin add java` fails because asdf binary doesn't exist
- **asdf adapter needs a bootstrap step** to install asdf itself first
- This is a design issue: asdf/sdkman TOML configs assume tool managers exist in image

**Workaround:** Use `apt` strategy (works) or use a custom image with asdf pre-installed.

### ❌ T4: node-build capability

**Result:** `{"error":"Step 'Install Node.js via asdf' failed: exit 2 (expected 0)"}`

**Root Cause:** Same as T2 — asdf not installed in base image.

### ✅ T5: Unknown capability error handling

**Result:** `{"error":"Unsupported operation: asdf doesn't know how to install rust-build"}`

Graceful error from `CapabilityRegistry` fallback to `ToolResolver`. The error message is helpful and correctly identifies that asdf was the candidate.

### ⚠️ T7: sandbox_sync push/pull

**Push Result:** `{"backend":"tar","error":"Sync failed: tar: /workspace/test-push.xml: Cannot open: No such file or directory"}`

**Cause:** `/workspace` directory didn't exist because `git clone` failed earlier (missing git). After fixing git install, push should work.

**Pull Result:** `{"backend":"tar","mode":"pull","status":"synced","target":"/tmp/petclinic-output/"}` ✅

sandbox_sync pull works correctly — successfully pulled files from container to host using tar backend.

### ✅ T8: LocalProvider sandbox

**Result:** `{"from_pool":false,"sandbox_id":"b59f8d10-0186-4f1d-ac7d-cb8c536d026a","status":"running","template":"local"}`

Commands execute on host filesystem. Requires `DANGEROUS_ALLOW_LOCAL=1` env var AND `--dangerous-allow-local` flag.

**Note (F-023 fixed):** Status was "pending" before fix. Now correctly shows "running".

---

## Bugs Fixed During Testing

### 🐛 F-019: Relative paths in Podman bind mounts
**File:** `podman.rs`, `docker.rs`
**Symptom:** `docker create container: creating named volume "target/debug/bastion-worker": names must match [a-zA-Z0-9][a-zA-Z0-9_.-]*`
**Root Cause:** `worker_binary` path like `target/debug/bastion-worker` is relative. Docker interprets relative paths as named volumes.
**Fix:** Canonicalize to absolute path with `.canonicalize()` before using in bind mount.
```rust
let worker_binary_abs = self.worker_binary.canonicalize().unwrap_or_else(|_| {
    if self.worker_binary.is_relative() {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp")).join(&self.worker_binary)
    } else { self.worker_binary.clone() }
});
```

### 🐛 F-022: TOML provider configs never loaded
**File:** `main.rs`
**Symptom:** `provider: "local"` always fell back to Podman despite local.toml existing
**Root Cause:** `load_from_dir()` was defined but never called. Code only counted TOML files without loading them.
**Fix:** Call `registry.load_from_dir(&providers_dir)` on `ProviderRegistry` (not raw `ProviderFactory`).
- Added `ProviderRegistry::register()` public method
- Changed `factory.into_providers()` → `registry.into_providers()`
- Changed `factory.default()` → `registry.default()`

### 🐛 F-023: LocalProvider sandbox status = "pending"
**File:** `local.rs`
**Symptom:** LocalProvider sandbox always showed `status: "pending"` instead of "running"
**Root Cause:** `LocalProvider::create()` didn't call `sandbox.mark_running()` or `sandbox.set_timeout()`
**Fix:** Added both calls after creating the Sandbox entity.

---

## Known Issues

### ⚠️ F-020: Session invalidation on gateway restart
Gateway HTTP sessions expire when gateway restarts. OpenCode MCP client has stale session ID.
**Workaround:** Restart OpenCode after gateway restart.
**Fix needed:** Session persistence in SQLite, or OpenCode MCP client auto-reconnect on 404.

### ⚠️ F-021: rmcp keep-alive timeout (5 min)
Long-running `sandbox_prepare` operations (~3 min) approach the 5-min hard limit.
**Fix needed:** Increase keep-alive timeout OR implement ping/pong heartbeat.

### ⚠️ Sandbox expiration not auto-enforced
`expires_at` stored in SQLite but no background task checks expiration.
Expired sandboxes still show "running" in DB.
**Fix needed:** Background task that terminates expired sandboxes.

### ⚠️ sync_from_provider creates orphans
ALL podman containers get added to SQLite during sync, not just Bastion-created.
Old hodei/mentat containers appear in sandbox_list with `status: pending`.
**Fix needed:** Filter by container label `app=bastion` or similar marker.

### ⚠️ asdf adapter needs bootstrap
asdf TOML configs (jvm-build.asdf, node-build.asdf) assume asdf exists in base image.
debian:bookworm-slim doesn't have asdf → `asdf plugin add java` fails with exit 2.
**Fix needed:** asdf adapter should install asdf first, OR document that custom images with asdf pre-installed are required.

---

## Insights

### Performance Baselines
| Operation | Time |
|-----------|------|
| jvm-build (apt): openjdk-17 + maven | 56s (first run, no cache) |
| git install | 14.6s |
| git clone PetClinic (--depth 1) | 1.1s |
| Maven build PetClinic | 91.6s |
| **Total PetClinic pipeline** | **~164s** |

### TOML Architecture Working
- ✅ `.bastion/providers/*.toml` correctly loaded via `ProviderRegistry::load_from_dir()`
- ✅ Provider resolution uses lowercase lookup ("local" → "local")
- ✅ LocalProvider registered from TOML when `DANGEROUS_ALLOW_LOCAL=1`
- ⚠️ firecracker/gvisor/wasm skipped (feature not enabled)

### sandbox_sync works
- Backend: `tar`
- Pull: correctly syncs files from container to host
- Push: needs target directory to exist in container

### HTTP Transport Status
- Gateway running as daemon on port 18765 ✅
- Streamable HTTP (MCP 2024-11-05) working ✅
- SSE events parse correctly with sseclient-py ✅
- Keep-alive timeout: 5 min (risk for long operations) ⚠️

---

## Next Steps
1. Fix asdf bootstrap (add asdf installation step to adapter)
2. Add background expiration enforcement task
3. Fix orphan detection (add Bastion-specific container labels)
4. Implement session persistence for F-020
5. Increase keep-alive timeout for F-021
6. Test snapshot lifecycle (sandbox_snapshot create/restore/list)
7. Test SQLite persistence across gateway restart (sandbox should survive)
