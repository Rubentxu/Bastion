# Plan de Pruebas E2E — Template Artifacts

## Features a probar

| ID | Feature | Tipo | Criticidad |
|----|---------|------|------------|
| T01 | `ArtifactCatalog` register + resolve | Unit | HIGH |
| T02 | `UniversalMaterializer` extract + verify | Integration | HIGH |
| T03 | `PodmanOptimizedMaterializer` host cache + podman cp | Integration | HIGH |
| T04 | `ZipLayerMaterializer` tar→zip + layer deploy | Integration | MEDIUM |
| T05 | `LayerArtifact` + `LayerStack` limits | Unit | MEDIUM |
| T06 | `sandbox_prepare` MCP tool | E2E via gateway | CRITICAL |
| T07 | `SnapshotManager` create → restore → verify | E2E via gateway | HIGH |
| T08 | Full pipeline: create → prepare → build → snapshot → restore | E2E via gateway | CRITICAL |
| T09 | Host cache reuse (2nd sandbox faster) | Performance | MEDIUM |
| T10 | Error handling: unknown capability, missing artifact | E2E | MEDIUM |

## Prerequisites

- Podman daemon running
- Gateway binary built (`cargo build -p bastion-gateway`)
- Worker binary built (`cargo build -p bastion-worker`)
- `/var/lib/bastion/artifacts` writable (or use temp dir)

## Test Suite Structure

### 1. Unit tests (cargo test)
- Domain: `cargo test -p bastion-domain -- template`
- Infrastructure: `cargo test -p bastion-infrastructure -- template`
- Infrastructure: `cargo test -p bastion-infrastructure --test template_materializer_test`
- Infrastructure: `cargo test -p bastion-infrastructure --test podman_optimized_test`
- Infrastructure: `cargo test -p bastion-infrastructure --test layer_materializer_test`
- Infrastructure: `cargo test -p bastion-infrastructure --lib template::snapshot`

### 2. E2E Gateway test (Python script)

Script: `/tmp/e2e_template_artifacts.py`

Tests:

1. **Gateway health check** — `sandbox_health`
2. **Create sandbox** — `sandbox_create`
3. **Prepare jvm capability** — `sandbox_prepare(sandbox_id, capability="jvm-build")`
   - Verify: capability resolved
   - Verify: materialization cached
4. **Manual jvm install** (fallback since no artifacts registered) — `apt-get install java maven git`
5. **PetClinic full build** — download + `mvn package`
6. **Snapshot create** — `podman commit` via script
7. **Snapshot restore** — `podman create` + `podman start`
8. **Verify restored sandbox** — `java -version`, `mvn -version`
9. **Cleanup**

### 3. Performance metrics

| Test | Metric | Expected |
|------|--------|----------|
| T02 | Extract duration | < 1s |
| T03 | Host cache hit duration | < 500ms |
| T09 | 2nd sandbox materialization | ≤ 1st duration |

## Expected results

| ID | Expected |
|----|----------|
| T01 | PASS - catalog register + resolve 4 assertions |
| T02 | PASS - artifact materialized, file verified |
| T03 | PASS - podman cp works, host cache reused |
| T04 | PASS - zip created, layer deployed |
| T05 | PASS - layer stack: 5 layers max, duplicates blocked |
| T06 | PASS - sandbox_prepare returns env_ref |
| T07 | PASS - snapshot create/restore (if podman available) |
| T08 | PASS - full PetClinic build pipeline |
| T09 | PASS - 2nd materialization faster or equal |
| T10 | PASS - graceful error on missing capability |
