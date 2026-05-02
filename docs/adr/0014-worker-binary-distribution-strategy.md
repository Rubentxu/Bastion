# ADR-0014: Worker Binary Distribution Strategy

## Status

**Implemented** (2026-05-02, commit 90a29a8)

> ✅ TLS migration complete: `tls-native-roots` → `tls-ring`

## Context

The `bastion-worker` binary must be present inside every sandbox container across multiple backends:
Podman/Docker (current), Kubernetes, Firecracker MicroVM, gVisor, and Lambda/FaaS.

### Current State (Post-Implementation)

- Worker binary uses **BoringSSL/ring** for TLS (musl-compatible) ✅
- `Cargo.toml` now uses `tonic = { features = ["tls-ring", "gzip"] }` instead of `tls-native-roots`
- **PodmanProvider** bind-mounts the host binary into containers
- **FirecrackerProvider** and **gVisorProvider** copy the binary into rootfs at VM/container creation time
- `.cargo/config.toml` has musl linker configured
- `scripts/build-worker.sh` exists for musl builds
- **Remaining**: Install `musl-gcc` toolchain to enable musl builds (`apt install musl-tools`)

### Problem

Firecracker and gVisor rootfs images use musl libc (buildroot-based). A glibc-linked binary will not execute in these environments. The current bind-mount approach only works for Podman and fails for:
- **Read-only rootfs** containers
- **Kubernetes** without hostPath volumes
- **MicroVM** environments (Firecracker, gVisor)
- **scratch/distroless** base images that contain no glibc

### Backend Requirements Summary

| Backend | Current Method | Binary Format Required |
|---------|---------------|----------------------|
| Podman/Docker | Bind-mount from host | glibc or musl |
| Kubernetes | Init container or ConfigMap | Static musl |
| Firecracker | Copy into rootfs at VM start | **Static musl** |
| gVisor | Copy into OCI rootfs bundle | **Static musl** |
| Lambda/FaaS | Custom runtime bootstrap | **Static musl** |

## Decision

**Adopt a single static musl binary built with rustls TLS as the universal distribution artifact for all backends.**

### Chosen Strategy: Static musl Binary with rustls TLS

1. **Switch TLS backend**: Change `tonic` features from `tls-native-roots` (OpenSSL) to `tls` (rustls)
   ```toml
   # From:
   tonic = { version = "0.14", features = ["tls-native-roots", "gzip"] }
   # To:
   tonic = { version = "0.14", features = ["tls", "gzip"] }
   ```

2. **Build target**: `x86_64-unknown-linux-musl` (already configured in `.cargo/config.toml`)

3. **Single binary for all providers** — each provider chooses its own injection mechanism:
   - **Podman**: bind-mount (unchanged — works with any binary)
   - **Kubernetes**: init container copies binary from shared volume or ConfigMap
   - **Firecracker**: copy static binary into buildroot-based rootfs
   - **gVisor**: copy static binary into OCI rootfs bundle
   - **Lambda/FaaS**: bootstrap script downloads binary via HTTP with sha256 verification

### Rationale

