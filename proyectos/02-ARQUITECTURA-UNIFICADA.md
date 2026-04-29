# Arquitectura Unificada: Gateway MCP con Sandboxes Remotos via gRPC

> **Fecha**: Abril 2026  
> **Estado**: Diseño  
> **Dependencias**: [01-INVESTIGACION-SANDBOX-MCP.md](./01-INVESTIGACION-SANDBOX-MCP.md)

---

## Tabla de Contenidos

1. [Visión General](#1-visión-general)
2. [Arquitectura del Sistema](#2-arquitectura-del-sistema)
3. [Componentes](#3-componentes)
4. [Contrato gRPC (Protobuf)](#4-contrato-grpc-protobuf)
5. [Trait SandboxProvider](#5-trait-sandboxprovider)
6. [Flujo de Ejecución](#6-flujo-de-ejecución)
7. [Seguridad](#7-seguridad)
8. [Configuración y Despliegue](#8-configuración-y-despliegue)
9. [Stack Tecnológico](#9-stack-tecnológico)

---

## 1. Visión General

### 1.1 Problema

Los agentes de IA necesitan ejecutar herramientas (tools) en entornos aislados por razones de
seguridad, reproducibilidad y escalabilidad. Actualmente:

- **E2B** es un servicio hosted — no self-hostable fácilmente
- **Vercel Sandbox** es vendor-locked — solo funciona en Vercel
- **Bolt/Lovable** son productos cerrados — no son infraestructura reusable
- No existe un **Gateway MCP open-source** que abstraiga múltiples backends

### 1.2 Solución

Un Gateway MCP escrito en Rust que:

1. Acepta conexiones de agentes via protocolo MCP (stdio/HTTP)
2. Expone tools de sandbox (`sandbox_run`, `sandbox_create`, `sandbox_write`, etc.)
3. Enruta las ejecuciones a workers en sandboxes remotos via gRPC
4. Soporta múltiples backends: Podman, Firecracker, gVisor, Kubernetes
5. Proporciona streaming de resultados via progress notifications MCP

### 1.3 Principios de diseño

- **MCP-first**: El Gateway es un MCP server estándar. Los agentes no necesitan saber
  que la ejecución es remota.
- **gRPC interno**: La comunicación gateway↔worker usa gRPC para eficiencia y streaming.
- **Pluggable backends**: Cada backend implementa el mismo `SandboxProvider` trait.
- **Seguridad por defecto**: Sandboxes aislados, credenciales inyectadas, networking restringido.
- **Observable**: Métricas, logs estructurados, tracing distribuido.

---

## 2. Arquitectura del Sistema

### 2.1 Diagrama de Alto Nivel

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Agente de IA                                │
│  (Claude, GPT, Gemini, OpenCode, Cursor, etc.)                     │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ MCP (stdio / Streamable HTTP)
                           │ JSON-RPC 2.0
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    GATEWAY MCP (Rust)                                │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
│  │  rmcp Server  │  │  Tool Router  │  │  Sandbox Pool Manager   │  │
│  │  (stdio/HTTP) │  │  (dispatch)   │  │  (hot pools, lifecycle) │  │
│  └──────────────┘  └──────┬───────┘  └──────────┬───────────────┘  │
│                           │                      │                   │
│  ┌────────────────────────┴──────────────────────┴───────────────┐  │
│  │                SandboxProvider Trait                           │  │
│  │  (create, run_command, write_file, read_file, terminate)      │  │
│  └────────────────────────┬──────────────────────────────────────┘  │
│                           │                                         │
│  ┌──────────┐  ┌─────────┴──┐  ┌──────────┐  ┌──────────────────┐  │
│  │  Podman   │  │ Firecracker │  │  gVisor  │  │  Kubernetes     │  │
│  │  Backend  │  │  Backend    │  │  Backend │  │  Backend        │  │
│  └─────┬────┘  └──────┬──────┘  └────┬─────┘  └───────┬────────┘  │
└────────┼──────────────┼──────────────┼─────────────────┼───────────┘
         │              │              │                 │
    ┌────▼────┐   ┌─────▼─────┐  ┌────▼────┐    ┌──────▼──────┐
    │ Podman  │   │Firecracker│  │ runsc   │    │  K8s Pod    │
    │ Container│  │ microVM   │  │ (gVisor)│    │  ephemeral  │
    │         │   │           │  │         │    │             │
    │ Worker  │   │  Worker   │  │ Worker  │    │   Worker    │
    │ (gRPC   │   │  (gRPC    │  │ (gRPC   │    │   (gRPC     │
    │  server)│   │   server) │  │  server)│    │    server)  │
    └─────────┘   └───────────┘  └─────────┘    └─────────────┘
```

### 2.2 Componentes lógicos

```
Gateway MCP
├── MCP Server (rmcp)
│   ├── Transport: stdio / Streamable HTTP
│   ├── Tools: sandbox_create, sandbox_run, sandbox_write, sandbox_read, sandbox_terminate
│   ├── Resources: sandbox://<id>/status, sandbox://<id>/logs
│   └── Prompts: sandbox_worksheet (plantilla de flujo)
├── Tool Router
│   ├── Dispatch por tool_name
│   ├── Validación de argumentos
│   └── Correlación con progress tokens
├── Sandbox Pool Manager
│   ├── Pool de sandboxes calientes
│   ├── Lifecycle management
│   ├── Timeout enforcement
│   └── Cleanup automático
├── SandboxProvider (trait)
│   ├── PodmanBackend
│   ├── FirecrackerBackend
│   ├── GVisorBackend
│   └── KubernetesBackend
└── gRPC Client (tonic)
    ├── Connection pooling
    ├── Load balancing
    └── Retry con backoff

Worker (dentro del sandbox)
├── gRPC Server (tonic)
│   ├── CreateSandbox (si el backend lo soporta)
│   ├── CallTool
│   ├── ReadFile / WriteFile
│   └── TerminateSandbox
├── Tool Executor
│   ├── Command runner (async process spawning)
│   ├── File I/O (tokio::fs)
│   └── Environment setup
└── Stream Manager
    ├── stdout/stderr capture
    ├── Progress reporting
    └── Result aggregation
```

---

## 3. Componentes

### 3.1 Gateway MCP

El Gateway es un **MCP server** estándar implementado con rmcp. Los agentes se conectan via
stdio o Streamable HTTP y ven un catálogo de tools de sandbox.

#### Tools expuestas al agente

| Tool | Descripción | Parámetros |
|------|-------------|-----------|
| `sandbox_create` | Crea un nuevo sandbox aislado | `template`, `env_vars`, `resources`, `timeout_ms` |
| `sandbox_run` | Ejecuta un comando en el sandbox | `sandbox_id`, `command`, `args`, `timeout_ms` |
| `sandbox_write` | Escribe un archivo en el sandbox | `sandbox_id`, `path`, `content` (base64) |
| `sandbox_read` | Lee un archivo del sandbox | `sandbox_id`, `path` |
| `sandbox_list_files` | Lista archivos en un directorio | `sandbox_id`, `directory` |
| `sandbox_terminate` | Destruye el sandbox | `sandbox_id` |
| `sandbox_snapshot` | Crea snapshot del estado (si soportado) | `sandbox_id` |
| `sandbox_info` | Obtiene información del sandbox | `sandbox_id` |

#### Resources expuestas

| Resource | Descripción |
|----------|-------------|
| `sandbox://{sandbox_id}/status` | Estado actual del sandbox |
| `sandbox://{sandbox_id}/logs` | Logs del sandbox (stream) |

### 3.2 Tool Router

El Tool Router despacha cada `tools/call` al backend adecuado:

```rust
pub struct SandboxToolRouter {
    providers: HashMap<String, Arc<dyn SandboxProvider>>,
    default_provider: String,
    sandbox_registry: Arc<RwLock<HashMap<String, SandboxEntry>>>,
}

struct SandboxEntry {
    sandbox_id: String,
    provider_name: String,
    sandbox_info: SandboxInfo,
    created_at: DateTime<Utc>,
    last_activity: DateTime<Utc>,
}
```

Flujo de dispatch:

1. Recibe `tools/call` de rmcp
2. Parsea el tool_name y extrae parámetros
3. Si la tool necesita un `sandbox_id`, busca en el registry qué provider lo gestiona
4. Si es `sandbox_create`, usa el provider default o el especificado en `template`
5. Traduce la operación a una llamada al `SandboxProvider` trait
6. Para operaciones con streaming, usa `notify_progress` de MCP

### 3.3 Sandbox Pool Manager

Gestiona el ciclo de vida de los sandboxes y optimiza latencia:

```rust
pub struct SandboxPoolManager {
    pools: HashMap<String, SandboxPool>,  // provider_name → pool
    config: PoolConfig,
    cleanup_handle: JoinHandle<()>,
}

struct SandboxPool {
    provider: Arc<dyn SandboxProvider>,
    hot_pool: Vec<SandboxInfo>,          // Sandboxes pre-creados listos para usar
    active: HashMap<String, SandboxEntry>, // Sandboxes en uso
    max_pool_size: usize,
    min_pool_size: usize,
}

struct PoolConfig {
    min_hot_per_provider: usize,   // Mínimo de sandboxes calientes por provider
    max_hot_per_provider: usize,   // Máximo de sandboxes calientes
    idle_timeout_ms: u64,          // Timeout para sandboxes idle en pool
    cleanup_interval_ms: u64,      // Intervalo de limpieza
}
```

Estrategias de pool:

- **Hot pool**: Sandboxes pre-creados y listos. Cuando un agente pide `sandbox_create`,
  se entrega uno del pool (latencia ~0ms) y se rellena el pool en background.
- **Warm pool**: Templates pre-cargados. El sandbox se crea bajo demanda pero la imagen
  ya está disponible localmente.
- **Cold**: Creación bajo demanda completa. Latencia depende del backend.

### 3.4 Worker gRPC

El Worker corre **dentro del sandbox** y es un servidor gRPC que recibe instrucciones
del Gateway:

```rust
pub struct SandboxWorker {
    sandbox_id: String,
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    active_commands: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
}
```

Responsabilidades del Worker:

1. Escuchar conexiones gRPC del Gateway
2. Ejecutar comandos (async process spawning con tokio)
3. Capturar stdout/stderr como stream
4. Leer/escribir archivos
5. Reportar progreso incremental
6. Manejar cancelaciones

---

## 4. Contrato gRPC (Protobuf)

### 4.1 Definición completa

```protobuf
syntax = "proto3";

package sandbox.v1;

// === Lifecycle ===

service SandboxWorker {
  // Sandbox lifecycle
  rpc CreateSandbox (CreateSandboxRequest) returns (SandboxInfo);
  rpc TerminateSandbox (TerminateSandboxRequest) returns (google.protobuf.Empty);
  rpc GetSandboxInfo (GetSandboxInfoRequest) returns (SandboxInfo);
  rpc ListSandboxes (ListSandboxesRequest) returns (ListSandboxesResponse);

  // Tool execution — streaming para stdout/stderr incremental
  rpc CallTool (CallToolRequest) returns (stream CallToolResponse);

  // File operations
  rpc ReadFile (ReadFileRequest) returns (ReadFileResponse);
  rpc WriteFile (WriteFileRequest) returns (WriteFileResponse);
  rpc ListFiles (ListFilesRequest) returns (ListFilesResponse);

  // Snapshots (opcional, depende del backend)
  rpc CreateSnapshot (CreateSnapshotRequest) returns (SnapshotInfo);
  rpc RestoreSnapshot (RestoreSnapshotRequest) returns (SandboxInfo);
}

// === Messages ===

message CreateSandboxRequest {
  string template_id = 1;               // Imagen o template base
  map<string, string> env_vars = 2;     // Variables de entorno (credenciales aquí)
  ResourcesSpec resources = 3;          // CPU, memoria, disco
  NetworkSpec network = 4;              // Reglas de red
  int64 timeout_ms = 5;                 // Timeout del sandbox
  string metadata = 6;                  // JSON arbitrario para tracking
}

message ResourcesSpec {
  int32 cpu_count = 1;                  // Número de vCPUs
  int64 memory_mb = 2;                  // Memoria en MiB
  int64 disk_mb = 3;                    // Disco en MiB
}

message NetworkSpec {
  bool allow_internet = 1;              // Acceso a Internet
  repeated string allowed_hosts = 2;    // Whitelist de hosts
  repeated string denied_hosts = 3;     // Blacklist de hosts
  bool expose_ports = 4;                // Exponer puertos del sandbox
  repeated int32 exposed_ports = 5;     // Puertos a exponer
}

message SandboxInfo {
  string sandbox_id = 1;
  string template_id = 2;
  SandboxStatus status = 3;
  int64 created_at_ms = 4;              // Unix timestamp millis
  int64 expires_at_ms = 5;              // Unix timestamp millis
  map<string, string> metadata = 6;
  ResourcesSpec resources = 7;
  NetworkSpec network = 8;
}

enum SandboxStatus {
  SANDBOX_STATUS_UNSPECIFIED = 0;
  SANDBOX_STATUS_PENDING = 1;
  SANDBOX_STATUS_RUNNING = 2;
  SANDBOX_STATUS_PAUSED = 3;
  SANDBOX_STATUS_STOPPED = 4;
  SANDBOX_STATUS_FAILED = 5;
}

message TerminateSandboxRequest {
  string sandbox_id = 1;
  bool force = 2;                       // Forzar terminación
}

message GetSandboxInfoRequest {
  string sandbox_id = 1;
}

message ListSandboxesRequest {
  string provider_name = 1;             // Filtrar por provider
  SandboxStatus status_filter = 2;      // Filtrar por estado
  int32 limit = 3;
  string cursor = 4;                    // Paginación
}

message ListSandboxesResponse {
  repeated SandboxInfo sandboxes = 1;
  string next_cursor = 2;
}

// === Tool Execution ===

message CallToolRequest {
  string sandbox_id = 1;
  string tool_name = 2;                 // Nombre de la tool MCP
  string arguments = 3;                 // JSON string con argumentos
  int64 timeout_ms = 4;                 // Timeout por operación
  string progress_token = 5;            // Para correlacionar con MCP progress
}

message CallToolResponse {
  ContentType type = 1;
  bytes data = 2;                       // Datos del chunk
  bool is_final = 3;                    // True si es el último chunk
  map<string, string> metadata = 4;     // Metadata adicional

  enum ContentType {
    CONTENT_TYPE_UNSPECIFIED = 0;
    STDOUT = 1;                          // Output estándar
    STDERR = 2;                          // Output de error
    EXIT_CODE = 3;                       // Código de salida (en data como i64 LE)
    RESULT = 4;                          // Resultado final
    ERROR = 5;                           // Error
    PROGRESS = 6;                        // Update de progreso
    FILE_CONTENT = 7;                    // Contenido de archivo
  }
}

// === File Operations ===

message ReadFileRequest {
  string sandbox_id = 1;
  string path = 2;
  int64 offset_bytes = 3;               // Para archivos grandes
  int64 max_bytes = 4;                  // Limitar tamaño de lectura
}

message ReadFileResponse {
  bytes content = 1;
  bool is_truncated = 2;
  string encoding = 3;                  // "utf-8", "base64", "binary"
}

message WriteFileRequest {
  string sandbox_id = 1;
  string path = 2;
  bytes content = 3;
  bool create_dirs = 4;                 // Crear directorios intermedios
  string encoding = 5;                  // "utf-8", "base64", "binary"
}

message WriteFileResponse {
  int64 bytes_written = 1;
}

message ListFilesRequest {
  string sandbox_id = 1;
  string directory = 2;
  bool recursive = 3;
  int32 limit = 4;
}

message ListFilesResponse {
  repeated FileEntry entries = 1;
}

message FileEntry {
  string path = 1;
  bool is_directory = 2;
  int64 size_bytes = 3;
  int64 modified_at_ms = 4;
  string permissions = 5;               // e.g., "rwxr-xr-x"
}

// === Snapshots ===

message CreateSnapshotRequest {
  string sandbox_id = 1;
  string snapshot_name = 2;             // Nombre descriptivo
  map<string, string> labels = 3;       // Labels para categorizar
}

message SnapshotInfo {
  string snapshot_id = 1;
  string sandbox_id = 2;
  string snapshot_name = 3;
  int64 created_at_ms = 4;
  int64 size_bytes = 5;
  map<string, string> labels = 6;
}

message RestoreSnapshotRequest {
  string snapshot_id = 1;
  map<string, string> env_vars = 2;     // Sobreescribir env vars
  int64 timeout_ms = 3;                 // Nuevo timeout
}
```

### 4.2 Mapping MCP ↔ gRPC

| Operación MCP | Tool MCP | RPC gRPC | Streaming |
|--------------|----------|----------|-----------|
| `tools/call("sandbox_create", ...)` | `sandbox_create` | `CreateSandbox` | No |
| `tools/call("sandbox_run", ...)` | `sandbox_run` | `CallTool` | Server streaming |
| `tools/call("sandbox_write", ...)` | `sandbox_write` | `WriteFile` | No |
| `tools/call("sandbox_read", ...)` | `sandbox_read` | `ReadFile` | No |
| `tools/call("sandbox_list_files", ...)` | `sandbox_list_files` | `ListFiles` | No |
| `tools/call("sandbox_terminate", ...)` | `sandbox_terminate` | `TerminateSandbox` | No |
| `tools/call("sandbox_snapshot", ...)` | `sandbox_snapshot` | `CreateSnapshot` | No |
| `tools/call("sandbox_info", ...)` | `sandbox_info` | `GetSandboxInfo` | No |

---

## 5. Trait SandboxProvider

### 5.1 Definición del Trait

```rust
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Result type alias for sandbox operations
pub type Result<T> = std::result::Result<T, SandboxError>;

/// Stream type for command output chunks
pub type CommandStream = Pin<Box<dyn Stream<Item = Result<CommandChunk>> + Send>>;

/// Universal trait for sandbox providers.
///
/// Implemented by: PodmanBackend, FirecrackerBackend, GVisorBackend, KubernetesBackend
#[async_trait]
pub trait SandboxProvider: Send + Sync + std::fmt::Debug {
    // ── Lifecycle ──────────────────────────────────────────────

    /// Create a new isolated sandbox with the given configuration.
    async fn create_sandbox(&self, config: &SandboxConfig) -> Result<SandboxInfo>;

    /// Terminate and clean up a sandbox.
    async fn terminate(&self, sandbox_id: &str) -> Result<()>;

    /// Check if a sandbox is still alive.
    async fn is_alive(&self, sandbox_id: &str) -> Result<bool>;

    /// List sandboxes managed by this provider.
    async fn list_sandboxes(&self, filter: &SandboxFilter) -> Result<Vec<SandboxInfo>>;

    /// Get detailed info about a specific sandbox.
    async fn get_info(&self, sandbox_id: &str) -> Result<SandboxInfo>;

    /// Update the sandbox timeout (extends or shortens lifetime).
    async fn set_timeout(&self, sandbox_id: &str, timeout_ms: u64) -> Result<()>;

    // ── Execution ──────────────────────────────────────────────

    /// Execute a command and wait for completion.
    async fn run_command(
        &self,
        sandbox_id: &str,
        cmd: &CommandSpec,
    ) -> Result<CommandResult>;

    /// Execute a command with streaming output (stdout/stderr chunks).
    async fn run_command_stream(
        &self,
        sandbox_id: &str,
        cmd: &CommandSpec,
    ) -> Result<CommandStream>;

    // ── File Operations ────────────────────────────────────────

    /// Write content to a file inside the sandbox.
    async fn write_file(
        &self,
        sandbox_id: &str,
        path: &str,
        content: &[u8],
    ) -> Result<()>;

    /// Read content from a file inside the sandbox.
    async fn read_file(
        &self,
        sandbox_id: &str,
        path: &str,
    ) -> Result<Vec<u8>>;

    /// List files in a directory inside the sandbox.
    async fn list_files(
        &self,
        sandbox_id: &str,
        dir: &str,
    ) -> Result<Vec<FileEntry>>;

    // ── Snapshots (optional) ───────────────────────────────────

    /// Create a snapshot of the sandbox state.
    /// Returns Err if the backend doesn't support snapshots.
    async fn create_snapshot(&self, sandbox_id: &str) -> Result<SnapshotInfo> {
        Err(SandboxError::UnsupportedOperation("snapshots"))
    }

    /// Restore a sandbox from a snapshot.
    /// Returns Err if the backend doesn't support snapshots.
    async fn restore_snapshot(&self, snapshot_id: &str) -> Result<SandboxInfo> {
        Err(SandboxError::UnsupportedOperation("snapshots"))
    }

    // ── Capabilities ───────────────────────────────────────────

    /// Report what this provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Human-readable name for logging and config.
    fn name(&self) -> &str;
}
```

### 5.2 Tipos de datos

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub template_id: String,
    pub env_vars: HashMap<String, String>,
    pub resources: ResourcesSpec,
    pub network: NetworkSpec,
    pub timeout_ms: u64,
    pub metadata: HashMap<String, String>,
    pub provider_hint: Option<String>,  // Sugerir un provider específico
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesSpec {
    pub cpu_count: u32,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

impl Default for ResourcesSpec {
    fn default() -> Self {
        Self {
            cpu_count: 1,
            memory_mb: 512,
            disk_mb: 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSpec {
    pub allow_internet: bool,
    pub allowed_hosts: Vec<String>,
    pub denied_hosts: Vec<String>,
    pub expose_ports: bool,
    pub exposed_ports: Vec<u16>,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            allow_internet: true,
            allowed_hosts: vec![],
            denied_hosts: vec![],
            expose_ports: false,
            exposed_ports: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInfo {
    pub sandbox_id: String,
    pub template_id: String,
    pub status: SandboxStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: HashMap<String, String>,
    pub resources: ResourcesSpec,
    pub network: NetworkSpec,
    pub provider_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SandboxStatus {
    Pending,
    Running,
    Paused,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env_vars: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandChunk {
    pub chunk_type: ChunkType,
    pub data: Vec<u8>,
    pub is_final: bool,

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum ChunkType {
        Stdout,
        Stderr,
        ExitCode,
        Progress,
        Error,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub permissions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub snapshot_id: String,
    pub sandbox_id: String,
    pub snapshot_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub size_bytes: u64,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_snapshots: bool,
    pub supports_streaming: bool,
    pub supports_pause_resume: bool,
    pub max_timeout_ms: u64,
    pub max_memory_mb: u64,
    pub max_cpu_count: u32,
    pub supports_networking: bool,
    pub requires_kvm: bool,
    pub avg_startup_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxFilter {
    pub provider_name: Option<String>,
    pub status: Option<SandboxStatus>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Sandbox not found: {0}")]
    NotFound(String),

    #[error("Sandbox timeout: {0}")]
    Timeout(String),

    #[error("Sandbox already exists: {0}")]
    AlreadyExists(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(&'static str),

    #[error("Provider unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Command failed with exit code {exit_code}: {stderr}")]
    CommandFailed { exit_code: i32, stderr: String },

    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}
```

### 5.3 Implementación: PodmanBackend (referencia)

```rust
use async_trait::async_trait;
use std::sync::Arc;

pub struct PodmanBackend {
    client: podman_api::Podman,
    config: PodmanConfig,
    grpc_endpoint: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PodmanConfig {
    pub socket_path: String,               // Unix socket de Podman
    pub default_image: String,             // Imagen base
    pub network_mode: String,              // "bridge", "host", "none"
    pub rootless: bool,                    // Ejecución rootless
    pub hot_pool_size: usize,              // Tamaño del pool caliente
}

#[async_trait]
impl SandboxProvider for PodmanBackend {
    async fn create_sandbox(&self, config: &SandboxConfig) -> Result<SandboxInfo> {
        let container_id = self.client
            .containers()
            .create(&ContainerCreate {
                image: &config.template_id,
                env: config.env_vars.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect(),
                // Inyectar worker binary + gRPC server
                cmd: vec![
                    "/usr/local/bin/sandbox-worker",
                    "--grpc-addr", &self.grpc_endpoint,
                ],
                // Recursos
                resource_limits: Some(ResourceLimits {
                    cpu_count: config.resources.cpu_count,
                    memory_mb: config.resources.memory_mb,
                    disk_mb: config.resources.disk_mb,
                }),
                // Red
                network: if config.network.allow_internet {
                    NetworkMode::Bridge
                } else {
                    NetworkMode::None
                },
                ..Default::default()
            })
            .await?;

        // Iniciar el contenedor
        self.client.containers()
            .get(&container_id)
            .start()
            .await?;

        Ok(SandboxInfo {
            sandbox_id: container_id,
            template_id: config.template_id.clone(),
            status: SandboxStatus::Running,
            created_at: chrono::Utc::now(),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::milliseconds(config.timeout_ms as i64)),
            metadata: config.metadata.clone(),
            resources: config.resources.clone(),
            network: config.network.clone(),
            provider_name: self.name().to_string(),
        })
    }

    async fn run_command_stream(
        &self,
        sandbox_id: &str,
        cmd: &CommandSpec,
    ) -> Result<CommandStream> {
        // Conectar al worker gRPC dentro del contenedor
        let mut client = self.get_grpc_client(sandbox_id).await?;

        let request = tonic::Request::new(CallToolRequest {
            sandbox_id: sandbox_id.to_string(),
            tool_name: "run_command".to_string(),
            arguments: serde_json::to_string(cmd)?,
            timeout_ms: cmd.timeout_ms.unwrap_or(30_000),
            progress_token: String::new(),
        });

        let response = client.call_tool(request).await?;
        let stream = response.into_inner().map(|result| {
            result.map_err(SandboxError::Grpc).and_then(|r| {
                Ok(CommandChunk {
                    chunk_type: match r.r#type {
                        1 => ChunkType::Stdout,
                        2 => ChunkType::Stderr,
                        3 => ChunkType::ExitCode,
                        5 => ChunkType::Progress,
                        6 => ChunkType::Error,
                        _ => ChunkType::Stdout,
                    },
                    data: r.data,
                    is_final: r.is_final,
                })
            })
        });

        Ok(Box::pin(stream))
    }

    async fn terminate(&self, sandbox_id: &str) -> Result<()> {
        self.client
            .containers()
            .get(sandbox_id)
            .remove(&ContainerRemoveOpts::builder().force(true).build())
            .await?;
        Ok(())
    }

    async fn is_alive(&self, sandbox_id: &str) -> Result<bool> {
        let inspect = self.client
            .containers()
            .get(sandbox_id)
            .inspect()
            .await?;
        Ok(inspect.state.running)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,  // 24h
            max_memory_mb: 16_384,        // 16 GB
            max_cpu_count: 16,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 1500,          // ~1.5s
        }
    }

    fn name(&self) -> &str { "podman" }

    // ... otros métodos implementados similarmente
}
```

---

## 6. Flujo de Ejecución

### 6.1 Flujo completo: sandbox_run con streaming

```
Tiempo ──────────────────────────────────────────────────────────────────►

Agente          Gateway MCP             Provider (Podman)      Worker (en sandbox)
  │                 │                        │                      │
  │  tools/call     │                        │                      │
  │  ("sandbox_run",│                        │                      │
  │   {sandbox_id:  │                        │                      │
  │    "abc123",    │                        │                      │
  │    command:     │                        │                      │
  │    "npm test"}) │                        │                      │
  │────────────────►│                        │                      │
  │                 │                        │                      │
  │                 │  Validación            │                      │
  │                 │  - sandbox existe?     │                      │
  │                 │  - provider correcto?  │                      │
  │                 │  - permisos?           │                      │
  │                 │                        │                      │
  │                 │  run_command_stream()  │                      │
  │                 │───────────────────────►│                      │
  │                 │                        │  gRPC CallTool       │
  │                 │                        │─────────────────────►│
  │                 │                        │                      │
  │                 │                        │                Ejecuta "npm test"
  │                 │                        │                      │
  │  notify_        │  stream chunk          │  stream chunk        │  STDOUT "running test 1..."
  │  progress       │◄──────────────────────│◄─────────────────────│
  │◄────────────────│                        │                      │
  │                 │                        │                      │
  │  notify_        │  stream chunk          │  stream chunk        │  STDOUT "✓ test 1 passed"
  │  progress       │◄──────────────────────│◄─────────────────────│
  │◄────────────────│                        │                      │
  │                 │                        │                      │
  │  notify_        │  stream chunk          │  stream chunk        │  PROGRESS {80%}
  │  progress       │◄──────────────────────│◄─────────────────────│
  │◄────────────────│                        │                      │
  │                 │                        │                      │
  │                 │                        │  stream chunk        │  STDERR "Warning: deprecated"
  │                 │  stream chunk          │◄─────────────────────│
  │                 │◄──────────────────────│                      │
  │                 │                        │                      │
  │                 │                        │  stream chunk        │  EXIT_CODE {0}
  │                 │  stream chunk          │◄─────────────────────│
  │                 │◄──────────────────────│                      │
  │                 │                        │                      │
  │                 │  stream chunk          │  stream chunk        │  RESULT {is_final: true}
  │                 │◄──────────────────────│◄─────────────────────│
  │                 │                        │                      │
  │                 │  Construye respuesta   │                      │
  │                 │  MCP final             │                      │
  │  tools/call     │                        │                      │
  │  response       │                        │                      │
  │◄────────────────│                        │                      │
  │                 │                        │                      │
```

### 6.2 Flujo: sandbox_create con pool caliente

```
Tiempo ──────────────────────────────────────────────────────────────────►

Agente          Gateway MCP             Pool Manager           Podman
  │                 │                        │                      │
  │  tools/call     │                        │                      │
  │  ("sandbox_     │                        │                      │
  │   create",...)  │                        │                      │
  │────────────────►│                        │                      │
  │                 │                        │                      │
  │                 │  checkout_sandbox()    │                      │
  │                 │───────────────────────►│                      │
  │                 │                        │                      │
  │                 │  Sandbox del pool      │                      │
  │  SandboxInfo    │  (latencia ~0ms)       │                      │
  │◄────────────────│◄───────────────────────│                      │
  │                 │                        │                      │
  │                 │                        │  Rellenar pool       │
  │                 │                        │  en background       │
  │                 │                        │─────────────────────►│
  │                 │                        │  create + start      │
  │                 │                        │◄─────────────────────│
  │                 │                        │                      │
```

### 6.3 Flujo: Pipeline multi-sandbox

```
Agente          Gateway MCP
  │                 │
  │ sandbox_create  │  → Crea sandbox "build" (Podman)
  │────────────────►│     sandbox_id: "build-001"
  │◄────────────────│
  │                 │
  │ sandbox_run     │  → "cargo build --release" en build-001
  │────────────────►│     Stream de progreso...
  │◄────────────────│     exit_code: 0
  │                 │
  │ sandbox_run     │  → "cargo test" en build-001
  │────────────────►│     Stream de progreso...
  │◄────────────────│     exit_code: 0
  │                 │
  │ sandbox_read    │  → Lee /app/target/release/myapp
  │────────────────►│     Recupera binario (bytes)
  │◄────────────────│
  │                 │
  │ sandbox_create  │  → Crea sandbox "deploy" (Firecracker)
  │────────────────►│     sandbox_id: "deploy-001"
  │◄────────────────│
  │                 │
  │ sandbox_write   │  → Escribe binario en deploy-001
  │────────────────►│     /app/myapp ← bytes del binario
  │◄────────────────│
  │                 │
  │ sandbox_run     │  → "./myapp" en deploy-001
  │────────────────►│     Stream de output...
  │◄────────────────│     exit_code: 0
  │                 │
  │ sandbox_terminate│ → Destruye build-001
  │────────────────►│
  │◄────────────────│
  │                 │
  │ sandbox_terminate│ → Destruye deploy-001
  │────────────────►│
  │◄────────────────│
```

---

## 7. Seguridad

### 7.1 Modelo de seguridad por capas

```
Capa 1: Aislamiento del sandbox
  ├── Kernel dedicado (Firecracker) / Sentry (gVisor) / Namespaces (Podman)
  ├── Filesystem privado (destruido al terminar)
  ├── Network namespace (acceso controlado)
  └── Rate limiting (CPU, memoria, disco, red)

Capa 2: Inyección de credenciales
  ├── Environment variables (cifradas en tránsito)
  ├── Secret mounts (archivos desde vault, nunca expuestos al Gateway)
  ├── OIDC tokens (para auth con servicios externos)
  └── Nunca se exponen credenciales del host al sandbox

Capa 3: Control de acceso del Gateway
  ├── Validación de todas las tool calls
  ├── Rate limiting por agente/usuario
  ├── Audit logging de todas las operaciones
  └── Timeout enforcement (cleanup automático)

Capa 4: Red
  ├── Firewall por sandbox (allowlist/denylist)
  ├── Sin acceso a metadata service del host
  ├── Sin acceso a otros sandboxes
  └── DNS filtering opcional
```

### 7.2 Inyección de credenciales

```rust
/// Credential injection strategies
pub enum CredentialSource {
    /// Pasar como environment variable al sandbox
    /// Se cifran en tránsito gRPC y se inyectan al crear el sandbox
    EnvironmentVariable {
        key: String,
        value_ref: SecretRef,  // Referencia al vault, nunca el valor plano
    },

    /// Montar como archivo en el sandbox
    /// El worker lee el secret del vault y lo escribe en un tmpfs
    FileMount {
        path: String,           // Path dentro del sandbox
        content_ref: SecretRef,
        permissions: String,    // e.g., "0400"
    },

    /// OIDC token para auth con servicios externos
    OidcToken {
        audience: String,       // Target service
        scope: Vec<String>,
    },
}

/// Reference to a secret (never contains the actual value)
pub struct SecretRef {
    pub vault: String,          // e.g., "hashicorp", "aws-secrets"
    pub path: String,           // e.g., "prod/database/password"
    pub version: Option<u32>,
}
```

### 7.3 Configuración de red

```rust
/// Network policy for a sandbox
pub struct NetworkPolicy {
    /// Default outbound internet access
    pub allow_internet: bool,

    /// Specific hosts to allow (takes precedence over deny)
    pub allowed_hosts: Vec<HostPattern>,

    /// Specific hosts to deny
    pub denied_hosts: Vec<HostPattern>,

    /// Whether to expose ports publicly
    pub expose_ports: bool,

    /// Ports to expose (only if expose_ports is true)
    pub exposed_ports: Vec<PortSpec>,
}

pub struct HostPattern {
    pub host: String,           // e.g., "api.example.com", "*.github.com"
    pub port_range: Option<(u16, u16)>,  // Optional port restriction
}

pub struct PortSpec {
    pub port: u16,
    pub protocol: String,       // "tcp", "udp"
    pub public: bool,           // Accessible from outside?
}
```

---

## 8. Configuración y Despliegue

### 8.1 Configuración del Gateway (TOML)

```toml
# sandbox-gateway.toml

[server]
# MCP transport: "stdio" or "http"
transport = "http"
http_addr = "0.0.0.0:8080"

# Default provider when not specified
default_provider = "podman"

[providers.podman]
enabled = true
socket_path = "/run/podman/podman.sock"
default_image = "sandbox-worker:latest"
network_mode = "bridge"
rootless = true
hot_pool_size = 3

[providers.firecracker]
enabled = false
kernel_path = "/opt/firecracker/vmlinux"
rootfs_path = "/opt/firecracker/rootfs.ext4"
firecracker_bin = "/usr/local/bin/firecracker"
jailer_bin = "/usr/local/bin/jailer"
hot_pool_size = 5

[providers.gvisor]
enabled = false
runsc_bin = "/usr/local/bin/runsc"
default_image = "sandbox-worker:latest"
hot_pool_size = 2

[providers.kubernetes]
enabled = false
namespace = "sandboxes"
image = "sandbox-worker:latest"
service_account = "sandbox-worker"
hot_pool_size = 0  # K8s no usa hot pool

[pool]
min_hot_per_provider = 1
max_hot_per_provider = 10
idle_timeout_ms = 300_000       # 5 minutos
cleanup_interval_ms = 60_000    # 1 minuto

[security]
max_timeout_ms = 86_400_000     # 24 horas
default_timeout_ms = 3_600_000  # 1 hora
max_memory_mb = 16_384          # 16 GB
max_cpu_count = 16
audit_log_path = "/var/log/sandbox-gateway/audit.jsonl"

[grpc]
max_message_size = 67_108_864   # 64 MiB
connect_timeout_ms = 5_000
request_timeout_ms = 300_000    # 5 minutos

[logging]
level = "info"
format = "json"                 # "json" or "text"
```

### 8.2 Imagen Docker del Worker

```dockerfile
# Dockerfile.sandbox-worker
FROM rust:1.85-slim AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --bin sandbox-worker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/sandbox-worker /usr/local/bin/

# El worker escucha gRPC en este puerto
EXPOSE 50051

ENTRYPOINT ["sandbox-worker"]
CMD ["--grpc-addr", "0.0.0.0:50051"]
```

### 8.3 Despliegue del Gateway

```dockerfile
# Dockerfile.sandbox-gateway
FROM rust:1.85-slim AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --bin sandbox-gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/sandbox-gateway /usr/local/bin/
COPY sandbox-gateway.toml /etc/sandbox-gateway/config.toml

EXPOSE 8080

ENTRYPOINT ["sandbox-gateway"]
CMD ["--config", "/etc/sandbox-gateway/config.toml"]
```

---

## 9. Stack Tecnológico

### 9.1 Crates Rust

| Componente | Crate | Versión | Propósito |
|-----------|-------|---------|-----------|
| MCP Server/Client | `rmcp` | 1.5.0 | SDK oficial MCP |
| gRPC | `tonic` | 0.12+ | Framework gRPC |
| Protocol Buffers | `prost` | 0.13+ | Serialización protobuf |
| Async runtime | `tokio` | 1.x | Runtime async |
| Serialization | `serde` + `serde_json` | 1.x | JSON serialization |
| Logging | `tracing` + `tracing-subscriber` | 0.1.x | Structured logging |
| CLI | `clap` | 4.x | Argument parsing |
| Config | `config-rs` | 0.14+ | Config file parsing |
| HTTP client | `reqwest` | 0.13+ | Firecracker API calls |
| Container API | `bollard` o `podman-api` | latest | Docker/Podman API |
| K8s client | `kube` | 0.97+ | Kubernetes API |
| Error handling | `thiserror` + `anyhow` | latest | Error types |
| DateTime | `chrono` | 0.4.x | Timestamps |
| UUID | `uuid` | 1.x | Sandbox IDs |

### 9.2 Herramientas de desarrollo

| Herramienta | Propósito |
|------------|-----------|
| `cargo-nextest` | Test runner mejorado |
| `cargo-tarpaulin` | Coverage |
| `grpcurl` | Testing gRPC endpoints |
| `podman` | Container runtime |
| `firecracker` | microVM runtime |
| `runsc` | gVisor runtime |

### 9.3 Estructura del proyecto

```
sandbox-gateway/
├── Cargo.toml                    # Workspace
├── crates/
│   ├── sandbox-core/             # Tipos, traits, errores compartidos
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── types.rs          # SandboxConfig, SandboxInfo, etc.
│   │   │   ├── traits.rs         # SandboxProvider trait
│   │   │   ├── error.rs          # SandboxError
│   │   │   └── proto/            # Protobuf definitions
│   │   └── Cargo.toml
│   │
│   ├── sandbox-gateway/          # Binario: MCP Gateway
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── server.rs         # rmcp MCP server
│   │   │   ├── router.rs         # Tool routing
│   │   │   ├── pool.rs           # Sandbox pool manager
│   │   │   └── config.rs         # Config parsing
│   │   └── Cargo.toml
│   │
│   ├── sandbox-worker/           # Binario: gRPC Worker (dentro del sandbox)
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── grpc_server.rs    # tonic gRPC server
│   │   │   ├── executor.rs       # Command execution
│   │   │   └── file_ops.rs       # File read/write
│   │   └── Cargo.toml
│   │
│   └── sandbox-providers/        # Provider implementations
│       ├── src/
│       │   ├── lib.rs
│       │   ├── podman.rs         # PodmanBackend
│       │   ├── firecracker.rs    # FirecrackerBackend
│       │   ├── gvisor.rs         # GVisorBackend
│       │   └── kubernetes.rs     # KubernetesBackend
│       └── Cargo.toml
│
├── proto/
│   └── sandbox/v1/
│       └── sandbox.proto         # Protobuf definition
│
├── config/
│   └── sandbox-gateway.toml      # Default config
│
├── images/
│   ├── Dockerfile.sandbox-gateway
│   └── Dockerfile.sandbox-worker
│
├── tests/
│   ├── integration/
│   │   ├── test_podman.rs
│   │   ├── test_streaming.rs
│   │   └── test_lifecycle.rs
│   └── fixtures/
│       └── test-config.toml
│
└── docs/
    ├── architecture.md
    ├── security.md
    └── configuration.md
```
