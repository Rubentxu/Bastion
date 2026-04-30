# Bastion Architecture

## System Overview

Bastion is a sandbox orchestration system that lets AI agents execute tools in isolated environments — containers or microVMs — through an MCP-compatible gateway.

```
MCP Client (OpenCode, Claude Code, Goose...)
        │
        │ stdio (MCP JSON-RPC)
        ▼
┌──────────────────────────────────────┐
│  Bastion Gateway                     │
│  ┌────────────┐  ┌─────────────────┐ │
│  │ MCP Server │  │ RegistryService │ │
│  │ (rmcp)     │  │ (gRPC :50052)   │ │
│  └─────┬──────┘  └────────┬────────┘ │
│        │                  │          │
│        │     ┌────────────┘          │
│        ▼     ▼                       │
│  ┌──────────────────┐               │
│  │   Use Cases      │               │
│  │ (Application)    │               │
│  └────────┬─────────┘               │
└───────────┼─────────────────────────┘
            │
            ▼
  ┌──────────────────┐
  │ ProviderFactory  │
  │ ┌──────────────┐ │
  │ │ PodmanProvider│◀────────────────────────┐
  │ └──────┬───────┘ │                        │
  │ ┌──────┴───────┐ │   bind-mount worker    │
  │ │ Firecracker   │ │   binary into         │
  │ │ Provider      │ │   each sandbox        │
  │ └──────────────┘ │                        │
  └──────────────────┘                        │
                                               │
  ┌───────────────────────────────────────────┘
  ▼
┌─────────────────────────────────────────────────┐
│ Sandbox Environment                             │
│  ┌────────────────────────────────────────────┐ │
│  │ bastion-worker (gRPC CLIENT)               │ │
│  │  • Connects OUTBOUND to Gateway :50052     │ │
│  │  • One worker per sandbox                  │ │
│  │  • Executes commands locally               │ │
│  │  • Concurrent: Semaphore(4) + tokio::spawn │ │
│  └────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
```

## Worker Protocol v2 (JNLP-inspired)

Workers initiate outbound connections to the Gateway, eliminating the need for port mapping, firewall rules, or inbound orchestration. This follows the Jenkins Remoting / JNLP pattern.

### Connection Lifecycle

```
Worker                                      Gateway
  │                                            │
  │─────── Register (sandbox_id, nonce) ──────▶│  Phase 1
  │◀────── CHALLENGE (gateway_nonce) ─────────│
  │                                            │
  │──── ChallengeResponse(proof) ─────────────▶│  Phase 2
  │  proof = HMAC-SHA256(secret, W_nonce||G_nonce)
  │◀──── ACCEPTED (session_token, jwt) ───────│
  │                                            │
  │═══════ CommandStream (bidirectional) ══════│  Phase 3
  │  ← RunCommand, ReadFile, WriteFile, ...    │
  │  → StdoutChunk, ExitResult, Pong, ...      │
  │═════════════════════════════════════════════│
```

### Authentication (3-Phase)

1. **Register** — Worker sends sandbox_id, protocol version, capabilities, and a 32-byte random nonce
2. **ChallengeResponse** — Gateway replies with CHALLENGE status + its own 32-byte nonce; Worker computes `HMAC-SHA256(secret, worker_nonce || gateway_nonce)` and sends the proof. The secret never transits the wire.
3. **CommandStream** — On ACCEPTED, the Gateway returns a JWT session token valid for the session lifetime. The Worker opens a bidirectional gRPC stream for command delivery.

### Heartbeat & Health

- **10s Ping/Pong** — Gateway sends `PingRequest` every 10 seconds; Worker replies with `PongResponse` that piggybacks health metrics (CPU, memory, disk, loadavg, active commands) sourced from `/proc/meminfo`, `/proc/loadavg`, and `/proc/uptime`.
- **30s Watchdog** — The `RegistryService` scans registered workers every 30 seconds. Workers that missed 3 pings (30s of silence) are evicted. On reconnect, stale sessions are cleaned up.

### Reliability

| Mechanism | Behavior |
|-----------|----------|
| **Auto-reconnect** | Exponential backoff + random jitter: 1s → 2s → 4s → … → 60s max |
| **Circuit breaker** | 3 consecutive failures → 30s open state, then half-open probe |
| **Graceful shutdown** | GraceLevel: `Graceful` (wait for in-flight), `Draining` (no new, wait current), `Forceful` (kill immediately) |
| **File chunking** | 4MB max chunk size; large files streamed in chunks to avoid OOM |
| **gRPC compression** | Gzip on both send and accept; 60-80% bandwidth reduction on stdout/stderr |

## Component Responsibilities

### Gateway (`bastion-gateway`)

- **MCP Server** — Serves 12 MCP tools over stdio (rmcp). Translates tool calls into use case invocations.
- **RegistryService** — gRPC server on `:50052` implementing `WorkerRegistry`. Manages worker registration, authentication challenges, command routing, heartbeat monitoring, and dead-worker cleanup.
- **Composition root** — Wires `PodmanProvider` + `RegistryService` + `CommandRouter` + `SandboxPoolManager`.

### Worker (`bastion-worker`)

- **gRPC CLIENT** — Connects outbound to `--gateway-addr`. Runs inside every sandbox.
- **Command executor** — Receives `GatewayCommand` messages (run, read, write, list, ping, shutdown) and executes them in the sandbox's local filesystem.
- **Concurrency** — `Semaphore(4)` gates concurrent commands; each command is `tokio::spawn`'d so streaming responses don't block other commands.
- **Security** — Path traversal protection: allowlist restricts file ops to `/workspace`, `/tmp`, `/home`, `/opt`, `/var/tmp`.