- **rustls** is pure Rust (no C dependencies), enabling fully static linking
- Single binary artifact simplifies CI/CD (one build, one artifact)
- Project workspace already includes `tokio-rustls = "0.26"` and `rustls-pemfile = "2.0"`
- Expected binary size: ~4-6MB (acceptable for all backends, well under Lambda's 50MB compressed limit)
- All other dependencies (tokio, prost, serde, hmac, sha2) already support static linking

## Consequences

### What Becomes Easier

- **Cross-backend consistency**: One binary works everywhere; no conditional compilation or per-backend builds
- **CI/CD simplification**: Single build command, single artifact, single hash to verify
- **Minimal container images**: Worker runs on `alpine:3.19` (~10MB + 5MB binary) or `distroless/base` (~20MB + 5MB binary)
- **Version management**: One binary to version, sign, and distribute

### What Becomes Harder

- **Build environment**: Requires `musl-tools` package (`apt install musl-tools`) and `rustup target add x86_64-unknown-linux-musl`
- **Debugging**: musl's allocator and libc behave differently than glibc; edge cases may surface
- **Build time**: ~30% increase due to static linking and musl compilation
- **Native TLS root certificates**: Must bundle CA certificates or use `webpki-roots` instead of system trust store

### Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| rustls behavior differs from OpenSSL under edge cases (cert validation, ALPN) | Low | rustls is production-hardened; run integration tests against all backends |
| musl allocator performance differs from glibc's under heavy load | Low | Worker is IO-bound (command execution), not allocator-bound; benchmark if concerned |
| Build breakage on CI due to missing musl-gcc | Medium | Document prerequisite in CI setup; add musl-tools to CI image |
| Multi-arch (aarch64) builds add complexity for ARM-based Firecracker/Lambda | Low | Defer multi-arch; `x86_64` covers current use cases |

## Alternatives Considered

### Option A: Static musl Binary (ACCEPTED)

See Decision section above.

### Option B: Shell Bootstrap with HTTP Download

A minimal shell script downloads the worker binary at container startup via curl/wget.

**Rejected because**:
- Network dependency at startup — container fails if gateway/artifact store is unreachable
- Requires curl or wget in base image (not available in distroless/scratch)
- Adds 200ms-2s cold-start latency
- Introduces additional failure modes during cold starts
- Two-step process (download + execute) complicates lifecycle management

### Option C: Pre-built Container Images per Backend

Publish versioned container images (`bastion-worker:musl-v0.1.0`, `bastion-worker:distroless-latest`) to a registry.

**Rejected because**:
- Multiple images per architecture creates version management complexity
- CI/CD must push images on every release
- Backend-specific images defeat the "single artifact" goal
- Registry storage costs and pull latency for large fleets
- Does not solve the Firecracker/gVisor rootfs problem (still need a binary to bake in)

### Option D: Multi-stage Docker Build per Deployment

Build the worker inside a Dockerfile and produce a deployable image in CI.

**Rejected because**:
- Build complexity: every deployment requires a full Rust toolchain in CI
- Larger image sizes (base + build artifacts)
- Version updates require full image rebuild
- Does not provide a standalone binary for Firecracker rootfs injection

### Option E: Compressed + Chunked Download (Jenkins agent.jar Style)

Two-stage bootstrap: tiny bootstrap binary downloads a compressed worker from the gateway.

**Rejected because**:
- Adds decompression dependency (gzip/zstd) to the bootstrap
- Two-step process adds latency and complexity
- Bootstrap still handles updates and integrity checks — more code to maintain
- Single static binary achieves the same goal with less complexity

## Migration Plan

### Phase 1: Switch TLS Backend
- [ ] Change `tonic` features in workspace `Cargo.toml`: `tls-native-roots` → `tls`
- [ ] Verify `tokio-rustls` and `rustls-pemfile` are imported where TLS config is constructed
- [ ] Run existing integration tests to confirm no TLS regressions

### Phase 2: Enable musl Build
- [ ] Verify `.cargo/config.toml` musl linker configuration (already present)
- [ ] Build: `cargo build --release --target x86_64-unknown-linux-musl -p bastion-worker`
- [ ] Verify static linking: `ldd` should report "not a dynamic executable"
- [ ] Verify binary runs in Alpine container locally

### Phase 3: Update Providers
- [ ] **PodmanProvider**: No change (bind-mount works with any binary)
- [ ] **FirecrackerProvider**: Update `inject_worker()` to verify binary is static musl
- [ ] **gVisorProvider**: Update `inject_worker()` to verify binary is static musl
- [ ] **Kubernetes**: Document init container pattern in deployment manifests

### Phase 4: Lambda/FaaS Bootstrap
- [ ] Create shell bootstrap script that downloads worker from artifact store
- [ ] Add sha256 integrity verification to bootstrap
- [ ] Document Lambda custom runtime configuration

## Security Considerations

| Concern | Approach |
|---------|----------|
| **Binary integrity** | sha256 checksum published with each release; bootstrap scripts verify before execution |
| **Worker→Gateway TLS** | rustls provides TLS 1.3 with certificate validation; no reduction in security from OpenSSL |
| **No secrets in binary** | Authentication uses HMAC challenge-response with pre-shared secret; binary contains no credentials |
| **Supply chain** | rustls and webpki-roots are audited pure-Rust crates; reduces attack surface vs OpenSSL C codebase |
| **CA certificates** | Use `webpki-roots` crate instead of system trust store (consistent behavior across all environments) |
