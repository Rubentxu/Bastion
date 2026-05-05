# High-Performance Bastion MCP — Architecture Research

> **Date:** 2026-05-05 | **Status:** Research | **Priority:** Critical for production

---

## Executive Summary

El sistema actual puede manejar **~20-50 pipelines concurrentes** con la configuración actual. Para llegar a **100-1000+ pipelines pesados simultáneos** con long-running jobs, necesitamos cambios arquitectónicos en varios frentes.

---

## Current Architecture Analysis

### Hot Paths (from CogniCode)
```
sandbox_run (fan_in: 37) ──► RunCommandUseCase
sandbox_create (fan_in: 39) ──► CreateSandboxUseCase
sandbox_prepare (fan_in: ?) ──► PrepareEnvironmentUseCase
```

### Current Concurrency Model

```
┌─────────────────────────────────────────────────────┐
│  OpenCode / Python Client                          │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐             │
│  │Session A│ │Session B│ │Session C│  ... N       │
│  └────┬────┘ └────┬────┘ └────┬────┘             │
│       │            │            │                   │
│       └────────────┴────────────┘                   │
│                    │                                │
│                    ▼                                │
│         ┌─────────────────────┐                     │
│         │ BastionGateway     │                     │
│         │ (single instance)   │ ◄── rate_limiter   │
│         │ - MCP handlers      │     (100 burst)    │
│         │ - Repository        │     (20 req/s)     │
│         │ - PoolManager       │                    │
│         └──────────┬──────────┘                    │
│                    │                                │
│       ┌───────────┼───────────┐                    │
│       ▼           ▼           ▼                    │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐           │
│  │Podman   │ │Local    │ │ Wasm    │  ...        │
│  │Provider │ │Provider │ │Provider │             │
│  └────┬────┘ └────┬────┘ └─────────┘             │
│       │            │                               │
│       ▼            ▼                               │
│  ┌─────────────────────────────────┐             │
│  │ SQLite DB (single writer)       │  ◄── bottleneck │
│  └─────────────────────────────────┘             │
└─────────────────────────────────────────────────────┘
```

### Current Limits

| Component | Limit | Notes |
|-----------|-------|-------|
| Rate limiter | 100 burst, 20 req/s | Global per gateway instance |
| Pool | max_total: 50 | Sandboxes pre-created |
| SQLite | 1 writer | Writes are serialized |
| SSE sessions | unbounded | But keep-alive timeout = 5 min |
| Worker registry | 1 per gateway | gRPC port 50052 |

---

## Identified Bottlenecks

### 🔴 CRITICAL: rmcp Keep-Alive Timeout (5 min)

**File:** `rmcp` crate (external dependency)

**Problem:**
```rust
// In rmcp streamable_http_server:
// worker quit with fatal: keep alive timeout after 300000ms
```

Long-running pipelines (Maven builds, npm installs) that take > 5 min get **killed**. This is a hard limit in the `rmcp` library.

**Impact:** Any pipeline step > 5 min is killed

**Current workaround:** None — rmcp controls this

### 🔴 CRITICAL: Rate Limiter is Global

**File:** `server.rs` line 73
```rust
rate_limiter: Arc<Mutex<RateLimiter>>, // 100 burst, 20 req/s
```

All clients share ONE rate limiter. A single misbehaving client can starve all others.

**Impact:** No per-client isolation

### 🟡 HIGH: SQLite Single Writer

**File:** `SqliteSandboxRepository`

```rust
// All writes go through a single database
// Concurrent reads are fine, but writes serialize
```

For 100+ concurrent sandboxes, DB write contention becomes significant.

**Impact:** ~1000 writes/sec sustained = potential slowdowns

### 🟡 HIGH: Sandbox Pool Disabled by Default

**File:** `main.rs`
```rust
pool_manager: Option<Arc<SandboxPoolManager>> = if args.pool_enabled {
```

Currently `pool_enabled = false`. Sandbox creation is **~1.5-5s** instead of **<200ms**.

