# Worker Binary Distribution Strategies — Research

> **Author**: SDD Explore Phase  
> **Date**: 2026-05-02  
> **Status**: Research complete  
> **Project**: Bastion — MCP Gateway for AI agent sandbox orchestration

---

## 1. Executive Summary

This document analyzes strategies for distributing the `bastion-worker` binary to sandbox containers across different backends (Podman, Kubernetes, Firecracker, gVisor, Lambda/FaaS). The analysis reveals that the current approach (bind-mount for Podman) works for development but fails for production deployment on MicroVMs and gVisor which require a **statically-linked musl binary**.

**Key Findings:**
- Current release binary: **2.5MB** dynamically linked against glibc
- All dependencies (tokio, tonic, prost, serde) support static linking
- **Critical blocker**: `tonic` with `tls-native-roots` feature pulls in OpenSSL which does NOT support musl static linking
- **Solution**: Switch to `rustls` (pure Rust TLS) to achieve full static linking
- Expected musl binary size: ~4-6MB (tokio-rt is larger when statically linked)

---

## 2. Current Binary Analysis

### 2.1 Runtime Dependencies (glibc build)

```bash
$ ldd target/release/bastion-worker
    linux-vdso.so.1 (0x...)
    libgcc_s.so.1 => /lib64/libgcc_s.so.1
    libm.so.6 => /lib64/libm.so.6
    libc.so.6 => /lib64/libc.so.6
    /lib64/ld-linux-x86-64.so.2
```

**Required libraries:**
| Library | Purpose | Can be static? |
|---------|---------|----------------|
| linux-vdso | Kernel virtual DSO (always present) | N/A (kernel) |
| libgcc_s | GCC support library | YES (bundled) |
| libm | Math library | YES (bundled) |
| libc | C library (glibc) | **NO** - requires glibc |

### 2.2 Binary Size Comparison

| Build Type | Size | Linked Against |
|------------|------|----------------|
| Debug | 79MB | glibc (dynamic) |
| Release (glibc) | 2.5MB | glibc (dynamic) |
| Release (musl, projected) | ~4-6MB | musl (static) |

### 2.3 Key Dependencies Analysis

From `Cargo.toml` workspace dependencies:

