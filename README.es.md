# 🏰 Bastion

<div align="center">

**Gateway MCP para Ejecución Segura de Agentes IA en Sandboxes**

[![Rust](https://img.shields.io/badge/rust-stable-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Licencia](https://img.shields.io/badge/licencia-Apache--2.0-green.svg)](LICENSE)
[![CI](https://github.com/Rubentxu/Bastion/actions/workflows/ci.yml/badge.svg)](https://github.com/Rubentxu/Bastion/actions/workflows/ci.yml)
[![Versión](https://img.shields.io/badge/versión-0.1.0-blue.svg)](https://github.com/Rubentxu/Bastion/releases)

</div>

---

**Bastion** es un Gateway [MCP](https://spec.modelcontextprotocol.io/) (Model Context Protocol) de código abierto que permite a los agentes de IA ejecutar herramientas de forma segura en entornos aislados — contenedores, microVMs o sandboxes a nivel de kernel. Construido en Rust con Domain-Driven Design (DDD) y Arquitectura Limpia (Clean Architecture).

## 📖 Tabla de Contenidos

- [¿Por qué Bastion?](#-por-qué-bastion)
- [Arquitectura](#-arquitectura)
- [Características](#-características)
- [Inicio Rápido](#-inicio-rápido)
- [Uso](#-uso)
  - [Con OpenCode](#con-opencode)
  - [Con Claude Code](#con-claude-code)
  - [Opciones CLI](#opciones-cli)
- [Herramientas MCP](#-herramientas-mcp)
- [Arquitectura en Profundidad](#-arquitectura-en-profundidad)
  - [Estructura de Crates DDD](#estructura-de-crates-ddd)
  - [Flujo de Datos](#flujo-de-datos)
- [Hoja de Ruta](#-hoja-de-ruta)
- [Desarrollo](#-desarrollo)
- [Contribuir](#-contribuir)
- [Licencia](#-licencia)

## 🤔 ¿Por qué Bastion?

Los agentes de IA necesitan ejecutar código, pero ejecutar código no confiable directamente en el host es peligroso. Los servidores MCP existentes típicamente ejecutan comandos en el mismo proceso o máquina — sin aislamiento, sin límites de recursos, sin limpieza.

**Bastion resuelve esto proporcionando un gateway compatible con MCP que actúa como intermediario seguro:**

```
Agente (Cliente MCP)
    │
    │  tools/call("sandbox_run", {command: "npm test"})
    ▼
Bastion Gateway ──▶ Contenedor Sandbox (Podman/Firecracker/gVisor)
    │                      │
    │  {exit_code: 0,       │  npm test
    │   stdout: "42 pasaron"}│  se ejecuta aislado
    ▼                      ▼
```

- **Aislamiento**: Cada comando se ejecuta en su propio contenedor o microVM
- **Control de recursos**: Límites de CPU, memoria y tiempo por sandbox
- **Estado limpio**: Sin fugas de estado entre ejecuciones
- **Nativo MCP**: Funciona con cualquier cliente compatible MCP (OpenCode, Claude Code, Goose, etc.)
- **Abstracción de proveedor**: Cambia de backend sin modificar el código del agente

## 🏗 Arquitectura

![Arquitectura de Bastion](docs/assets/diagrama.png)

## ✨ Características

| Característica | Estado | Descripción |
|----------------|--------|-------------|
| **Backend Podman** | ✅ Estable | Aislamiento basado en contenedores vía API bollard |
| **Backend Firecracker** | ✅ Implementado | Aislamiento microVM vía API REST de Firecracker sobre Unix socket |
| **Ejecución con Streaming** | ✅ Estable | Transmisión stdout/stderr en tiempo real durante comandos |
| **Pool Manager** | ✅ Estable | Contenedores pre-calentados para creación <200ms |
| **Abstracción de Proveedor** | ✅ Estable | ProviderFactory — cambia backends vía configuración |
| **Métricas Prometheus** | ✅ Estable | Conteo de sandboxes, latencia de comandos, tasas de error |
| **Health Checks** | ✅ Estable | Validación de conectividad de proveedor + pool |
| **Backend gVisor** | 🔜 Planeado | Sandboxing a nivel de kernel vía runsc |
| **Backend Kubernetes** | 🔜 Planeado | Sandboxes efímeros basados en Pods |

## 🚀 Inicio Rápido

### Requisitos previos

- **Rust** 1.80+ ([instalar](https://rustup.rs))
- **Podman** 4.x+ ([instalar](https://podman.io/docs/installation))

### 1. Clonar y Compilar

```bash
git clone https://github.com/Rubentxu/Bastion.git
cd Bastion
cargo build --release
```

### 2. Iniciar el Servicio Podman

```bash
# Crear directorio del socket e iniciar el servicio API
mkdir -p $XDG_RUNTIME_DIR/podman
podman system service --time 3600 unix://$XDG_RUNTIME_DIR/podman/podman.sock &
```

### 3. Ejecutar el Gateway

```bash
# Modo básico
./target/release/bastion-gateway \
  --image debian:bookworm-slim

# Con pool caliente (recomendado para producción)
./target/release/bastion-gateway \
  --image debian:bookworm-slim \
  --pool-enabled \
  --pool-min-idle 2 \
  --pool-max-idle 5
```

### 4. Conectar un Cliente MCP

Configura tu cliente MCP para usar el gateway Bastion. Consulta [Uso](#-uso) para configuraciones específicas de cada cliente.

## 📝 Uso

### Con OpenCode

Añade a `~/.config/opencode/config.toml`:

```toml
[[mcp_servers]]
name = "bastion"
command = "/ruta/a/bastion/target/release/bastion-gateway"
args = [
    "--pool-enabled",
    "--image", "debian:bookworm-slim"
]
```

Luego usa en cualquier sesión de OpenCode:

```
/sandbox_create template="debian:bookworm-slim"
/sandbox_run sandbox_id="abc123" command="python -c 'print(2+2)'"
/sandbox_read sandbox_id="abc123" path="/tmp/output.txt"
/sandbox_terminate sandbox_id="abc123"
```

### Con Claude Code

Añade a la configuración MCP de Claude Code:

```json
{
  "mcpServers": {
    "bastion": {
      "command": "/ruta/a/bastion/target/release/bastion-gateway",
      "args": [
        "--pool-enabled",
        "--image", "debian:bookworm-slim"
      ]
    }
  }
}
```

### Opciones CLI

```
bastion-gateway [OPCIONES]

Configuración del Sandbox:
  --socket <RUTA>       Ruta del socket Podman [por defecto: /run/user/1000/podman/podman.sock]
  --image <IMAGEN>      Imagen de contenedor por defecto [por defecto: debian:bookworm-slim]
  --config <RUTA>       Ruta del archivo de configuración [por defecto: config/sandbox-gateway.toml]

Opciones del Pool:
  --pool-enabled               Habilitar pool de sandboxes
  --pool-min-idle <N>          Mínimo de contenedores inactivos por plantilla [por defecto: 2]
  --pool-max-idle <N>          Máximo de contenedores inactivos por plantilla [por defecto: 5]
  --pool-max-total <N>         Máximo total de contenedores [por defecto: 50]
  --pool-idle-timeout-ms <MS>  Tiempo de expiración por inactividad [por defecto: 600000]
  --pool-refill-interval-ms <MS> Intervalo de relleno del pool [por defecto: 5000]
```

## 🔧 Herramientas MCP

Bastion expone 12 herramientas MCP para la gestión de sandboxes:

### Ciclo de Vida

| Herramienta | Parámetros | Retorna |
|-------------|------------|---------|
| `sandbox_create` | `template`, `timeout_ms` | `sandbox_id`, `status`, `from_pool` |
| `sandbox_terminate` | `sandbox_id` | `status` (`terminated` o `pooled`) |
| `sandbox_info` | `sandbox_id` | `sandbox_id`, `status`, `template`, `created_at`, `expires_at` |
| `sandbox_list` | — | `count`, `sandboxes[]` |

### Ejecución

| Herramienta | Parámetros | Retorna |
|-------------|------------|---------|
| `sandbox_run` | `sandbox_id`, `command` | `exit_code`, `stdout`, `stderr`, `duration_ms` |
| `sandbox_run_stream` | `sandbox_id`, `command` | `exit_code`, `stdout`, `stderr`, `chunks_received` |

### Operaciones de Archivos

| Herramienta | Parámetros | Retorna |
|-------------|------------|---------|
| `sandbox_write` | `sandbox_id`, `path`, `content` | `status` |
| `sandbox_read` | `sandbox_id`, `path` | `content`, `encoding` |
| `sandbox_list_files` | `sandbox_id`, `path` | `count`, `entries[]` |

### Observabilidad

| Herramienta | Parámetros | Retorna |
|-------------|------------|---------|
| `sandbox_health` | — | `status`, `version`, `checks[]` |
| `sandbox_metrics` | — | Métricas en formato Prometheus |
| `sandbox_pool_stats` | — | `enabled`, `active`, `idle`, `templates[]` |

## 🧬 Arquitectura en Profundidad

### Estructura de Crates DDD

| Crate | Capa | Responsabilidad |
|-------|------|-----------------|
| `bastion-domain` | Dominio | Entidades, value objects, traits (`SandboxProvider`, `SandboxRepository`) |
| `bastion-application` | Aplicación | Casos de uso (orquestación entre dominio e infraestructura) |
| `bastion-infrastructure` | Infraestructura | Adaptadores (`PodmanProvider`, `InMemoryRepo`, `PoolManager`, `Metrics`) |
| `bastion-gateway` | Presentación | Servidor MCP vía `rmcp`, raíz de composición, CLI |
| `bastion-worker` | Infraestructura | Runtime worker gRPC para agentes de ejecución en sandbox (planeado) |

### Flujo de Datos

```
┌──────────────┐     ┌─────────────────┐     ┌──────────────────┐
│ Cliente MCP  │────▶│ BastionGateway   │────▶│   Casos de Uso   │
│ (OpenCode,   │     │ (servidor rmcp)  │     │  (Aplicación)    │
│  Claude Code)│◀────│ 12 handlers      │◀────│                  │
└──────────────┘     └────────┬────────┘     └────────┬─────────┘
                              │                       │
                              ▼                       ▼
                     ┌────────────────┐     ┌──────────────────┐
                     │ ProviderFactory │     │ SandboxRepository │
                     │  (Podman,       │     │   (InMemory)      │
                     │   Firecracker,  │     └──────────────────┘
                     │   gVisor)       │
                     └───────┬────────┘
                             │
                             ▼
                     ┌────────────────┐
                     │ Runtime de     │
                     │ Contenedor/VM  │
                     └────────────────┘
```

## 🗺 Hoja de Ruta

| Versión | Hito | Contenido |
|---------|------|-----------|
| **v0.1.0** ✅ | MVP | Backend Podman, 12 herramientas, pool caliente, streaming, métricas |
| **v0.2.0** | Multi-backend | Pool Manager, backend Firecracker |
| **v0.3.0** | Multi-backend | Backend gVisor, selección de proveedor |
| **v0.4.0** | Streaming | Notificaciones de progreso MCP, cancelación |
| **v0.5.0** | Pipelines | Pipelines multi-sandbox basados en DSL |
| **v0.6.0** | Base de datos | Backends de sandbox PostgreSQL + SQLite |
| **v0.9.0** | Kubernetes | Sandboxes efímeros basados en Pods K8s |
| **v1.0.0** | Estable | Todas las características, API estable, publicación en crates.io |

Consulta [CHANGELOG.md](CHANGELOG.md) para notas detalladas de cada versión.

## 💻 Desarrollo

```bash
# Compilar
cargo build --release

# Ejecutar todos los tests
cargo test --workspace

# Ejecutar tests de integración (requiere Podman)
cargo test --test podman_lifecycle -- --test-threads=1

# Linting
cargo clippy --workspace -- -D warnings

# Formateo
cargo fmt --all -- --check

# Generar documentación
cargo doc --no-deps --document-private-items --open
```

### Estructura del Proyecto

```
Bastion/
├── crates/
│   ├── bastion-domain/         # Modelo de dominio + puertos
│   │   └── src/
│   │       ├── sandbox/        # Agregado Sandbox
│   │       ├── execution/      # Tipos de comando + streaming
│   │       ├── provider/       # Trait SandboxProvider
│   │       └── shared/         # DomainError, tipos Id
│   ├── bastion-application/    # Casos de uso
│   │   └── src/
│   │       ├── sandbox/        # Crear, terminar, listar, info
│   │       ├── execution/      # Ejecutar, ejecutar_stream
│   │       └── file_ops/       # Leer, escribir, listar_archivos
│   ├── bastion-infrastructure/ # Adaptadores
│   │   └── src/
│   │       ├── provider/       # PodmanProvider, ProviderFactory
│   │       ├── pool/           # SandboxPoolManager
│   │       ├── persistence/    # InMemorySandboxRepository
│   │       ├── metrics/        # GatewayMetrics
│   │       └── config/         # Cargador de configuración
│   ├── bastion-gateway/        # Servidor MCP
│   │   └── src/
│   │       ├── main.rs         # Raíz de composición + CLI
│   │       └── server.rs       # 12 handlers de herramientas MCP
│   └── bastion-worker/         # Worker gRPC (planeado)
├── docs/assets/                # Imágenes de documentación
├── config/                     # Configuraciones de ejemplo
├── proto/                      # Definiciones Protobuf
└── proyectos/                  # Documentos de planificación
```

## 🤝 Contribuir

¡Las contribuciones son bienvenidas! Consulta [CONTRIBUTING.md](CONTRIBUTING.md) para directrices sobre:

- Principios de arquitectura y diseño
- Estilo de código y convenciones
- Formato de mensajes de commit
- Lista de verificación para Pull Requests

## 📄 Licencia

Apache-2.0 — consulta [LICENSE](LICENSE) para más detalles.

---

<div align="center">

**Construido con Rust** 🦀 **·** **DDD** 🧬 **·** **MCP** 🔌

[🇬🇧 Read in English](README.md)

</div>