**Impact:** Slow startup for each pipeline step

### 🟡 HIGH: No Pipeline Priority/Queue

All sandboxes are equal. A heavy 10-min Maven build blocks a quick `ls` command.

**Impact:** No QoS, starvations possible

### 🟡 MEDIUM: No Cancellation Support

Once a `sandbox_run` starts, it runs to completion (or timeout). No `sandbox_cancel` tool.

**Impact:** Wasted resources on abandoned pipelines

### 🟡 MEDIUM: No Progress Updates

Long operations (jvm-build ~56s, Maven ~92s) return **only at the end**. No streaming progress.

**Impact:** Poor UX, client doesn't know if stuck

### 🟢 LOW: Worker Registry Single Instance

Single gRPC server on port 50052. If it dies, all workers lose their registry.

---

## Proposed Improvements (Priority Order)

### Phase 1: Quick Wins (1-2 days)

#### 1.1 Enable Sandbox Pool by Default
```rust
// In main.rs
pool_enabled: bool = true,  // Change default
```

**Impact:** sandbox_create goes from ~1.5s → <200ms

#### 1.2 Add `sandbox_cancel` Tool
```rust
#[tool(description = "Cancel a running command")]
async fn sandbox_cancel(&self, Parameters(params): Parameters<SandboxCancelParams>) -> String
```

**Impact:** User can stop abandoned builds

#### 1.3 Increase Rate Limiter Defaults
```rust
// From 20 req/s → 100 req/s
// From 100 burst → 500 burst
```

**Impact:** Better throughput for bursty workloads

#### 1.4 Fix rmcp Keep-Alive Timeout
**Option A:** Fork rmcp and increase timeout
**Option B:** Implement ping/pong heartbeat from client
**Option C:** Use streaming responses for long operations (MCP has this)

### Phase 2: Core Improvements (1-2 weeks)

#### 2.1 Per-Client Rate Limiting
```rust
// Per-session rate limit instead of global
struct PerSessionRateLimiter {
    limits: HashMap<SessionId, RateLimiter>,
}
```

**Impact:** Isolation between clients

#### 2.2 Pipeline Priority System
```rust
#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
enum PipelinePriority {
    Low = 0,     // Background jobs
    Normal = 1,  // Default
    High = 2,    // User-initiated
    Critical = 3, // Production deployments
}

// In sandbox_create params
struct SandboxCreateParams {
    priority: PipelinePriority,
    // ...
}
```

**Impact:** QoS for different workload types

#### 2.3 Streaming Progress Updates
MCP supports **progress tokens**:
```rust
#[tool(description = "Prepare environment with progress streaming")]
async fn sandbox_prepare(
    &self,
    params: Parameters<...>,
    request_ctx: RequestContext<RoleServer>,
) -> String {
    // Progress token from request_ctx.meta
    // Send progress updates via SSE:
    // event: progress
    // data: {"step": "installing_java", "percent": 45}
}
```

**Impact:** User sees real-time progress

#### 2.4 Batch Operations
```rust
#[tool(description = "Create multiple sandboxes at once")]
async fn sandbox_batch_create(
    &self,
    Parameters(params): Parameters<BatchCreateParams>,
) -> String {
    // params.items: Vec<SandboxCreateItem>
    // Execute all in parallel via tokio::spawn
    // Return Vec<CreateResult>
}
```

**Impact:** 100 sandboxes in ~2s instead of ~150s

### Phase 3: Scale-Out Architecture (2-4 weeks)

#### 3.1 Horizontal Gateway Scaling
```
                    ┌─────────────────┐
                    │  Load Balancer  │
                    │  (kube-proxy)   │
                    └────────┬────────┘
                             │
         ┌───────────────────┼───────────────────┐
         ▼                   ▼                   ▼
  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
  │ Gateway #1  │    │ Gateway #2  │    │ Gateway #3  │
  │ :18765      │    │ :18766      │    │ :18767      │
  └──────┬──────┘    └──────┬──────┘    └──────┬──────┘
         │                   │                   │
         └───────────────────┼───────────────────┘
                             │
                    ┌────────┴────────┐
                    │  SQLite/Redis   │
                    │  (shared state) │
                    └─────────────────┘
```

