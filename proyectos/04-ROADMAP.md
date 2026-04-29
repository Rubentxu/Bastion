# Roadmap: Gateway MCP con Sandboxes Remotos

> **Fecha**: Abril 2026  
> **Estado**: Planificación  
> **Horizonte**: 6-9 meses (1 persona) o 3-5 meses (2 personas)

---

## Visión General

```
2026 Q2                    Q3                         Q4
────┬──────────────────────┬──────────────────────────┬──────────────────
    │                      │                          │
    │  F0  F1              │  F2          F3          │  F4        F5
    │  PoC MVP             │  Multi       Streaming   │  Pipes     Catalog
    │  │   │               │  backend     + Obs       │  + DB      + K8s
    │  ▼   ▼               │              │           │            │
    │  ✓   ✓               │              ✓           │            ✓
    │                      │                          │
    │  HITO 1:             │  HITO 2:                 │  HITO 3:
    │  Primer agente       │  Multi-backend           │  Catálogo
    │  ejecuta en sandbox  │  con streaming           │  extensible
    │                      │                          │
```

---

## Hitos

### 🏁 Hito 1: Primer Agente en Sandbox (Semanas 1-4)

**Entregable**: Un agente de IA puede crear un sandbox Podman, ejecutar comandos,
leer/escribir archivos, y destruir el sandbox, todo via protocolo MCP estándar.

```
Entrada:  Agente MCP → tools/call("sandbox_run", {command: "npm test"})
Salida:   {exit_code: 0, stdout: "3 tests passed", stderr: ""}
```

| Semana | Fase | Entregable |
|--------|------|-----------|
| 1 | F0: PoC | rmcp + tonic + Podman validados |
| 2 | F1: Core | Trait SandboxProvider + tipos + protobuf |
| 3 | F1: Gateway | 5 tools MCP + Tool Router + Worker gRPC |
| 4 | F1: Polish | Tests E2E + docs + config TOML |

**Criterio de éxito**: `claude` (Claude Code) puede usar el Gateway como MCP server
y ejecutar código en un sandbox Podman.

### 🏁 Hito 2: Multi-Backend con Streaming (Semanas 5-12)

**Entregable**: Gateway soporta 3 backends (Podman, Firecracker, gVisor) con pool
caliente, streaming de stdout/stderr en tiempo real, y observabilidad.

```
Entrada:  Agente → sandbox_create({provider: "firecracker", template: "rust:latest"})
Salida:   {sandbox_id: "fc-001", status: "running"} en <200ms (pool caliente)

Entrada:  Agente → sandbox_run({sandbox_id: "fc-001", command: "cargo build"})
Salida:   [streaming] Compiling sandbox-core v0.1.0 ...
          [streaming] Finished dev [unoptimized] in 12.3s
          [final] {exit_code: 0}
```

| Semana | Fase | Entregable |
|--------|------|-----------|
| 5-6 | F2: Pool | SandboxPoolManager + hot pool |
| 7-8 | F2: Firecracker | FirecrackerBackend + REST client + snapshots |
| 9 | F2: gVisor | GVisorBackend + runsc wrapper |
| 10 | F3: Streaming | Progress notifications + cancellation |
| 11 | F3: Observabilidad | Métricas Prometheus + tracing + health |
| 12 | Buffer | Polish, tests de carga, documentación |

**Criterio de éxito**: Benchmarks muestran <200ms para sandbox_create (pool caliente),
<100ms overhead para sandbox_run, streaming funcional, y métricas en Grafana.

### 🏁 Hito 3: Catálogo Extensible y K8s (Semanas 13-27)

**Entregable**: Sistema de appliances, pipelines multi-sandbox, database sandboxing,
backend Kubernetes, HTTP transport para multi-agente.

```
Entrada:  Agente → sandbox_create({appliance: "python-dev"})
Salida:   {sandbox_id: "py-001", tools: ["run_code", "read_file", "write_file", ...]}

Entrada:  Agente → sandbox_pipeline({pipeline: "build-test-deploy"})
Salida:   [stage: build] ✅ Compiled
          [stage: test]  ✅ 42 tests passed
          [stage: deploy] ✅ Deployed to sandbox-003
```

| Semana | Fase | Entregable |
|--------|------|-----------|
| 13-16 | F4: Pipelines | Pipeline DSL + executor + artifact transfer |
| 17-18 | F4: Database | DatabaseProvider trait + PostgreSQL + SQLite |
| 19-21 | F5: Catálogo | Appliance registry + dynamic tool discovery |
| 22-23 | F5: HTTP | Streamable HTTP + auth + rate limiting |
| 24-26 | F5: K8s | KubernetesBackend + pod pool |
| 27 | Buffer | Integration testing, docs, release |

