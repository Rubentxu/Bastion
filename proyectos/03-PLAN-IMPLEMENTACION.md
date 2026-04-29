# Plan de Implementación: Gateway MCP con Sandboxes Remotos

> **Fecha**: Abril 2026  
> **Estado**: Planificación  
> **Dependencias**: [01-INVESTIGACION-SANDBOX-MCP.md](./01-INVESTIGACION-SANDBOX-MCP.md), [02-ARQUITECTURA-UNIFICADA.md](./02-ARQUITECTURA-UNIFICADA.md)

---

## Tabla de Contenidos

1. [Estrategia de Implementación](#1-estrategia-de-implementación)
2. [Fase 0: PoC de Validación](#2-fase-0-poc-de-validación)
3. [Fase 1: MVP](#3-fase-1-mvp)
4. [Fase 2: Multi-Backend](#4-fase-2-multi-backend)
5. [Fase 3: Streaming y Observabilidad](#5-fase-3-streaming-y-observabilidad)
6. [Fase 4: Pipelines y Composición](#6-fase-4-pipelines-y-composición)
7. [Fase 5: Catálogo y Extensibilidad](#7-fase-5-catálogo-y-extensibilidad)
8. [Estimaciones y Dependencias](#8-estimaciones-y-dependencias)
9. [Criterios de Calidad](#9-criterios-de-calidad)

---

## 1. Estrategia de Implementación

### 1.1 Principios

1. **Incremental**: Cada fase produce un entregable funcional usable
2. **Test-first**: Tests de integración antes de implementación
3. **Un backend a la vez**: Podman → Firecracker → gVisor → K8s
4. **MCP-first**: El Gateway es siempre un MCP server estándar compatible

### 1.2 Enfoque de desarrollo

```
Fase 0: Validación técnica (1 semana)
    ↓
Fase 1: MVP funcional con Podman (2-3 semanas)
    ↓
Fase 2: Múltiples backends (4-6 semanas)
    ↓
Fase 3: Streaming, observabilidad (2-3 semanas)
    ↓
Fase 4: Pipelines multi-sandbox (4-8 semanas)
    ↓
Fase 5: Catálogo extensible (4-6 semanas)
```

### 1.3 Rama de desarrollo

```
main
├── fase-0/poc-rmcp-tonic          # Validación
├── fase-1/mvp-podman              # MVP
├── fase-2/firecracker-backend     # Multi-backend
├── fase-2/gvisor-backend          # Multi-backend
├── fase-3/streaming               # Streaming
├── fase-4/pipelines               # Pipelines
└── fase-5/catalog                 # Catálogo
```

---

## 2. Fase 0: PoC de Validación

**Duración**: 1 semana  
**Objetivo**: Confirmar que rmcp y tonic coexisten sin conflictos en el mismo proceso async

### 2.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| 0.1 | Crear workspace Cargo | `cargo init --workspace` con crates: core, gateway, worker, providers | Alta | 2h |
| 0.2 | Setup protobuf | Definir `sandbox.proto` mínimo (solo `CallTool` + `stream CallToolResponse`) | Alta | 2h |
| 0.3 | PoC rmcp server | Servidor MCP mínimo con 1 tool (`echo`) usando rmcp | Alta | 3h |
| 0.4 | PoC tonic server | Worker gRPC mínimo que recibe `CallTool` y devuelve stream | Alta | 3h |
| 0.5 | PoC integración | Gateway rmcp que llama a tonic worker via gRPC | **Crítica** | 4h |
| 0.6 | PoC Podman spawn | Crear/destruir contenedor Podman desde Rust en <2s | Alta | 3h |
| 0.7 | Benchmark Firecracker | Medir snapshot/restore time en hardware local | Media | 2h |
| 0.8 | Documentar resultados | Escribir findings y decisiones de diseño | Alta | 2h |

### 2.2 Criterios de aceptación

- [ ] rmcp server arranca y responde a `tools/list` via stdio
- [ ] tonic worker arranca y responde a `CallTool` via gRPC
- [ ] Gateway recibe `tools/call`, traduce a gRPC, recibe respuesta, devuelve resultado MCP
- [ ] Podman crea/destruye un contenedor en <3 segundos
- [ ] No hay deadlocks ni panic entre tokio + rmcp + tonic

### 2.3 Riesgos específicos

| Riesgo | Mitigación |
|--------|-----------|
| rmcp y tonic compiten por tokio runtime | Usar `tokio::main` con `#[tokio::main(flavor = "multi_thread")]` |
| Podman API no responde rápido | Usar `podman_api` crate con timeout configurable |
| Protobuf generation falla | Usar `prost-build` con features estables |

### 2.4 Código de PoC — Integración rmcp + tonic

```rust
// crates/sandbox-gateway/src/poc.rs

use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router, ServiceExt, transport::stdio};
use tonic::transport::Channel;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunParams {
    command: String,
}

#[derive(Clone)]
struct SandboxGateway {
    grpc_channel: Channel,  // tonic channel to worker
}

#[tool_router(server_handler)]
impl SandboxGateway {
    #[tool(description = "Execute a command in a remote sandbox")]
    async fn sandbox_run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> String {
        let mut client = sandbox_proto::sandbox_worker_client::SandboxWorkerClient::new(
            self.grpc_channel.clone()
        );

        let request = tonic::Request::new(sandbox_proto::CallToolRequest {
            sandbox_id: "poc".to_string(),
            tool_name: "run".to_string(),
            arguments: serde_json::json!({"command": params.command}).to_string(),
            timeout_ms: 30_000,
            progress_token: String::new(),
        });

        match client.call_tool(request).await {
            Ok(response) => {
                let stream = response.into_inner();
                // Collect all chunks
                let mut result = String::new();
                #[allow(unused)]
                use futures::StreamExt;
                // Note: in real impl, use notify_progress for intermediate chunks
                futures::pin_mut!(stream);
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(c) if c.is_final => {
                            result = String::from_utf8_lossy(&c.data).to_string();
                        }
                        Ok(c) => {
                            result.push_str(&String::from_utf8_lossy(&c.data));
                        }
                        Err(e) => {
                            result = format!("gRPC error: {}", e);
                        }
                    }
                }
                result
            }
            Err(e) => format!("Connection error: {}", e),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::init();

    // Connect to gRPC worker
    let grpc_channel = Channel::from_static("http://127.0.0.1:50051")
        .connect()
        .await?;

    let gateway = SandboxGateway { grpc_channel };

    // Start MCP server on stdio
    let service = gateway.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
```

---

## 3. Fase 1: MVP

**Duración**: 2-3 semanas  
**Objetivo**: Gateway MCP funcional con backend Podman, 4 tools básicas

### 3.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| **Core** | | | | |
| 1.1 | Crate `sandbox-core` | Tipos compartidos (`SandboxConfig`, `SandboxInfo`, `SandboxError`, etc.) | Alta | 6h |
| 1.2 | Trait `SandboxProvider` | Definición del trait universal con `#[async_trait]` | Alta | 3h |
| 1.3 | Protobuf completo | Definir todos los mensajes y service del protobuf | Alta | 4h |
| **Gateway** | | | | |
| 1.4 | rmcp server completo | Tools: `sandbox_create`, `sandbox_run`, `sandbox_write`, `sandbox_read`, `sandbox_terminate` | Alta | 1.5 días |
| 1.5 | Tool Router | Dispatch por tool_name, validación de args, routing a provider | Alta | 6h |
| 1.6 | Sandbox Registry | HashMap thread-safe de sandboxes activos | Alta | 3h |
| 1.7 | Config parsing | Leer `sandbox-gateway.toml` con config-rs | Media | 3h |
| **Worker** | | | | |
| 1.8 | gRPC server completo | Implementar `SandboxWorker` service con tonic | Alta | 1 día |
| 1.9 | Command executor | Async process spawning con tokio, capture stdout/stderr | Alta | 6h |
| 1.10 | File operations | read/write/list files via tokio::fs | Alta | 3h |
| **Provider** | | | | |
| 1.11 | PodmanBackend | Implementar `SandboxProvider` para Podman | Alta | 1.5 días |
| 1.12 | Container lifecycle | create, start, stop, remove containers | Alta | 6h |
| 1.13 | gRPC connection | Worker se registra con Gateway o viceversa | Alta | 4h |
| **Tests** | | | | |
| 1.14 | Tests unitarios core | Tipos, trait, errores | Alta | 4h |
| 1.15 | Tests integración Podman | Lifecycle completo via MCP client | Alta | 6h |
| 1.16 | Test E2E | Agente → Gateway → Worker → resultado | Alta | 4h |
| **Docs** | | | | |
| 1.17 | README + getting started | Instrucciones de build y run | Media | 2h |
| 1.18 | Config reference | Documentar sandbox-gateway.toml | Media | 2h |

### 3.2 Criterios de aceptación

- [ ] `sandbox_create` crea un contenedor Podman y devuelve `SandboxInfo`
- [ ] `sandbox_run("abc123", "echo hello")` devuelve `"hello\n"`
- [ ] `sandbox_write("abc123", "/tmp/test.txt", "content")` escribe el archivo
- [ ] `sandbox_read("abc123", "/tmp/test.txt")` devuelve `"content"`
- [ ] `sandbox_terminate("abc123")` destruye el contenedor
- [ ] Un agente MCP puede conectarse via stdio y usar todas las tools
- [ ] Config via TOML funciona con al menos provider Podman
- [ ] Tests de integración pasan con Podman corriendo localmente

### 3.3 Entregable

Un binario `sandbox-gateway` que:

```bash
# Iniciar el gateway
sandbox-gateway --config sandbox-gateway.toml

# Desde un agente MCP (ej. Claude Code):
# tools/list → muestra sandbox_create, sandbox_run, sandbox_write, sandbox_read, sandbox_terminate
# tools/call("sandbox_create", {template: "rust:latest"}) → {sandbox_id: "abc123", status: "running"}
# tools/call("sandbox_run", {sandbox_id: "abc123", command: "cargo test"}) → {exit_code: 0, stdout: "..."}
```

---

## 4. Fase 2: Multi-Backend

**Duración**: 4-6 semanas  
**Objetivo**: Implementar Firecracker y gVisor como backends adicionales

### 4.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| **Pool Manager** | | | | |
| 2.1 | SandboxPoolManager | Pool de sandboxes calientes por provider | Alta | 1.5 días |
| 2.2 | Hot pool logic | Pre-crear sandboxes, checkout, refill en background | Alta | 1 día |
| 2.3 | Cleanup scheduler | Destruir sandboxes idle que superen timeout | Media | 4h |
| **Firecracker** | | | | |
| 2.4 | FirecrackerBackend | Implementar `SandboxProvider` para Firecracker | Alta | 2 semanas |
| 2.5 | REST API client | Comunicación con Firecracker via HTTP Unix socket | Alta | 3 días |
| 2.6 | VM lifecycle | create, start, stop microVMs via API REST | Alta | 3 días |
| 2.7 | Snapshot/Restore | Crear y restaurar snapshots de microVMs | Media | 3 días |
| 2.8 | Jailer integration | Integración con jailer para seguridad | Media | 2 días |
| **gVisor** | | | | |
| 2.9 | GVisorBackend | Implementar `SandboxProvider` para gVisor/runsc | Alta | 1 semana |
| 2.10 | runsc wrapper | CLI wrapper para crear/manejar contenedores runsc | Alta | 3 días |
| 2.11 | Rootless support | Ejecución sin privilegios | Media | 2 días |
| **Provider Selection** | | | | |
| 2.12 | Provider factory | Selección dinámica por config/template | Alta | 4h |
| 2.13 | Provider fallback | Fallback automático si un provider falla | Media | 4h |
| 2.14 | Capabilities report | Reportar capacidades de cada provider | Media | 2h |
| **Tests** | | | | |
| 2.15 | Tests Firecracker | Lifecycle completo con microVM | Alta | 1 día |
| 2.16 | Tests gVisor | Lifecycle completo con runsc | Alta | 1 día |
| 2.17 | Tests multi-provider | Cambio dinámico de provider | Media | 4h |
| 2.18 | Tests pool | Pool caliente, checkout, refill | Media | 4h |

### 4.2 Criterios de aceptación

- [ ] `sandbox_create` con `provider: "firecracker"` crea una microVM
- [ ] `sandbox_create` con `provider: "gvisor"` crea un contenedor runsc
- [ ] Pool caliente pre-crea N sandboxes y entrega en <10ms
- [ ] Snapshot/Restore funciona en Firecracker backend
- [ ] Fallback automático: si Firecracker no disponible, usar gVisor
- [ ] `ProviderCapabilities` reportado correctamente por cada backend

### 4.3 Matriz de capabilities por backend

| Capability | Podman | Firecracker | gVisor | K8s |
|-----------|--------|-------------|--------|-----|
| Snapshots | ❌ | ✅ | ❌ | ❌ |
| Streaming | ✅ | ✅ | ✅ | ✅ |
| Pause/Resume | ✅ | ❌ | ❌ | ❌ |
| KVM required | ❌ | ✅ | ❌ | ❌ |
| Rootless | ✅ | ❌ | ✅ | ❌ |
| Networking | ✅ | ✅ | ✅ | ✅ |
| Startup time | ~1.5s | ~125ms | ~2s | ~5s |
| Isolation level | Low | High | Medium | Medium |

---

## 5. Fase 3: Streaming y Observabilidad

**Duración**: 2-3 semanas  
**Objetivo**: Streaming de stdout/stderr via MCP progress, métricas, tracing

### 5.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| **Streaming** | | | | |
| 3.1 | Progress notifications | Integrar `notify_progress` de rmcp con gRPC stream | Alta | 1 día |
| 3.2 | Stdout/stderr chunks | Stream incremental de output en tiempo real | Alta | 1 día |
| 3.3 | Cancellation | `notify_cancelled` → cancel gRPC stream → kill process | Alta | 6h |
| 3.4 | Backpressure | Manejar consumer lento (agente) con producer rápido (process) | Media | 4h |
| **Observabilidad** | | | | |
| 3.5 | Metrics (Prometheus) | Contadores de sandboxes, latencias, errores | Media | 1 día |
| 3.6 | Structured logging | `tracing` con spans por sandbox_id y tool_name | Alta | 4h |
| 3.7 | Distributed tracing | OpenTelemetry integration | Baja | 1 día |
| 3.8 | Health checks | `/health` endpoint para load balancers | Media | 2h |
| **Resources MCP** | | | | |
| 3.9 | `sandbox://{id}/status` | Resource MCP para estado del sandbox | Media | 4h |
| 3.10 | `sandbox://{id}/logs` | Resource MCP para logs (streaming) | Media | 4h |
| **Tests** | | | | |
| 3.11 | Tests streaming | Verificar que progress llega en orden | Alta | 4h |
| 3.12 | Tests cancellation | Verificar que kill funciona mid-stream | Alta | 3h |
| 3.13 | Load test | 50+ sandboxes concurrentes | Media | 1 día |

### 5.2 Criterios de aceptación

- [ ] `sandbox_run` con comando largo reporta progreso via `notify_progress`
- [ ] Stdout aparece en chunks incrementales, no solo al final
- [ ] Cancelar una operación mata el proceso en el sandbox
- [ ] Métricas de Prometheus: `sandbox_created_total`, `sandbox_run_duration_seconds`, `sandbox_active_count`
- [ ] Tracing con correlation ID por sandbox
- [ ] 50 sandboxes concurrentes sin degradation

---

## 6. Fase 4: Pipelines y Composición

**Duración**: 4-8 semanas  
**Objetivo**: Orquestar múltiples sandboxes, transferencia de artefactos, branching

### 6.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| **Pipeline Engine** | | | | |
| 4.1 | Pipeline DSL | Definición declarativa de pipelines (YAML/TOML) | Alta | 1 semana |
| 4.2 | Pipeline executor | Orquestar stages secuenciales/paralelos | Alta | 1 semana |
| 4.3 | Artifact transfer | Mover archivos entre sandboxes | Alta | 3 días |
| 4.4 | Pipeline state machine | `pending → running → completed/failed` por stage | Alta | 3 días |
| **Nuevas Tools** | | | | |
| 4.5 | `sandbox_transfer` | Copiar archivos entre sandboxes | Alta | 3 días |
| 4.6 | `sandbox_pipeline` | Ejecutar un pipeline definido | Alta | 3 días |
| 4.7 | `sandbox_branch` | Crear un sandbox como branch de otro | Media | 4h |
| **Database Sandbox** | | | | |
| 4.8 | DatabaseProvider trait | `create_branch`, `run_query`, `merge_branch` | Media | 1 semana |
| 4.9 | PostgreSQL backend | `CREATE DATABASE ... TEMPLATE ...` | Media | 3 días |
| 4.10 | SQLite backend | Copiar archivo .db a sandbox efímero | Media | 2 días |
| **Tests** | | | | |
| 4.11 | Tests pipeline | Pipeline build→test→deploy end-to-end | Alta | 3 días |
| 4.12 | Tests artifact transfer | Verificar integridad de archivos transferidos | Alta | 2 días |
| 4.13 | Tests database | Branch, query, merge de DB | Media | 2 días |

### 6.2 Ejemplo de Pipeline DSL

```yaml
# pipeline.yaml
name: build-test-deploy
description: "Build, test, and deploy an application"

stages:
  - name: build
    provider: podman
    template: "rust:latest"
    steps:
      - run: "cargo build --release"
    artifacts:
      - from: "/app/target/release/myapp"
        to: "build-artifact"

  - name: test
    provider: podman
    template: "rust:latest"
    depends_on: [build]
    steps:
      - run: "cargo test --release"
    artifacts:
      - from: "/app/target/release/myapp"
        to: "test-artifact"

  - name: deploy
    provider: firecracker
    template: "alpine:latest"
    depends_on: [test]
    steps:
      - write:
          path: "/app/myapp"
          artifact: "test-artifact"
      - run: "chmod +x /app/myapp && /app/myapp"

cleanup:
  terminate_all: true
```

### 6.3 Criterios de aceptación

- [ ] Pipeline con 3 stages se ejecuta end-to-end
- [ ] Artefactos se transfieren entre sandboxes de diferentes providers
- [ ] Pipeline falla correctamente si un stage falla
- [ ] Database sandbox permite crear branch y ejecutar queries aislados
- [ ] Cleanup automático de todos los sandboxes al terminar

---

## 7. Fase 5: Catálogo y Extensibilidad

**Duración**: 4-6 semanas  
**Objetivo**: Registro de "appliances" (imágenes con MCP servers), catálogo configurable

### 7.1 Tareas

| # | Tarea | Descripción | Prioridad | Estimación |
|---|-------|-------------|-----------|------------|
| **Appliance Registry** | | | | |
| 5.1 | Appliance spec | Definición de appliance (imagen + tools + config) | Alta | 3 días |
| 5.2 | Registry storage | Almacenamiento y búsqueda de appliances | Alta | 3 días |
| 5.3 | Dynamic tool discovery | Al arrancar sandbox, descubrir tools disponibles | Alta | 1 semana |
| 5.4 | Tool proxy | Exponer tools del sandbox como tools MCP del Gateway | Alta | 1 semana |
| **Templates** | | | | |
| 5.5 | Template system | Templates predefinidos: rust-dev, python-dev, node-dev | Media | 3 días |
| 5.6 | Custom templates | Crear templates desde Dockerfile | Media | 3 días |
| 5.7 | Template registry | Compartir templates entre equipos | Baja | 1 semana |
| **HTTP Transport** | | | | |
| 5.8 | Streamable HTTP server | rmcp transport HTTP para multi-agente | Alta | 3 días |
| 5.9 | Auth middleware | API keys, tokens para multi-tenant | Alta | 3 días |
| 5.10 | Rate limiting | Por agente/usuario | Media | 2 días |
| **Kubernetes Backend** | | | | |
| 5.11 | KubernetesBackend | Implementar `SandboxProvider` para K8s | Alta | 2 semanas |
| 5.12 | Pod pool | Deployment + Service preexistente con gRPC | Alta | 1 semana |
| 5.13 | Namespace isolation | Un namespace por agente/equipo | Media | 3 días |
| **Tests** | | | | |
| 5.14 | Tests appliance | Registro, descubrimiento, ejecución | Alta | 3 días |
| 5.15 | Tests HTTP transport | Multi-agente concurrente | Alta | 2 días |
| 5.16 | Tests K8s | Lifecycle en cluster real | Media | 3 días |

### 7.2 Concepto de Appliance

Un **Appliance** es una imagen de contenedor que incluye:

1. El `sandbox-worker` binary (gRPC server)
2. Un MCP server específico (ej. `mcp-server-filesystem`, `mcp-server-github`)
3. Herramientas del lenguaje (Python, Node.js, Rust toolchain)
4. Configuración del entorno

```yaml
# appliance.yaml
name: "python-dev"
description: "Python development environment with filesystem and code interpreter"
version: "1.0.0"

base_image: "python:3.13-slim"

install:
  - "pip install mcp-server-filesystem e2b-code-interpreter"

worker:
  binary: "/usr/local/bin/sandbox-worker"
  grpc_port: 50051

mcp_servers:
  - name: "filesystem"
    command: "mcp-server-filesystem"
    args: ["/workspace"]
  - name: "code-interpreter"
    command: "python"
    args: ["-m", "code_interpreter"]

tools:
  # Auto-discovered from MCP servers + sandbox worker tools
  - sandbox_run
  - sandbox_write
  - sandbox_read
  - read_file        # from filesystem MCP server
  - write_file       # from filesystem MCP server
  - run_code         # from code interpreter

resources:
  default_cpu: 1
  default_memory_mb: 1024
  default_disk_mb: 2048
```

### 7.3 Criterios de aceptación

- [ ] Appliance se registra y se descubre automáticamente
- [ ] Tools del MCP server dentro del sandbox se exponen como tools MCP del Gateway
- [ ] Streamable HTTP transport funciona con múltiples agentes concurrentes
- [ ] Kubernetes backend crea Pods efímeros y los gestiona
- [ ] Rate limiting previene abuso

---

## 8. Estimaciones y Dependencias

### 8.1 Resumen de estimaciones

| Fase | Duración estimada | Personas | Esfuerzo total |
|------|-------------------|----------|---------------|
| **0. PoC** | 1 semana | 1 | 21h |
| **1. MVP** | 2-3 semanas | 1 | 80-100h |
| **2. Multi-Backend** | 4-6 semanas | 1-2 | 160-200h |
| **3. Streaming** | 2-3 semanas | 1 | 60-80h |
| **4. Pipelines** | 4-8 semanas | 1-2 | 160-240h |
| **5. Catálogo** | 4-6 semanas | 1-2 | 160-200h |
| **Total** | **17-27 semanas** | **1-2** | **640-840h** |

### 8.2 Dependencias entre fases

```
Fase 0 (PoC) ────────► Fase 1 (MVP) ────────► Fase 2 (Multi-Backend)
                                                     │
                                                     ├──► Fase 3 (Streaming)
                                                     │
                                                     └──► Fase 4 (Pipelines)
                                                              │
                                                              └──► Fase 5 (Catálogo)
```

### 8.3 Dependencias externas

| Dependencia | Tipo | Riesgo |
|------------|------|--------|
| rmcp v1.5+ | Crate | Bajo — oficial, maduro |
| tonic v0.12+ | Crate | Bajo — estable |
| Podman | Sistema | Bajo — disponible en Linux |
| Firecracker | Sistema | Medio — requiere KVM |
| gVisor/runsc | Sistema | Medio — requiere Linux 4.14+ |
| Kubernetes cluster | Infra | Alto — setup complejo |
| Protobuf codegen | Build | Bajo — prost-build estable |

### 8.4 Hito mínimo viable para demo

**4 semanas desde Fase 0**:

```
Semana 1: Fase 0 (PoC) — Validación técnica
Semana 2: Fase 1 — Core types + Podman backend + Worker gRPC
Semana 3: Fase 1 — Gateway MCP completo + Tools
Semana 4: Fase 1 — Tests + Documentación

Resultado: Gateway MCP funcional con Podman, 5 tools, streaming básico
```

---

## 9. Criterios de Calidad

### 9.1 Test Coverage por fase

| Fase | Unit Tests | Integration Tests | E2E Tests |
|------|-----------|-------------------|-----------|
| 1 (MVP) | >80% core | Podman lifecycle | Agente→Gateway→Worker |
| 2 (Multi) | >80% providers | Firecracker + gVisor | Multi-provider |
| 3 (Stream) | Streaming logic | Progress delivery | Streaming E2E |
| 4 (Pipeline) | Pipeline engine | Artifact transfer | Full pipeline |
| 5 (Catalog) | Registry logic | Appliance discovery | Dynamic tools |

### 9.2 Performance targets

| Métrica | Target | Nota |
|---------|--------|------|
| Gateway throughput | >1,000 tool calls/segundo | Con sandboxes pre-creados |
| Latencia sandbox_create (hot pool) | <50ms | Pool caliente |
| Latencia sandbox_create (cold) | <2s (Podman) | Under load |
| Latencia sandbox_run | <100ms overhead | Sin contar ejecución |
| Streaming latency | <50ms por chunk | stdout/stderr |
| Memory overhead gateway | <100 MB | Sin sandboxes activos |
| Concurrent sandboxes | >100 | Por instancia de gateway |

### 9.3 Security checklist

- [ ] No se exponen credenciales del host al sandbox
- [ ] Environment variables cifradas en tránsito gRPC
- [ ] Rate limiting por agente/usuario
- [ ] Timeout enforcement — cleanup automático de sandboxes huérfanos
- [ ] Network isolation — sandboxes no pueden acceder a otros sandboxes
- [ ] Audit logging de todas las operaciones
- [ ] Container escape mitigation (Firecracker: kernel dedicado, gVisor: Sentry)
- [ ] No ejecutar como root (Podman rootless, Firecracker jailer)

### 9.4 Definition of Done por tarea

Cada tarea individual se considera "done" cuando:

1. ✅ Código implementado y compilando sin warnings
2. ✅ Tests unitarios pasando (>80% coverage de la nueva lógica)
3. ✅ Documentación inline (`///` doc comments en items públicos)
4. ✅ No regresiones en tests existentes
5. ✅ Code review completado (si aplica)
6. ✅ Config y defaults razonables