```toml
# Async runtime — FULLY STATIC COMPATIBLE
tokio = { version = "1.52", features = ["rt-multi-thread", "macros", "sync", "time", "io-util", "process", "fs"] }

# gRPC transport — CRITICAL ISSUE
tonic = { version = "0.14", features = ["tls-native-roots", "gzip"] }
#                    ^^^^^^^^^^^^^^^^^^^ PROBLEM: Uses OpenSSL (not musl-compatible)

# Alternative (RECOMMENDED):
# tonic = { version = "0.14", features = ["tls", "gzip"] }  # Use rustls instead

# Security — STATIC COMPATIBLE
hmac = "0.12"
sha2 = "0.10"  # Uses ring or openssl-sys depending on features

# Serialization — STATIC COMPATIBLE
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 2.4 Does bastion-worker need libc? Which one?

**Answer**: It needs a C library at runtime. Currently linked against **glibc** (libc.so.6).

- **Alpine Linux**: Uses **musl** (not glibc). A glibc binary will NOT work on Alpine.
- **Distroless**: Uses **musl** or just contains static binaries
- **scratch**: Contains NO libraries — only works with fully static binaries
- **Firecracker VM**: Uses **musl** (buildroot-based rootfs)
- **gVisor**: Uses a custom rootfs, typically based on **busybox + musl**

---

## 3. Backend-Specific Requirements

### 3.1 Podman/Docker (Current Approach: Bind-Mount)

```rust
// From bastion-infrastructure/src/provider/podman.rs:219
binds.push(format!(
    "{}:/usr/local/bin/bastion-worker:ro",
    self.worker_binary.display()
));
```

| Aspect | Details |
|---------|---------|
| **Injection Method** | Host binary bind-mounted into container |
| **Container Image** | Any image with `/usr/local/bin` writable |
| **Binary Format** | glibc or musl — doesn't matter (host provides libs) |
| **Startup Time** | ~100ms (binary already on host) |
| **Pros** | Simple, works with development builds |
| **Cons** | Fails on read-only rootfs, Kubernetes without hostPath, MicroVMs |

### 3.2 Kubernetes

| Aspect | Details |
|---------|---------|
| **Options** | Init container (download), ConfigMap (small binary), hostPath (dev only) |
| **Image Approach** | Multi-stage build or pre-built worker image |
| **Binary Format** | Must be **static musl** for distroless scratch images |
| **Startup Time** | Depends on image size (100ms - 2s) |

### 3.3 Firecracker MicroVM

```rust
// From bastion-infrastructure/src/provider/firecracker.rs:398
let worker_dest = mount_point.join("usr/local/bin/bastion-worker");
std::fs::copy(&self.worker_binary, &worker_dest)
```

| Aspect | Details |
|---------|---------|
| **Injection Method** | Binary copied into rootfs at VM start |
| **Rootfs Type** | Usually buildroot-based, uses **musl** |
| **Binary Format** | **MUST be static musl** |
| **Startup Time** | ~50ms (binary embedded in rootfs) |
| **Cons** | Binary baked into rootfs image at runtime |

### 3.4 gVisor (runsc)

```rust
// From bastion-infrastructure/src/provider/gvisor.rs:219
let worker_dest = rootfs_dest.join("usr/local/bin/bastion-worker");
std::fs::copy(&self.worker_binary, &worker_dest)
```

| Aspect | Details |
|---------|---------|
| **Injection Method** | Binary copied into OCI rootfs bundle |
| **Rootfs Type** | Custom gVisor rootfs, typically **musl** |
| **Binary Format** | **MUST be static musl** |
| **Cons** | Binary must be available at bundle creation time |

### 3.5 AWS Lambda / FaaS

| Aspect | Details |
|--------|---------|
| **Runtime** | Custom runtime or embedded in Lambda runtime |
| **Binary Format** | **MUST be static musl** |
| **Bootstrap** | Lambda provides `/var/runtime/bootstrap` — worker must replace this |
| **Size Limit** | 50MB compressed (Lambda), 250MB (Fargate) |

---

## 4. Distribution Strategies Analysis

### 4.1 Option 1: Static Rust Binary (musl target)

**Build command**:
```bash
rustup target add x86_64-unknown-linux-musl
cargo build --target x86_64-unknown-linux-musl --release -p bastion-worker
```

**Pros**:
- ✅ Single binary, no external dependencies
- ✅ Works on Alpine, distroless, scratch, Firecracker, gVisor, Lambda
- ✅ ~4-6MB size is acceptable
- ✅ Easiest deployment (just copy file)

**Cons**:
- ❌ **CRITICAL**: `tls-native-roots` feature uses OpenSSL which doesn't support musl
- ❌ Need `musl-gcc` linker (`apt install musl-tools` on Ubuntu)
- ❌ Some crates (especially TLS) don't support static musl linking
- ❌ Build time increases (~30%)

**Required Change**:
```toml
# Change from:
tonic = { version = "0.14", features = ["tls-native-roots", "gzip"] }

# To:
tonic = { version = "0.14", features = ["tls", "gzip"] }  # Uses rustls instead
tokio-rustls = "0.26"  # Already in workspace
rustls-pemfile = "2.0"  # Already in workspace
```

### 4.2 Option 2: Shell Bootstrap (wget/curl)

**Entry point script**:
```bash
#!/bin/sh
WORKER_URL="${WORKER_URL:-http://gateway:50052/worker/bastion-worker}"
if [ ! -f /usr/local/bin/bastion-worker ]; then
    curl -fL "$WORKER_URL" -o /usr/local/bin/bastion-worker
    chmod +x /usr/local/bin/bastion-worker
fi
exec /usr/local/bin/bastion-worker "$@"
```

**Pros**:
- ✅ Tiny bootstrap (few hundred bytes)
- ✅ Works with any container (Alpine, busybox, etc.)
- ✅ Easy version updates (change URL)
- ✅ Compressed download possible

**Cons**:
- ❌ **Network dependency** at startup — fails if gateway unreachable
- ❌ Requires curl or wget in base image
- ❌ Security: must verify binary integrity (TLS + signature)
- ❌ Startup latency: 200ms-2s depending on binary size and network
- ❌ Adds failure modes during cold starts

### 4.3 Option 3: Multi-stage Docker Build

**Dockerfile**:
```dockerfile
# Stage 1: Build
FROM rust:1.85 AS builder
WORKDIR /build
COPY . .
RUN apt-get install -y musl-tools && \
    rustup target add x86_64-unknown-linux-musl && \
    cargo build --target x86_64-unknown-linux-musl --release -p bastion-worker