### Provider (`bastion-infrastructure`)

- **PodmanProvider** — Creates containers via bollard Docker-compatible API. Bind-mounts the worker binary into each container so the worker can execute natively inside the sandbox.
- **FirecrackerProvider** — Creates microVMs via Firecracker REST API over Unix socket. Bakes the worker binary into the root filesystem image.
- **ProviderFactory** — Registry of named provider implementations. Default: `"podman"`.

### Domain (`bastion-domain`)

- **`SandboxProvider` trait** — Abstraction over container/VM backends. Defines `create`, `terminate`, `exec`, `read_file`, `write_file`, `list_files`, `ping`.
- **`CommandRouter` trait** — Decouples infrastructure from gateway. `PodmanProvider` calls `CommandRouter::route_command()` to send commands to the worker via the RegistryService, without importing `bastion-gateway`.
- **`SandboxRepository` trait** — Persistence abstraction for sandbox state. Currently `InMemorySandboxRepository`.

### Application (`bastion-application`)

- **Use cases** — `CreateSandbox`, `TerminateSandbox`, `RunCommand`, `ReadFile`, `WriteFile`, `ListFiles`, `GetSandboxInfo`, `ListSandboxes`, `HealthCheck`, `Metrics`, `PoolStats`.
- Orchestrates between domain traits and infrastructure adapters.

## Binary Distribution Strategies

The worker binary must be present inside each sandbox. Bastion supports three strategies:

| Strategy | How it works | Use case |
|----------|-------------|----------|
| **Bind-mount** | `PodmanProvider` bind-mounts the worker binary from the host into the container at `/usr/local/bin/bastion-worker` | Podman (default) |
| **Rootfs bake** | `FirecrackerProvider` bakes the worker into the rootfs image before booting the microVM | Firecracker |
| **Init container** | Kubernetes init container copies the worker binary into a shared volume | Kubernetes (planned) |

The worker binary is compiled as a **MUSL static binary** (~5-15MB) via `.cargo/config.toml`:
```toml
[target.x86_64-unknown-linux-musl]
linker = "x86_64-linux-musl-gcc"
```

Build with: `scripts/build-worker.sh`

## Security Model

| Layer | Mechanism | Detail |
|-------|-----------|--------|
| **Authentication** | HMAC-SHA256 challenge-response | Pre-shared secret. Secret never transits wire. `proof = HMAC(secret, W_nonce ∥ G_nonce)` |
| **Authorization** | JWT session token | Issued after successful registration, valid for 1h. Included in every `GatewayCommand`. |
| **Path traversal** | Allowlist validator | File operations restricted to: `/workspace`, `/tmp`, `/home`, `/opt`, `/var/tmp`. Relative paths and `..` traversal resolved and rejected. |
| **Rate limiting** | Token bucket per sandbox | 20 burst, 10 req/s steady. Prevents command flooding from compromised workers. |
| **Transport security** | Optional TLS | `--tls-cert` / `--tls-key` flags enable mTLS for the gRPC registry connection. |
| **Audit trail** | Structured logging | Every command execution, file operation, registration, and shutdown is logged with `tracing` (JSON format). Includes sandbox_id, command_id, timestamp, result. |

## Reliability Model

```
┌─────────────────────────────────────────────────────────────────┐
│                      Reliability Layers                          │
│                                                                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────────┐ │
│  │ Reconnect│  │ Heartbeat│  │ Watchdog │  │ Circuit Breaker │ │
│  │ 1s→60s   │  │ 10s ping │  │ 30s scan │  │ 3 fail→30s open │ │
│  │ + jitter │  │ + health │  │ dead→evict│  │                 │ │
│  └──────────┘  └──────────┘  └──────────┘  └─────────────────┘ │
│                                                                  │
│  ┌──────────────────┐  ┌────────────────┐  ┌──────────────────┐ │
│  │ Graceful Shutdown│  │ File Chunking  │  │ gRPC Compression │ │
│  │ Graceful/Drain/  │  │ 4MB chunks     │  │ gzip (60-80%)    │ │
│  │ Forceful          │  │ no OOM         │  │                  │ │
│  └──────────────────┘  └────────────────┘  └──────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## gRPC Service Definition

The protocol is defined in `proto/sandbox/v1/sandbox.proto` (package `bastion.worker.v2`):

```protobuf
service WorkerRegistry {
  rpc Register (RegisterRequest) returns (RegisterResponse);
  rpc ChallengeResponse (ChallengeProof) returns (RegisterResponse);
  rpc CommandStream (stream WorkerMessage) returns (stream GatewayCommand);
}
```

- **Register** — Initial handshake with capabilities and nonce
- **ChallengeResponse** — HMAC proof verification
- **CommandStream** — Bidirectional multiplexed stream over HTTP/2

HTTP/2 tuning: adaptive window enabled, 1MB initial stream window, 4MB initial connection window, 4MB max frame size.

## Project Layout

```
Bastion/
├── crates/
│   ├── bastion-domain/         # Traits: SandboxProvider, CommandRouter, SandboxRepository
│   ├── bastion-application/    # Use cases: CreateSandbox, RunCommand, ReadFile, ...
│   ├── bastion-infrastructure/ # PodmanProvider, FirecrackerProvider, PoolManager, InMemoryRepo
│   ├── bastion-gateway/        # MCP server + gRPC RegistryService + composition root
│   └── bastion-worker/         # gRPC CLIENT: registers, authenticates, executes commands
├── proto/sandbox/v1/           # Protobuf: WorkerRegistry service
├── config/                     # TOML config files
├── scripts/                    # build-worker.sh, etc.
└── docs/                       # Architecture docs, research notes
```
