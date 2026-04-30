# Changelog

## [0.1.0] — 2026-04-30

### Added
- DDD workspace with 5 crates (domain, application, infrastructure, gateway, worker)
- `SandboxProvider` trait with Podman backend via bollard
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
