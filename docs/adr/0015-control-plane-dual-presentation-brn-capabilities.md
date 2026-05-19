# ADR-0015: Control Plane Architecture — Dual Presentation Layer with BRN and Capability-Based Auth

## Status

**Proposed** (2026-05-19)

## Context

Bastion exposes 43 MCP tools oriented toward AI-agent execution (sandbox lifecycle, orientation, catalog queries). Admin/management features (pool tuning, provider registration, worker management, doctor check execution, catalog CRUD) are currently TOML-based or CLI-only with no runtime management surface.

The existing `bastion-dashboard` crate uses Leptos WASM + Axum but acts as a thin REST client to the gateway. It lacks Control Plane features.

A human-facing dashboard needs: live monitoring, provider/pool configuration, worker inspection, doctor execution, catalog management, and pipeline visualization.

## Decision

### 1. Dual Presentation Layer — Single Binary

The `bastion-gateway` binary serves both MCP and REST from one process:

- **MCP** (port 50052): Execution-scoped. AI agents create, run, terminate sandboxes. No admin operations.
- **REST API** (port 8080): Control Plane-scoped. Humans configure, monitor, and manage all infrastructure.

Both layers call into the same domain/application logic. No duplicated business rules.

```
bastion-gateway (single binary)
├── MCP Server      → EXECUTION capability
├── REST API (Axum) → ADMIN + READONLY capabilities
└── Shared Application Layer
```

### 2. BRN (Bastion Resource Name)

Uniform resource identifier following the pattern:

```
brn:{namespace}:{type}[/{sub-type}]:{resource-id}
```

Namespaces: `sandbox`, `project`, `provider`, `template`, `catalog`, `infra`

Examples:
- `brn:sandbox:sandbox:sandbox_abc123`
- `brn:provider:instance:inst_uuid-here`
- `brn:catalog:doctor:firecracker-readiness`
- `brn:infra:pool:default`

Gateway ID omitted in v1 for simplicity. Format designed so `brn:{gateway-id}:{ns}:...` can be added without breaking change.

### 3. Capability-Based Authorization

Three capabilities controlling access per resource:

| Capability | Scope |
|-----------|-------|
| `EXECUTION` | MCP — sandbox lifecycle, command execution |
| `ADMIN` | REST — configuration, management, mutations |
| `READONLY` | Both — queries, lists, status |

Resource-to-capability mapping:

| BRN Pattern | EXECUTION | ADMIN | READONLY |
|-------------|:---------:|:-----:|:--------:|
| `brn:sandbox:*` | ✅ | - | - |
| `brn:project:*` | ✅ | ✅ | ✅ |
| `brn:provider:*` | - | ✅ | ✅ |
| `brn:template:*` | - | ✅ | ✅ |
| `brn:catalog:doctor:*` | - | ✅ (run) | ✅ (list) |
| `brn:catalog:experience:*` | - | - | ✅ |
| `brn:infra:pool:*` | - | ✅ | ✅ |
| `brn:infra:worker:*` | - | ✅ | ✅ |
| `brn:infra:config:*` | - | ✅ | ✅ |
| `brn:infra:secret:*` | - | ✅ | ✅ |

### 4. Authentication

- **MCP**: HMAC challenge-response per connection (existing mechanism)
- **REST**: API Key via `Authorization: Bearer <api-key>` header

Auth is enforced at the presentation layer before reaching domain logic. Domain layer declares capability requirements; presentation layer maps auth context to capabilities.

## Considered Options

**Presentation architecture:**
- Separate binaries (rejected — operational complexity, state sharing issues)
- Single binary with shared domain (chosen — simpler ops, shared state)
- Dashboard calls MCP for admin (rejected — MCP is agent-oriented, wrong abstraction level)

**Resource naming:**
- Plain string IDs (rejected — no type safety, no hierarchy)
- Full AWS ARN with gateway-id (rejected — over-engineered for single-tenant)
- Simplified BRN without gateway-id (chosen — extensible, human-readable)

**Authorization:**
- Domain-layer auth (rejected — pollutes domain with infrastructure concern)
- Presentation-layer filtering only (rejected — no declarative policy)
- Capability-based with domain-declared requirements (chosen — clean separation, extensible)

## Consequences

- Adding a new resource type requires: BRN type + capability mapping + REST endpoint + (optional) MCP tool
- MCP tools are frozen to execution scope; admin features go through REST exclusively
- Dashboard can be extended independently of MCP
- BRN enables future fine-grained RBAC if multi-tenant is needed
- Single binary means REST and MCP share memory state (pool, registry) — no sync needed
