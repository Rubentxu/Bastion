# 🏰 Bastion

<div align="center">

**MCP Gateway for Sandboxed AI Agent Execution**

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg?logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-green.svg)](LICENSE)
[![CI](https://github.com/Rubentxu/Bastion/actions/workflows/ci.yml/badge.svg)](https://github.com/Rubentxu/Bastion/actions/workflows/ci.yml)
[![Version](https://img.shields.io/badge/version-0.1.0-blue.svg)](https://github.com/Rubentxu/Bastion/releases)

</div>

---

**Bastion** is an open-source [MCP](https://spec.modelcontextprotocol.io/) (Model Context Protocol) Gateway that enables AI agents to safely execute tools in isolated sandbox environments — containers, microVMs, or kernel-level sandboxes. Built in Rust with Domain-Driven Design (DDD) and Clean Architecture.

## 📖 Table of Contents

- [Why Bastion?](#-why-bastion)
- [Architecture](#-architecture)
- [Features](#-features)
- [Quick Start](#-quick-start)
- [Usage](#-usage)
  - [With OpenCode](#with-opencode)
  - [With Claude Code](#with-claude-code)
  - [CLI Options](#cli-options)
- [MCP Tools](#-mcp-tools)
- [Architecture Deep Dive](#-architecture-deep-dive)
  - [DDD Crate Structure](#ddd-crate-structure)
  - [Data Flow](#data-flow)
- [Roadmap](#-roadmap)
- [Development](#-development)
- [Contributing](#-contributing)
- [License](#-license)

## 🤔 Why Bastion?

AI agents need to run code, but running untrusted code directly on the host is dangerous. Existing MCP servers typically execute commands in the same process or machine — no isolation, no resource limits, no cleanup.

**Bastion solves this by providing an MCP-compatible gateway that acts as a secure intermediary:**

```
Agent (MCP Client)
    │
    │  tools/call("sandbox_run", {command: "npm test"})
    ▼
Bastion Gateway ──▶ Sandbox Container (Podman/Firecracker/gVisor)
    │                      │
    │  {exit_code: 0,       │  npm test
    │   stdout: "42 passed"}│  runs in isolation
    ▼                      ▼
```

- **Isolation**: Every command runs in its own container or microVM
- **Resource control**: CPU, memory, and time limits per sandbox
- **Clean slate**: No state leaks between executions
- **MCP native**: Works with any MCP-compatible client (OpenCode, Claude Code, Goose, etc.)
- **Provider abstraction**: Swap backends without changing agent code

## 🏗 Architecture

```
┌──────────────┐                         ┌──────────────────────────────────────┐
│  MCP Client  │──tools/call──▶┌─────┐   │ Worker (gRPC CLIENT)                 │
│ (OpenCode,   │                │     │   │  ┌────────┐  ┌────────┐  ┌────────┐ │
│  Claude Code,│◀──responses───│     │◀──┼──│Sandbox1│  │Sandbox2│  │SandboxN│ │
│  Goose...)   │               │ G   │   │  │┌──────┐│  │┌──────┐│  │┌──────┐│ │
└──────────────┘               │ A   │   │  ││worker││  ││worker││  ││worker││ │
                               │ T   │   │  ││(bin) ││  ││(bin) ││  ││(bin) ││ │
                               │ E   │   │  └┴──────┴┘  └┴──────┴┘  └┴──────┴┘ │
                               │ W   │   │     ▲ PodmanProvider                  │
                               │ A   │   │     │ bind-mounts binary              │
                               │ Y   │   │     │ :50052 outbound                 │
                               │     │   └─────┼─────────────────────────────────┘
                               │ :50052       │
                               │ gRPC Registry│
                               │ + MCP srv   │
                               └─────┼───────┘
                                     │
                                     ▼
                          ┌─────────────────────┐
                          │ ProviderFactory     │
                          │ ┌─────────────────┐ │
                          │ │ PodmanProvider   │ │
                          │ ├─────────────────┤ │
                          │ │ FirecrackerProvider │
                          │ └─────────────────┘ │
                          └─────────────────────┘
```

### Worker Protocol v2 (JNLP-inspired)

Workers connect **outbound** to the Gateway — no port mapping, no inbound firewall rules. The life cycle:

1. **Register** — Worker sends `sandbox_id`, protocol version, capabilities, and a random nonce
2. **ChallengeResponse** — Gateway responds with a challenge nonce; Worker proves identity via HMAC-SHA256(secret, worker_nonce || gateway_nonce). Secret never transits the wire.
3. **CommandStream** — Bidirectional streaming: Gateway sends commands, Worker streams stdout/stderr/exit/health back

Reliability: exponential backoff reconnect (1s→60s + jitter), 10s heartbeat ping/pong, 30s watchdog dead-worker cleanup, and circuit breaker (3 failures → 30s open).

![Bastion Architecture](docs/assets/diagrama.png)

## ✨ Features

| Feature | Status | Description |
|---------|--------|-------------|
| **Worker Protocol v2** | ✅ Stable | gRPC-based JNLP protocol: Register→HMAC auth→CommandStream; outbound workers |
| **Podman Backend** | ✅ Stable | Container-based isolation via bollard Docker API |
| **Firecracker Backend** | ✅ Implemented | microVM isolation via Firecracker REST API over Unix socket |
| **Streaming Execution** | ✅ Stable | Real-time stdout/stderr streaming during commands |
| **Hot Pool Manager** | ✅ Stable | Pre-warm containers for <200ms sandbox creation |
| **Provider Abstraction** | ✅ Stable | ProviderFactory — swap backends via config |
| **Prometheus Metrics** | ✅ Stable | Sandbox counts, command latency, error rates |
| **Health Checks** | ✅ Stable | Provider + pool connectivity validation |
| **gVisor Backend** | 🔜 Planned | Kernel-level sandboxing via runsc |
| **Kubernetes Backend** | 🔜 Planned | Pod-based ephemeral sandboxes |

## 🚀 Quick Start

### Prerequisites

- **Rust** 1.80+ ([install](https://rustup.rs))
- **Podman** 4.x+ ([install](https://podman.io/docs/installation))

### 1. Clone and Build

```bash
git clone https://github.com/Rubentxu/Bastion.git
cd Bastion
cargo build --release
```

### 2. Start Podman Service

```bash
# Create socket directory and start the API service
mkdir -p $XDG_RUNTIME_DIR/podman
podman system service --time 3600 unix://$XDG_RUNTIME_DIR/podman/podman.sock &
```

### 3. Run the Gateway

```bash
# Basic mode
./target/release/bastion-gateway \
  --image debian:bookworm-slim

# With hot pool (recommended for production)
./target/release/bastion-gateway \
  --image debian:bookworm-slim \
  --pool-enabled \
  --pool-min-idle 2 \
  --pool-max-idle 5
```

### 4. Connect an MCP Client

Configure your MCP client to use the Bastion gateway. See [Usage](#-usage) for client-specific configurations.

## 📝 Usage

### With OpenCode

Add to `~/.config/opencode/config.toml`:

```toml
[[mcp_servers]]
name = "bastion"
command = "/path/to/bastion/target/release/bastion-gateway"
args = [
    "--pool-enabled",
    "--image", "debian:bookworm-slim"
]
```

Then use in any OpenCode session:

```
/sandbox_create template="debian:bookworm-slim"
/sandbox_run sandbox_id="abc123" command="python -c 'print(2+2)'"
/sandbox_read sandbox_id="abc123" path="/tmp/output.txt"
/sandbox_terminate sandbox_id="abc123"
```

### With Claude Code

Add to Claude Code's MCP config:

```json
{
  "mcpServers": {
    "bastion": {
      "command": "/path/to/bastion/target/release/bastion-gateway",
      "args": [
        "--pool-enabled",
        "--image", "debian:bookworm-slim"
      ]
    }
  }
}
```

### CLI Options

```
bastion-gateway [OPTIONS]

Sandbox Configuration:
  --socket <PATH>      Podman socket path [default: /run/user/1000/podman/podman.sock]
  --image <IMAGE>      Default container image [default: debian:bookworm-slim]
  --config <PATH>      Configuration file path [default: config/sandbox-gateway.toml]

Pool Options:
  --pool-enabled               Enable sandbox pooling
  --pool-min-idle <N>          Min idle containers per template [default: 2]
  --pool-max-idle <N>          Max idle containers per template [default: 5]
  --pool-max-total <N>         Max total containers [default: 50]
  --pool-idle-timeout-ms <MS>  Idle eviction timeout [default: 600000]
  --pool-refill-interval-ms <MS> Pool refill interval [default: 5000]
```

## 🔧 MCP Tools

Bastion exposes 12 MCP tools for sandbox management:

### Lifecycle

| Tool | Parameters | Returns |
|------|------------|---------|
| `sandbox_create` | `template`, `timeout_ms` | `sandbox_id`, `status`, `from_pool` |
| `sandbox_terminate` | `sandbox_id` | `status` (`terminated` or `pooled`) |
| `sandbox_info` | `sandbox_id` | `sandbox_id`, `status`, `template`, `created_at`, `expires_at` |
| `sandbox_list` | — | `count`, `sandboxes[]` |

### Execution

| Tool | Parameters | Returns |
|------|------------|---------|
| `sandbox_run` | `sandbox_id`, `command` | `exit_code`, `stdout`, `stderr`, `duration_ms` |
| `sandbox_run_stream` | `sandbox_id`, `command` | `exit_code`, `stdout`, `stderr`, `chunks_received` |

### File Operations

| Tool | Parameters | Returns |
|------|------------|---------|
| `sandbox_write` | `sandbox_id`, `path`, `content` | `status` |
| `sandbox_read` | `sandbox_id`, `path` | `content`, `encoding` |
| `sandbox_list_files` | `sandbox_id`, `path` | `count`, `entries[]` |

### Observability

| Tool | Parameters | Returns |
|------|------------|---------|
| `sandbox_health` | — | `status`, `version`, `checks[]` |
| `sandbox_metrics` | — | Prometheus-formatted metrics |
| `sandbox_pool_stats` | — | `enabled`, `active`, `idle`, `templates[]` |

## 🧬 Architecture Deep Dive

### DDD Crate Structure

| Crate | Layer | Responsibility |
|-------|-------|---------------|
| `bastion-domain` | Domain | Entities, value objects, traits (`SandboxProvider`, `CommandRouter`, `SandboxRepository`) |
| `bastion-application` | Application | Use cases (orchestration between domain and infrastructure) |
| `bastion-infrastructure` | Infrastructure | Adapters (`PodmanProvider`, `FirecrackerProvider`, `InMemoryRepo`, `PoolManager`, `Metrics`) |
| `bastion-gateway` | Presentation | MCP server via `rmcp`, gRPC `RegistryService` on `:50052`, composition root, CLI |
| `bastion-worker` | Worker | gRPC CLIENT connecting outbound to Gateway; runs inside sandbox (JNLP pattern) |

### Data Flow

```
┌──────────────┐     ┌─────────────────────────┐     ┌──────────────────┐
│  MCP Client  │────▶│  BastionGateway          │────▶│   Use Cases      │
│ (OpenCode,   │     │  (rmcp server + gRPC)    │     │ (Application)    │
│  Claude Code)│◀────│  12 tool handlers        │◀────│                  │
└──────────────┘     │  RegistryService :50052  │     └────────┬─────────┘
                     └───┬──────────┬───────────┘              │
                         │          │                          ▼
                         │ gRPC     │              ┌──────────────────┐
                         ▼          ▼              │ SandboxRepository │
              ┌─────────────┐ ┌────────────────┐   │   (InMemory)      │
              │ Worker      │ │ ProviderFactory │   └──────────────────┘
              │ (gRPC CLIENT)│ │  (Podman,       │
              │ in sandbox   │ │   Firecracker)  │
              └─────────────┘ └────────────────┘
                   ▲ bind-mount worker binary
                   │
            PodmanProvider
```

## 🗺 Roadmap

| Version | Milestone | Content |
|---------|-----------|---------|
| **v0.1.0** ✅ | MVP | Podman backend, 12 tools, hot pool, streaming, metrics |
| **v0.2.0** | Multi-backend | Pool Manager, Firecracker backend |
| **v0.3.0** | Multi-backend | gVisor backend, provider selection |
| **v0.4.0** | Streaming | MCP progress notifications, cancellation |
| **v0.5.0** | Pipelines | DSL-based multi-sandbox pipelines |
| **v0.6.0** | Database | PostgreSQL + SQLite sandbox backends |
| **v0.9.0** | Kubernetes | K8s Pod-based ephemeral sandboxes |
| **v1.0.0** | Stable | All features, stable API, crates.io release |

See [CHANGELOG.md](CHANGELOG.md) for detailed release notes.

## 💻 Development

```bash
# Build
cargo build --release

# Run all tests
cargo test --workspace

# Run integration tests (requires Podman)
cargo test --test podman_lifecycle -- --test-threads=1

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all -- --check

# Generate docs
cargo doc --no-deps --document-private-items --open
```

### Project Structure

```
Bastion/
├── crates/
│   ├── bastion-domain/         # Domain model + ports
│   │   └── src/
│   │       ├── sandbox/        # Sandbox aggregate
│   │       ├── execution/      # Command + streaming types
│   │       ├── provider/       # SandboxProvider trait
│   │       └── shared/         # DomainError, Id types
│   ├── bastion-application/    # Use cases
│   │   └── src/
│   │       ├── sandbox/        # Create, terminate, list, info
│   │       ├── execution/      # Run, run_stream
│   │       └── file_ops/       # Read, write, list_files
│   ├── bastion-infrastructure/ # Adapters
│   │   └── src/
│   │       ├── provider/       # PodmanProvider, ProviderFactory
│   │       ├── pool/           # SandboxPoolManager
│   │       ├── persistence/    # InMemorySandboxRepository
│   │       ├── metrics/        # GatewayMetrics
│   │       └── config/         # Config loader
│   ├── bastion-gateway/        # MCP server
│   │   └── src/
│   │       ├── main.rs         # Composition root + CLI
│   │       └── server.rs       # 12 MCP tool handlers
│   └── bastion-worker/         # gRPC worker (outbound JNLP client)
│       └── src/
│           └── main.rs         # Connect→Register→ChallengeResponse→CommandStream
├── docs/assets/                # Documentation images
├── config/                     # Example configs
├── proto/                      # Protobuf definitions
└── proyectos/                  # Planning docs (Spanish)
```

## 🤝 Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on:

- Architecture and design principles
- Code style and conventions
- Commit message format
- Pull request checklist

## 📄 License

Apache-2.0 — see [LICENSE](LICENSE) for details.

---

<div align="center">

**Built with Rust** 🦀 **·** **DDD** 🧬 **·** **MCP** 🔌

[🇪🇸 Leer en español](README.es.md)

</div>
