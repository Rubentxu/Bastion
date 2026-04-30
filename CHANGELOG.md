# Changelog

## [Unreleased] — 2026-04-30

### Added — Bastion Worker Protocol v2 (JNLP-inspired)
- Worker as gRPC CLIENT connecting outbound to Gateway (JNLP pattern)
- 3-phase authentication: Register → HMAC-SHA256 ChallengeResponse → CommandStream
- Auto-reconnect with exponential backoff + jitter (1s→60s)
- Heartbeat mechanism: 10s PingRequest/PongResponse
- Watchdog: removes dead workers every 30s
- Circuit breaker: 3 consecutive failures → 30s open state
- Graceful shutdown with GraceLevel (Graceful/Draining/Forceful)
- Real health metrics from /proc (meminfo, loadavg, uptime)
- HMAC-SHA256 challenge-response authentication (secret never transits wire)
- Path traversal protection (allowlist: /workspace, /tmp, /home, /opt, /var/tmp)
- Per-sandbox token bucket rate limiting (20 burst, 10 req/s)
- Structured audit trail
- Optional TLS via --tls-cert/--tls-key
- gRPC gzip compression (60-80% bandwidth reduction)
- File chunking (4MB chunks, no OOM on large files)
- Concurrent command execution (Semaphore(4) + tokio::spawn)
- MUSL static binary support (.cargo/config.toml, scripts/build-worker.sh)
- HTTP/2 tuning: adaptive window, 1MB stream, 4MB connection
- CommandRouter trait in bastion-domain breaks infrastructure↔gateway cycle

### Changed
- Worker is now gRPC CLIENT (was: gRPC server with port mapping)
- Gateway now runs gRPC Registry SERVER on :50052 (was: only MCP server)
- PodmanProvider: bind-mounts worker binary (was: tar+upload)
- Proto package `bastion.worker.v2` with Register, ChallengeResponse, CommandStream services

### Removed
- `crates/bastion-worker/src/worker.rs` (replaced by main.rs worker protocol)
- Port-mapping approach for container communication

---

## [0.1.0] — 2026-04-30

### Added
- DDD workspace with 5 crates (domain, application, infrastructure, gateway, worker)
- `SandboxProvider` trait with Podman backend via bollard and Firecracker backend via REST API
- 12 MCP tools: sandbox_create, sandbox_run, sandbox_run_stream, sandbox_write, sandbox_read, sandbox_list_files, sandbox_list, sandbox_terminate, sandbox_info, sandbox_pool_stats, sandbox_health, sandbox_metrics
- Streaming command execution (stdout/stderr in real-time via CommandChunk)
- SandboxPoolManager with hot pool, background refill, and eviction
- ProviderFactory for multi-provider management
- In-memory SandboxRepository
- Structured logging via tracing
- Prometheus metrics (sandbox counts, command latency, errors)
- Integration tests for Podman lifecycle (5 tests)
- CLI configuration via clap

### Infrastructure
- Podman rootless socket support
- Configurable pool manager (min/max idle, refill interval)
- Health check endpoint
- Prometheus-formatted metrics export
