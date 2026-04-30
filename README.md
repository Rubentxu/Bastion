# Bastion — MCP Gateway for Sandboxed AI Agent Execution

[![Rust](https://img.shields.io/badge/rust-stable-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-green.svg)](LICENSE)

**Bastion** is an open-source MCP (Model Context Protocol) Gateway that lets AI agents safely execute tools in isolated sandbox environments. Built in Rust with Domain-Driven Design (DDD) and Clean Architecture.

## Architecture

![Bastion Architecture](docs/assets/diagrama.png)

## Features

- ✅ **12 MCP Tools** — Create, manage, and destroy sandboxes via MCP protocol
- ✅ **Podman Backend** — Container-based isolation via bollard API
- ✅ **Streaming Execution** — Real-time stdout/stderr streaming during command execution
- ✅ **Hot Pool Manager** — Pre-warm containers for <200ms sandbox creation
- ✅ **Multi-Provider** — ProviderFactory for dynamic backend selection
- ✅ **DDD Architecture** — Clean separation: domain, application, infrastructure, gateway
- ✅ **Observability** — Health checks + Prometheus metrics + structured logging
- 🔜 **Firecracker Backend** — microVM isolation (planned)
- 🔜 **gVisor Backend** — Kernel-level isolation (planned)

## Quick Start

### Prerequisites
- Rust 1.80+
- Podman 4.x+

### Build
```bash
cargo build --release
```

### Start Podman Service
```bash
mkdir -p $XDG_RUNTIME_DIR/podman
podman system service --time 3600 unix://$XDG_RUNTIME_DIR/podman/podman.sock &
```

### Run Gateway
```bash
./target/release/bastion-gateway \
  --socket /run/user/1000/podman/podman.sock \
  --image debian:bookworm-slim
```

### With Pool Enabled
```bash
./target/release/bastion-gateway \
  --pool-enabled \
  --pool-min-idle 2 \
  --pool-max-idle 5
```

### Test with OpenCode (or any MCP client)
Configure your MCP client to point to the bastion-gateway binary:
```json
{
  "mcpServers": {
    "bastion": {
      "command": "./target/release/bastion-gateway",
      "args": ["--pool-enabled"]
    }
  }
}
```

## Crate Structure

| Crate | Purpose |
|-------|---------|
| `bastion-domain` | Domain types, traits (SandboxProvider, SandboxRepository) |
| `bastion-application` | Use cases (orchestration logic) |
| `bastion-infrastructure` | Adapters (PodmanProvider, InMemoryRepo, PoolManager) |
| `bastion-gateway` | MCP server (rmcp), composition root |
| `bastion-worker` | gRPC worker runtime (TBD) |

## Development

```bash
# Run all tests
cargo test --workspace

# Run integration tests (requires Podman)
cargo test --test podman_lifecycle -- --test-threads=1

# Lint
cargo clippy --workspace

# Build docs
cargo doc --no-deps --document-private-items --open
```

## License

Apache-2.0 — see [LICENSE](LICENSE)