**Criterio de éxito**: Appliance se registra y tools se descubren automáticamente.
Pipeline build→test→deploy funciona end-to-end. K8s backend crea Pods efímeros.

---

## Roadmap Detallado por Fase

### Fase 0: PoC de Validación (Semana 1)

```
Dia 1:    Workspace Cargo + protobuf mínimo
Dia 2:    PoC rmcp server (1 tool echo)
Dia 3:    PoC tonic worker (gRPC stream)
Dia 4:    Integración rmcp → tonic → resultado
Dia 5:    Podman spawn + benchmark Firecracker + documentar
```

**Decisión gate**: Si rmcp + tonic no coexisten → pivotar a HTTP interno en vez de gRPC

### Fase 1: MVP con Podman (Semanas 2-4)

```
Semana 2: Core types + trait + protobuf completo
          ├── sandbox-core crate (types, error, traits)
          ├── proto/sandbox/v1/sandbox.proto
          └── sandbox-providers/src/podman.rs (start)

Semana 3: Gateway + Worker + Tools MCP
          ├── sandbox-gateway/src/server.rs (rmcp tools)
          ├── sandbox-gateway/src/router.rs (dispatch)
          ├── sandbox-worker/src/grpc_server.rs (tonic)
          ├── sandbox-worker/src/executor.rs (commands)
          └── sandbox-providers/src/podman.rs (complete)

Semana 4: Tests + Docs + Config
          ├── tests/integration/test_podman.rs
          ├── tests/integration/test_lifecycle.rs
          ├── config/sandbox-gateway.toml
          └── README.md
```

### Fase 2: Multi-Backend (Semanas 5-11)

```
Semana 5-6:  Pool Manager
             ├── sandbox-gateway/src/pool.rs
             ├── Hot pool: pre-create, checkout, refill
             └── Cleanup scheduler

Semana 7-8:  Firecracker Backend
             ├── sandbox-providers/src/firecracker.rs
             ├── REST API client (reqwest + Unix socket)
             ├── VM lifecycle (create, start, stop)
             └── Snapshot/Restore

Semana 9:    gVisor Backend
             ├── sandbox-providers/src/gvisor.rs
             ├── runsc wrapper (Command::new("runsc"))
             └── Rootless execution

Semana 10-11: Provider selection + fallback
              ├── Provider factory
              ├── Capability-based selection
              └── Tests multi-provider
```

### Fase 3: Streaming y Observabilidad (Semanas 10-12)

```
Semana 10:   Streaming MCP
             ├── Progress notifications (rmcp)
             ├── Stdout/stderr streaming via gRPC
             ├── Cancellation propagation
             └── Backpressure handling

Semana 11:   Observabilidad
             ├── Prometheus metrics
             ├── Structured logging (tracing)
             ├── OpenTelemetry spans
             └── Health check endpoint

Semana 12:   Buffer + Load testing
             ├── 50+ concurrent sandboxes test
             ├── Performance profiling
             └── Documentation update
```

### Fase 4: Pipelines y Composición (Semanas 13-20)

```
Semana 13-14: Pipeline DSL + Executor
              ├── Pipeline definition (YAML)
              ├── Stage execution engine
              └── State machine per stage

Semana 15-16: Artifact Transfer
              ├── sandbox_transfer tool
              ├── File streaming between sandboxes
              └── Checksum verification

Semana 17-18: Database Sandbox
              ├── DatabaseProvider trait
              ├── PostgreSQL backend (TEMPLATE)
              └── SQLite backend (file copy)

Semana 19-20: Tests + Polish
              ├── Pipeline E2E tests
              ├── Artifact integrity tests
              └── Database branch tests
```

### Fase 5: Catálogo y K8s (Semanas 21-27)

```
Semana 21-22: Appliance System
              ├── Appliance spec (YAML)
              ├── Registry storage
              └── Dynamic tool discovery

Semana 23-24: Tool Proxy + HTTP Transport
              ├── Expose sandbox MCP tools via gateway
              ├── Streamable HTTP server (rmcp)
              ├── Auth middleware (API keys)
              └── Rate limiting

Semana 25-26: Kubernetes Backend
              ├── sandbox-providers/src/kubernetes.rs
              ├── Pod pool (Deployment + Service)
              ├── Namespace isolation
              └── HPA integration

Semana 27:    Release
              ├── Integration testing
              ├── Documentation complete
              ├── Examples gallery
              └── crates.io publish
```

---