# Stage 2: Runtime
FROM alpine:3.19
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/bastion-worker /usr/local/bin/
ENTRYPOINT ["bastion-worker"]
```

**Pros**:
- ✅ Self-contained image with worker pre-installed
- ✅ Fastest startup (no download needed)
- ✅ Can use musl for smaller base image

**Cons**:
- ❌ Larger image size (base + worker = ~15MB vs 4MB)
- ❌ Version updates require rebuilding image
- ❌ Build time added to CI/CD pipeline
- ❌ Multiple images needed for multiple architectures

### 4.4 Option 4: Pre-built Custom Image

**Images needed**:
- `bastion-worker:musl-latest` — Alpine-based, latest worker
- `bastion-worker:musl-v0.1.0` — Pinned version
- `bastion-worker:distroless-latest` — distroless/base

**Pros**:
- ✅ Fastest startup (no build, no download)
- ✅ Version controlled via image tags
- ✅ sha256 digest for integrity

**Cons**:
- ❌ Version management complexity
- ❌ Multiple images per architecture
- ❌ CI/CD pipeline needs to push on every release
- ❌ Registry storage costs

### 4.5 Option 5: Compressed + Chunked Download (Jenkins agent.jar style)

**Bootstrap approach**:
1. Download small bootstrap (~50KB)
2. Bootstrap downloads compressed worker (~1MB gzipped)
3. Decompress and execute

**Jenkins comparison**:
- agent.jar: 170KB (contains bootstrap + protocol logic)
- Full remoting: downloaded separately

**Pros**:
- ✅ Fast initial bootstrap
- ✅ Can verify integrity before executing
- ✅ Compression reduces download time

**Cons**:
- ❌ Requires decompression library (gzip/zstd)
- ❌ Two-step process adds latency
- ❌ Bootstrap still needs to handle updates

---

## 5. Jenkins agent.jar Lessons

From `docs/research/jenkins-remoting-analysis.md`:

### 5.1 How Jenkins Does It

```
┌──────────┐                      ┌──────────────┐
│  Agent   │                      │  Controller  │
└────┬─────┘                      └──────┬───────┘
     │   1. HTTP GET /jnlpJars/agent.jar     │
     │ ─────────────────────────────────────►│
     │   ← agent.jar (170KB download)         │
     │◄────────────────────────────────────── │
     │
     │   2. java -jar agent.jar -url ... -secret ...
     │   ══► TCP connect to controller:PORT
     │   ══► TLS handshake + auth
```

### 5.2 Key Insights

1. **Small bootstrap is key**: agent.jar is tiny (170KB) because it only contains:
   - Bootstrap code to download protocol handler
   - Basic HTTP client
   - Protocol handshake logic

2. **No two-step download for Bastion**: We have a single binary, not a JVM with classloading

3. **TLS is non-negotiable**: Jenkins uses TLS for the agent connection (JNLP4-connect)

4. **Capability negotiation happens in handshake**: Both sides advertise supported features

### 5.3 Recommended Pattern for Bastion

For **Lambda/FaaS** where bootstrap size matters:

```
┌─────────────────────────────────────────────────┐
│  Bootstrap (shell script, < 1KB)                │
│  - Download worker binary from well-known URL   │
│  - Verify sha256 signature                      │
│  - Execute worker                                │
└─────────────────────────────────────────────────┘
```

For **Kubernetes/MicroVMs** where startup time matters:

```
┌─────────────────────────────────────────────────┐
│  Pre-built image: bastion-worker:latest        │
│  - Worker binary pre-installed                 │
│  - Multi-arch support via manifest             │
│  - sha256 digest in image config              │
└─────────────────────────────────────────────────┘
```

---

## 6. Minimum Container Images

### 6.1 What the Worker Actually Needs

Based on `main.rs` analysis, the worker:
- Uses **tokio** async runtime (needs threads, scheduling)
- Opens **outbound TCP connection** to gateway
- Spawns **child processes** via `tokio::process::Command`
- Reads/writes **files** in `/workspace`, `/tmp`, `/home`, `/opt`, `/var/tmp`
- Reads `/proc/meminfo`, `/proc/loadavg`, `/proc/uptime` for health
- **No** requirement for: X11, sound, GPUs, USB, etc.

### 6.2 Image Recommendations

| Base Image | Size | libc | Notes |
|------------|------|------|-------|
| `scratch` | 0KB | none | ❌ Cannot run (needs /proc) |
| `busybox:stable` | 4MB | musl | ⚠️ Limited, needs adding /proc |
| `alpine:3.19` | 10MB | musl | ✅ Works with musl binary |
| `distroless/base` | 20MB | musl | ✅ Minimal, no shell |
| `debian:stable-slim` | 80MB | glibc | ✅ Works with glibc binary |

### 6.3 Minimum Image for bastion-worker

```dockerfile
# Minimal Alpine-based worker image
FROM alpine:3.19
RUN apk add --no-cache ca-certificates tzdata
COPY bastion-worker /usr/local/bin/
ENTRYPOINT ["bastion-worker"]
# Size: ~12MB with worker
```

```dockerfile
# Distroless (no shell, even smaller)
FROM gcr.io/distroless/base:nonroot
COPY bastion-worker /
ENTRYPOINT ["/bastion-worker"]
# Size: ~8MB with worker
```

---

## 7. Recommended Architecture

### 7.1 Strategy: Hybrid Approach

**For Podman/Docker (development)**:
- Bind-mount from host (current approach)
- Binary can be glibc or musl

**For Kubernetes (production)**:
- **Primary**: Pre-built image `bastion-worker:musl-{version}`
- **Alternative**: Init container that downloads from ConfigMap

**For Firecracker/gVisor (MicroVMs)**:
- **Required**: Static musl binary pre-injected into rootfs
- Build rootfs with worker binary baked in at deployment time

**For Lambda/FaaS**:
- Custom runtime wrapper
- Bootstrap downloads worker from S3/internal artifact store

### 7.2 Critical Path: Enable Full Static Linking

**Step 1: Switch TLS backend from OpenSSL to rustls**

```toml
# Cargo.toml (bastion-worker)
tonic = { version = "0.14", features = ["tls", "gzip"] }  # Remove tls-native-roots
# rustls is pure Rust, no C library dependencies
```

**Step 2: Add musl target and verify build**

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --target x86_64-unknown-linux-musl --release -p bastion-worker
```