**Requirements:**
- Shared session store (Redis or SQLite with WAL mode)
- Worker registry must be distributed
- Sticky sessions for SSE or stateless HTTP

**Impact:** 1000+ concurrent pipelines

#### 3.2 PostgreSQL instead of SQLite
```rust
// For high concurrency
pub struct PostgresSandboxRepository {
    pool: sqlx::PgPool,  // Connection pool
}
```

**Benefits:**
- True concurrent writes
- Row-level locking
- Replication for HA
- Connection pooling

**Impact:** 10x more concurrent writes

#### 3.3 Worker Pool Clustering
```
Gateway #1 ──► Worker Registry #1 ──► Workers A, B, C
Gateway #2 ──► Worker Registry #2 ──► Workers D, E, F
```

Workers register with ALL registries for HA.

**Impact:** No single point of failure

### Phase 4: Advanced Features (1-2 months)

#### 4.1 Pipeline DAG Execution
```rust
struct Pipeline {
    id: PipelineId,
    steps: Vec<PipelineStep>,
    dependencies: HashMap<StepId, Vec<StepId>>,  // DAG
}

struct PipelineStep {
    sandbox_id: SandboxId,
    command: String,
    env: HashMap<String, String>,
    continue_on_error: bool,
}

// Execute like make - parallel where possible
```

**Impact:** Complex multi-step builds with proper parallelism

#### 4.2 Resource Quotas per Client
```rust
struct ClientQuota {
    max_sandboxes: usize,       // e.g., 10 per user
    max_concurrent: usize,       // e.g., 3 per user
    max_total_runtime_ms: u64,   // e.g., 1 hour per day
}
```

**Impact:** Multi-tenant isolation

#### 4.3 Adaptive Pool Sizing
```rust
struct AdaptivePoolConfig {
    min_idle: usize,          // 2
    max_idle: usize,          // 20
    scale_up_threshold: f32,   // 0.7 (70% utilization)
    scale_down_threshold: f32, // 0.2 (20% utilization)
    scale_factor: usize,       // +2 per scale-up
}
```

**Impact:** Cost optimization, fast response under load

---

## Implementation Roadmap

### Week 1: Foundation
- [ ] Enable pool by default
- [ ] Add `sandbox_cancel`
- [ ] Increase rate limiter defaults
- [ ] Add per-client rate limiting

### Week 2: Reliability
- [ ] Streaming progress for sandbox_prepare
- [ ] Streaming progress for sandbox_run
- [ ] `sandbox_batch_create`
- [ ] Pipeline priority system

### Week 3-4: Observability
- [ ] Prometheus metrics endpoint
- [ ] Structured logging with correlation IDs
- [ ] Dashboard for pool utilization
- [ ] Alerting on error rates

### Week 5-6: Scale-Out
- [ ] PostgreSQL backend option
- [ ] Horizontal gateway scaling
- [ ] Distributed worker registry
- [ ] Redis session store

### Week 7-8: Advanced
- [ ] Pipeline DAG executor
- [ ] Resource quotas
- [ ] Adaptive pool sizing
- [ ] Multi-region support

---

## Recommendations for Immediate Action

1. **Mañana:** Enable pool by default + increase rate limiter
2. **Esta semana:** Add `sandbox_cancel` + progress streaming
3. **Próxima semana:** Per-client rate limiting + priority system
4. **Este mes:** PostgreSQL option + horizontal scaling design

---

## Questions to Resolve

1. **Multi-tenant?** Do we need strict isolation between users/orgs?
2. **SLA?** What's the target latency for sandbox_create?
3. **Storage?** How long should sandboxes/artifacts be retained?
4. **Regions?** Single region or multi-region deployment?
5. **Persistence?** Should pipelines survive gateway restarts?