## Milestones y Releases

### Versionado propuesto

| Versión | Hito | Contenido |
|---------|------|-----------|
| `v0.1.0` | Hito 1 | MVP con Podman, 5 tools |
| `v0.2.0` | F2 parcial | Pool Manager + Firecracker |
| `v0.3.0` | F2 completa | Multi-backend (Podman + Firecracker + gVisor) |
| `v0.4.0` | Hito 2 | Streaming + Observabilidad |
| `v0.5.0` | F4 parcial | Pipelines + Artifact Transfer |
| `v0.6.0` | F4 completa | Database Sandbox |
| `v0.7.0` | F5 parcial | Appliance Registry |
| `v0.8.0` | F5 parcial | HTTP Transport + Auth |
| `v0.9.0` | Hito 3 | Kubernetes Backend |
| `v1.0.0` | Release | All features, stable API |

---

## Riesgos y Mitigaciones por Fase

| Fase | Riesgo | Probabilidad | Impacto | Mitigación |
|------|--------|-------------|---------|------------|
| **0** | rmcp + tonic conflicto | Baja | Alto | PoC antes de comprometer |
| **1** | Podman API inestable | Baja | Medio | Bollard como alternativa |
| **2** | Firecracker requiere KVM | Alta | Medio | gVisor como fallback |
| **2** | Firecracker REST complejidad | Media | Medio | Empezar con operaciones básicas |
| **3** | Progress notifications limitados | Baja | Medio | gRPC stream como respaldo |
| **4** | Pipeline DSL scope creep | Alta | Medio | DSL mínimo, extender después |
| **5** | K8s cold start >5s | Alta | Bajo | Pool de Pods calientes |
| **5** | Dynamic tool discovery complejo | Media | Alto | Empezar con catálogo estático |

---

## Métricas de Éxito

### Técnicas

| Métrica | Target Fase 1 | Target Fase 3 | Target Fase 5 |
|---------|--------------|---------------|---------------|
| Latencia sandbox_create (cold) | <3s | <2s | <1s |
| Latencia sandbox_create (hot) | N/A | <200ms | <50ms |
| Latencia sandbox_run overhead | <200ms | <100ms | <50ms |
| Throughput (tool calls/s) | 100 | 500 | 1000 |
| Concurrent sandboxes | 10 | 50 | 200 |
| Test coverage | >80% | >80% | >80% |
| Memory gateway (idle) | <200MB | <150MB | <100MB |

### De adopción

| Métrica | Target 3 meses | Target 6 meses | Target 12 meses |
|---------|---------------|----------------|-----------------|
| GitHub stars | 100 | 500 | 2000 |
| Contributors | 2-3 | 5-10 | 15+ |
| MCP servers usando el gateway | 1 | 5 | 20+ |
| Descargas crates.io | 100/mes | 1000/mes | 5000/mes |

---

## Próximos Pasos Inmediatos

### Esta semana (Fase 0)

```
□ 1. Crear repositorio GitHub con estructura del workspace
□ 2. Implementar PoC rmcp server (1 tool: echo)
□ 3. Implementar PoC tonic worker (gRPC echo)
□ 4. Integrar rmcp → tonic end-to-end
□ 5. Verificar Podman create/destroy en <3s
□ 6. Documentar findings y decisión de continuar
```

### Decisiones pendientes

| Decisión | Opciones | Criterio | Fecha límite |
|----------|---------|----------|-------------|
| Container API crate | `podman-api` vs `bollard` | API coverage + maintenance | Semana 2 |
| Config format | TOML vs YAML | Rust ecosystem preference | Semana 2 |
| Worker distribution | Built-in image vs download | User experience | Semana 3 |
| Streaming approach | MCP progress vs gRPC stream | MCP spec compliance | Semana 10 |
| Pipeline DSL format | YAML vs TOML vs DSL custom | Ergonomics | Semana 13 |

---

## Apéndice: Checklist de Lanzamiento v0.1.0

- [ ] `cargo test` pasa sin errores
- [ ] `cargo clippy` sin warnings
- [ ] `cargo doc` genera documentación completa
- [ ] README con quickstart guide
- [ ] Config de ejemplo funcional
- [ ] Al menos 1 agente MCP real probado (Claude Code, OpenCode, etc.)
- [ ] CI/CD pipeline (GitHub Actions) funcionando
- [ ] Tests de integración con Podman en CI
- [ ] CHANGELOG.md con cambios documentados
- [ ] LICENSE (Apache-2.0 o MIT)
- [ ] CONTRIBUTING.md
- [ ] crates.io publish de sandbox-core