**Step 3: Verify static linking**

```bash
$ ldd target/x86_64-unknown-linux-musl/release/bastion-worker
# Should output "not a dynamic executable" or similar for fully static
```

### 7.3 Implementation Sketch

```rust
// In bastion-infrastructure/src/provider/firecracker.rs
impl FirecrackerProvider {
    /// Inject worker binary into rootfs.
    /// The binary MUST be static musl for this to work.
    async fn inject_worker(&self, mount_point: &Path) -> Result<(), DomainError> {
        let worker_dest = mount_point.join("usr/local/bin/bastion-worker");

        // Verify binary is static musl
        let worker_path = &self.worker_binary;
        let output = Command::new("file")
            .arg(worker_path)
            .output()
            .await
            .map_err(|e| DomainError::Internal(format!("file command failed: {e}")))?;

        let file_output = String::from_utf8_lossy(&output.stdout);
        if !file_output.contains("musl") && !file_output.contains("static") {
            tracing::warn!(
                "Worker binary may not be static musl: {}",
                file_output
            );
        }

        std::fs::copy(&self.worker_binary, &worker_dest)
            .map_err(|e| DomainError::Internal(format!("Failed to copy worker: {e}")))?;
        Ok(())
    }
}
```

---

## 8. Open Questions

1. **Version pinning**: How do we handle worker version mismatches with gateway?
   - Protocol version already negotiated in handshake (good!)
   - Need to decide: fail fast or attempt protocol adaptation?

2. **Binary signing**: Should we sign worker binaries?
   - For FaaS: strongly recommended (S3 object versioning + signature verification)
   - For MicroVMs: less critical (rootfs is built by us)

3. **Multi-arch support**: Do we need ARM64 (aarch64) worker?
   - Firecracker supports ARM64
   - Lambda supports aarch64 (Graviton)
   - Multi-arch builds increase CI complexity

4. **Gateway as artifact server**: Should gateway serve worker downloads?
   - Currently: separate artifact storage
   - Jenkins style: controller serves agent.jar
   - Tradeoff: coupling vs simplicity

---

## 9. Conclusion

### Short-term (Current)
The bind-mount approach works for Podman development but **fails for production MicroVMs and gVisor**.

### Medium-term (This Sprint)
1. **Switch tonic from OpenSSL to rustls** (`features = ["tls", "gzip"]`)
2. **Add musl target support** (`rustup target add x86_64-unknown-linux-musl`)
3. **Verify static linking** with `ldd` and `file` commands
4. **Update providers** to require musl binary (gvisor, firecracker)

### Long-term (Future)
1. Publish `bastion-worker` images to registry (GHCR or custom)
2. Add bootstrap script option for Lambda/FaaS
3. Consider Jenkins-style version negotiation for gateway→worker

---

## Appendix: Quick Test Commands

```bash
# Check if binary is static
ldd target/release/bastion-worker 2>&1 | grep "not a dynamic" || echo "Dynamic linking detected"

# Check binary format
file target/release/bastion-worker

# Verify musl target builds
rustup target add x86_64-unknown-linux-musl
cargo build --target x86_64-unknown-linux-musl --release -p bastion-worker

# Check musl build output
ls -lh target/x86_64-unknown-linux-musl/release/bastion-worker
ldd target/x86_64-unknown-linux-musl/release/bastion-worker 2>&1 || echo "Static (no ldd output)"
```
