# Bastion Worker Protocol v2 — Mejoras Sobre Jenkins Remoting

> **Fecha**: 2026-04-30
> **Estado**: Diseño de mejoras
> **Base**: `docs/research/jenkins-remoting-analysis.md`
> **Objetivo**: Recrear todas las buenas prácticas de JNLP y mejorar en cada dimensión

---

## Dimensiones de Análisis

| # | Dimensión | Jenkins JNLP | Bastion v1 (actual) | Bastion v2 (propuesta) |
|---|-----------|-------------|--------------------|-----------------------|
| 1 | Protocolo | Java Serialization custom | Protobuf básico | **Protobuf + gRPC avanzado** |
| 2 | Seguridad | TLS + Secret estático | Sin seguridad | **Defense-in-depth (5 capas)** |
| 3 | Estabilidad | Reconnect manual | Sin reconnect | **State machine + auto-healing** |
| 4 | Eficiencia | Full objects, sin compress | Sin optimización | **Streaming + compress + zero-copy** |
| 5 | Rendimiento | JVM + JIT warmup | Sin optimizar | **Static binary + async + SIMD-ready** |

---

## 1. PROTOCOLO — Mejoras sobre JNLP4-connect

### 1.1 Lo que Jenkins hace bien (recrear)

| Práctica | Cómo la recreamos en Bastion |
|----------|------------------------------|
| **Outbound connection** | Worker → Gateway (ya diseñado) |
| **Protocol negotiation** | Capability exchange en Register |
| **Filter stack** (capas) | gRPC interceptors (auth, rate-limit, logging, metrics) |
| **Acknowledgment layer** | ACK explícito por command_id |
| **Capability advertising** | WorkerCapabilities en RegisterRequest |

### 1.2 Lo que Jenkins hace mal (mejorar)

#### Problema: Java Serialization es inseguro y lento
**Jenkins**: Serializa objetos Java completos (Callable<T>) y los envía por el wire. Permitió deserialization attacks (SECURITY-218, 2015).

**Bastion v2**: Protobuf con schema estricto. Solo comandos explícitos:
```protobuf
// Solo estas operaciones existen. No hay RPC genérico.
oneof payload {
    RunCommandRequest run = 10;
    ReadFileRequest read = 11;
    WriteFileRequest write = 12;
    ListFilesRequest list = 13;
    PingRequest ping = 14;
    ShutdownRequest shutdown = 15;
}
```

#### Problema: No hay correlación request-response
**Jenkins**: Los Commands van por un stream sin ID. La correlación es implícita por orden.

**Bastion v2**: Cada comando tiene `command_id` (UUID) y se correlaciona con `PendingCommands` map:
```
Gateway ─── GatewayCommand(command_id=abc123, run=...) ──► Worker
Worker  ─── WorkerMessage(command_id=abc123, stdout=...) ──► Gateway
Worker  ─── WorkerMessage(command_id=abc123, exit=...)   ──► Gateway
```

#### Problema: No hay control de flujo
**Jenkins**: Si el output es muy grande, se almacena en memoria. OOM en agentes con poca RAM.

**Bastion v2**: 
- Worker anuncia `max_output_bytes` en capabilities
- Archivos grandes se transfieren en chunks de 4MB
- gRPC HTTP/2 flow control nativo
- `WriteFileRequest` con chunking:

```protobuf
message WriteFileRequest {
  string path = 1;
  int32 mode = 2;
  int64 total_size = 3;
  int32 chunk_index = 4;        // NEW: para archivos grandes
  int32 total_chunks = 5;       // NEW: -1 = unknown (streaming)
  bytes content = 6;            // Max 4MB por chunk
}
```

#### Problema: Protocolo monolítico (JNLP1→2→3→4→WS)
**Jenkins**: Cada versión de protocolo es incompatible. Migración dolorosa.

**Bastion v2**: Versionado semántico en la negociación:
```protobuf
message RegisterRequest {
  string sandbox_id = 1;
  ProtocolVersion protocol_version = 2;  // SemVer
  WorkerCapabilities capabilities = 3;
  // ...
}

message ProtocolVersion {
  int32 major = 1;  // Breaking changes
  int32 minor = 2;  // New features (backward compatible)
  int32 patch = 3;  // Bug fixes
}
```

### 1.3 Nuevo: Compression

Jenkins no comprime el tráfico. gRPC soporta compression per-message:

```rust
// Gateway config
let channel = Channel::from_shared(gateway_url)
    .await?
    .http2_adaptive_window(true)
    .initial_stream_window_size(1024 * 1024)  // 1MB window
    .connect()
    .await?;

// Enable compression
tonic::transport::Server::builder()
    .accept_compressed(CompressionEncoding::Gzip)
    .send_compressed(CompressionEncoding::Gzip)
    .add_service(registry_svc)
    .serve(addr)
    .await?;
```

**Impacto estimado**: 60-80% reducción en bandwidth para texto (stdout, source code). Mínimo overhead para binario.

---

## 2. SEGURIDAD — Defense in Depth

### 2.1 Las 5 capas de seguridad de Bastion v2

```
┌─────────────────────────────────────────────────────────┐
│ Layer 5: APPLICATION    │ Comandos explícitos (whitelist) │
│                         │ No hay RPC genérico              │
│                         │ Path traversal protection        │
├─────────────────────────┤ Rate limiting por sandbox        │
│ Layer 4: AUTHORIZATION  │ Session token (JWT)              │
│                         │ Command allowlist per sandbox    │
│                         │ Sandbox isolation (chroot)       │
├─────────────────────────┤ Challenge-response auth          │
│ Layer 3: AUTHENTICATION │ Secret por sandbox (no reutilizable) │
│                         │ HMAC proof-of-possession         │
├─────────────────────────┤ TLS 1.3 (rustls)                 │
│ Layer 2: ENCRYPTION     │ Certificate pinning              │
│                         │ Mandatory (no plaintext mode)    │
├─────────────────────────┤ gRPC over HTTP/2                  │
│ Layer 1: TRANSPORT      │ Unix socket (mismo host)         │
│                         │ or TCP (remote)                  │
└─────────────────────────────────────────────────────────┘
```

### 2.2 Challenge-Response Authentication (nuevo vs Jenkins)

**Jenkins**: Envía el secret en texto (aunque sobre TLS).

**Bastion v2**: Challenge-response con HMAC. El secret **nunca** transita por el wire:

```
Worker                                      Gateway
  │                                            │
  │── RegisterRequest(sandbox_id, nonce_w) ──►│
  │                                            │ genera nonce_g
  │◄── RegisterResponse(challenge=nonce_g) ───│
  │                                            │
  │   HMAC(secret, nonce_w + nonce_g)          │
  │── ChallengeResponse(proof=HMAC...) ───────►│
  │                                            │ verifica HMAC
  │◄── RegisterResponse(accepted, token) ──────│
```

```protobuf
message RegisterRequest {
  string sandbox_id = 1;
  ProtocolVersion protocol_version = 2;
  WorkerCapabilities capabilities = 3;
  bytes worker_nonce = 4;       // 32 bytes random
}

message RegisterResponse {
  enum Status {
    ACCEPTED = 0;
    CHALLENGE = 1;              // Gateway challenges the worker
    REJECTED = 2;
  }
  Status status = 1;
  bytes gateway_nonce = 2;      // 32 bytes random (present if CHALLENGE)
  string session_token = 3;     // JWT (present if ACCEPTED)
  int64 heartbeat_interval_ms = 4;
  int64 command_timeout_ms = 5;
}

message ChallengeResponse {
  bytes proof = 1;              // HMAC-SHA256(secret, worker_nonce || gateway_nonce)
}
```

### 2.3 Session Token (JWT)

**Jenkins**: Sin concepto de sesión. Si el agent conecta, está aceptado para siempre.

**Bastion v2**: JWT con expiry. El worker debe re-registrarse cuando expira:

```rust
struct SessionClaims {
    sandbox_id: String,
    capabilities_hash: String,   // Detecta capability downgrade attacks
    issued_at: u64,
    expires_at: u64,             // Default: 1 hora
}
```

### 2.4 Rate Limiting (nuevo)

Jenkins no tiene rate limiting. Un worker malicioso puede floodear el controller.

**Bastion v2**: Token bucket per sandbox:

```rust
struct RateLimiter {
    max_commands_per_second: f64,  // Default: 10
    max_bytes_per_second: f64,     // Default: 10MB/s
    burst_size: u32,               // Default: 20 commands
}
```

### 2.5 Path Traversal Protection (nuevo)

```rust
/// Validate that a path doesn't escape the sandbox working directory
fn validate_path(base_dir: &Path, requested: &str) -> Result<PathBuf> {
    let resolved = base_dir.join(requested).canonicalize()?;
    ensure!(
        resolved.starts_with(base_dir),
        "Path traversal detected: {} escapes {}",
        resolved.display(),
        base_dir.display()
    );
    Ok(resolved)
}
```

### 2.6 Audit Trail (nuevo)

Jenkins tiene logging básico. Bastion v2 loguea cada comando:

```rust
#[derive(Serialize)]
struct AuditEntry {
    timestamp: String,         // ISO 8601
    sandbox_id: String,
    command_id: String,
    command_type: String,      // "run_command", "read_file", etc.
    command_detail: String,    // El comando ejecutado o path
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    error: Option<String>,
}
```

---

## 3. ESTABILIDAD — State Machine + Auto-Healing

### 3.1 Worker Lifecycle State Machine

Jenkins trata al agent como "connected" o "disconnected". No hay estados intermedios.

**Bastion v2**: State machine completa:

```
                    ┌──────────────────────────────────┐
                    │                                  │
                    ▼                                  │
  [Created] ──► Registering ──► Ready ──► Busy ──► Draining
                                       │         │         │
                                       │         │         ▼
                                       │         │    Disconnected
                                       │         │         │
                                       └─────────┘         │
                                        (reconnect)        │
                                                           │
                                                    (shutdown)
                                                           │
                                                           ▼
                                                      Terminated
```

| Estado | Descripción | Transiciones |
|--------|-------------|-------------|
| **Created** | Worker binario lanzado, no conectado aún | → Registering |
| **Registering** | TCP conectado, esperando registro | → Ready, → Disconnected |
| **Ready** | Registrado, esperando comandos | → Busy, → Draining |
| **Busy** | Ejecutando 1+ comandos | → Ready, → Draining |
| **Draining** | Shutdown solicitado, esperando comandos en vuelo | → Disconnected |
| **Disconnected** | Conexión perdida o shutdown completo | → Registering (reconnect) |
| **Terminated** | Proceso worker terminado | (final) |

### 3.2 Reconnection con Backoff + Jitter

**Jenkins**: El agent debe ser reiniciado manualmente si pierde conexión.

**Bastion v2**: Reconnection automática con jitter para evitar thundering herd:

```rust
fn next_backoff(attempt: u32, base: Duration, max: Duration) -> Duration {
    let exp = base * 2u32.saturating_pow(attempt);
    let jitter = rand::thread_rng().gen_range(0..base.as_millis() as u64);
    let total = exp + Duration::from_millis(jitter);
    total.min(max)
}
// Attempt 1: 1s ± 1s     = 0-2s
// Attempt 2: 2s ± 1s     = 1-3s
// Attempt 3: 4s ± 1s     = 3-5s
// Attempt 4: 8s ± 1s     = 7-9s
// Attempt 5: 16s ± 1s    = 15-17s
// ...
// Max:     60s ± 1s
```

### 3.3 Circuit Breaker

**Jenkins**: Si un agent falla, sigue recibiendo trabajo hasta que un humano lo notice.

**Bastion v2**: Circuit breaker automático:

```rust
struct CircuitBreaker {
    failure_threshold: u32,     // Default: 3
    recovery_timeout: Duration, // Default: 30s
    state: CircuitState,        // Closed, Open, HalfOpen
}

enum CircuitState {
    Closed,                      // Normal: commands flow
    Open { opened_at: Instant }, // Failed: reject commands, health-check only
    HalfOpen,                    // Testing: send 1 command to verify
}
```

### 3.4 Graceful Shutdown

**Jenkins**: `ShutdownRequest` no existe. El agent muere y los jobs fallan.

**Bastion v2**: Drain protocol:

```protobuf
message ShutdownRequest {
  enum GraceLevel {
    GRACEFUL = 0;    // Espera comandos en vuelo (default)
    DRAINING = 1;    // No acepta nuevos comandos
    FORCEFUL = 2;    // Kill inmediato
  }
  GraceLevel grace_level = 1;
  int64 timeout_ms = 2;        // Timeout para graceful drain
  string reason = 3;
}

message ShutdownAck {
  int32 pending_commands = 1;  // Cuántos comandos quedan en vuelo
  bool will_drain = 2;         // El worker va a esperar o va a force-kill
}
```

### 3.5 Watchdog Timer

**Jenkins**: `pingIntervalSec` (0 = disabled por defecto desde 2.60).

**Bastion v2**: Heartbeat bidireccional obligatorio:

```
Worker ─── PingRequest(timestamp=T1) ──────► Gateway
Worker ◄── PongResponse(ping_ts=T1, ts=T2) ── Gateway

Si no hay Pong en 3x heartbeat_interval:
  → Worker se considera disconnected
  → Circuit breaker se abre
  → Pending commands se cancelan con error "Worker disconnected"
```

---

## 4. EFICIENCIA — Streaming + Compression + Zero-Copy

### 4.1 Streaming de Archivos con Chunking

**Jenkins**: Transfiere archivos enteros en memoria. Archivos grandes = OOM.

**Bastion v2**: Chunked streaming:

```protobuf
message ReadFileRequest {
  string path = 1;
  int64 offset = 2;            // NEW: Para resume/seek
  int64 length = 3;            // NEW: -1 = todo, o bytes específicos
}

message FileChunk {             // NEW: Reemplaza FileContent
  string command_id = 1;
  bytes data = 2;
  int64 offset = 3;            // Offset dentro del archivo
  bool is_last = 4;            // true = último chunk
  int32 chunk_index = 5;
}
```

**Worker envía archivos en chunks de 4MB**:
```rust
async fn stream_file(path: &Path, tx: &mpsc::Sender<WorkerMessage>) -> Result<()> {
    let mut file = File::open(path).await?;
    let mut offset = 0i64;
    let mut chunk_index = 0u32;
    let mut buf = vec![0u8; 4 * 1024 * 1024]; // 4MB chunks

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 { break; }

        tx.send(WorkerMessage {
            command_id: cmd_id.clone(),
            payload: Some(Payload::FileChunk(FileChunk {
                data: buf[..n].to_vec().into(),
                offset,
                is_last: n < buf.len(),
                chunk_index,
                ..
            })),
        }).await?;

        offset += n as i64;
        chunk_index += 1;
    }
    Ok(())
}
```

### 4.2 Environment Reuse

**Jenkins**: Cada build executa un nuevo proceso.

**Bastion v2**: Opcionalmente mantiene procesos daemon entre comandos:

```protobuf
message RunCommandRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;
  map<string, string> env = 4;
  int64 timeout_ms = 5;
  bool persistent = 6;         // NEW: Si true, el proceso sobrevive entre comandos
  string session_id = 7;       // NEW: Para comandos en la misma sesión
}

message RunCommandResponse {
  string command_id = 1;
  string session_id = 2;       // NEW: Si persistent=true, usar en próximos comandos
}
```

### 4.3 gRPC Connection Pooling

**Jenkins**: Una conexión TCP por agent.

**Bastion v2**: gRPC multiplexa múltiples comandos sobre una sola conexión HTTP/2:

```
              ┌────────────────────────────┐
              │    Single HTTP/2 Connection │
              │    (gRPC multiplexed)       │
              │                            │
              │    Stream 1: cmd_abc123    │
              │    Stream 2: cmd_def456    │──► Worker process
              │    Stream 3: heartbeat     │    (4 concurrent commands)
              │    Stream 4: file_transfer │
              └────────────────────────────┘
```

gRPC/HTTP/2 ya proporciona:
- **Multiplexación**: Múltiples streams sobre una conexión TCP
- **Flow control**: Backpressure automático
- **Header compression**: HPACK
- **Keepalive**: HTTP/2 PING frames

### 4.4 Zero-Copy donde sea posible

```rust
// Usar bytes::Bytes para zero-copy de chunks grandes
use prost::bytes::Bytes;

// En vez de Vec<u8> (copia), usar Bytes (reference-counted slice)
message StdoutChunk {
  string command_id = 1;
  bytes data = 2;          // prost::bytes::Bytes — zero-copy
}
```

---

## 5. RENDIMIENTO — Static Binary + Async + Observability

### 5.1 Static Binary MUSL (sin runtime)

| Métrica | Jenkins Agent | Bastion Worker |
|---------|--------------|---------------|
| Tamaño binario | 170KB JAR + JVM ~200MB | ~5-15MB static binary |
| Tiempo arranque | 2-5s (JVM + JIT) | <50ms (native) |
| Memoria idle | ~100-200MB | ~2-5MB |
| Dependencia | Java 11+ instalado | Ninguna |

```toml
# .cargo/config.toml para MUSL
[target.x86_64-unknown-linux-musl]
linker = "x86_64-linux-musl-gcc"

# Build
# cargo build --release --target x86_64-unknown-linux-musl -p bastion-worker
```

### 5.2 Tokio Async Runtime

**Jenkins**: Thread pool bloqueante (NIO mejoró pero sigue siendo Java).

**Bastion v2**: Tokio work-stealing runtime:

```rust
#[tokio::main(worker_threads = 2)]  // 2 threads son suficientes para un worker
async fn main() -> Result<()> {
    // Worker lightweight: 2 threads manejan:
    // - gRPC stream receive/send
    // - Command execution (spawn_blocking para CPU-intensive)
    // - Heartbeat timer
    // - File I/O (tokio::fs)
}
```

### 5.3 Concurrent Command Execution

**Jenkins**: Un executor = un thread = un comando concurrente.

**Bastion v2**: Worker con `max_concurrent_commands` configurable:

```rust
struct CommandExecutor {
    semaphore: Arc<Semaphore>,    // Limita concurrencia
    working_dir: PathBuf,
    timeout: Duration,
}

impl CommandExecutor {
    async fn execute(&self, cmd: GatewayCommand) -> Vec<WorkerMessage> {
        let permit = self.semaphore.acquire().await?;
        let result = tokio::time::timeout(
            self.timeout,
            self.run_process(cmd),
        ).await;
        drop(permit);
        // ...
    }
}
```

### 5.4 Health Metrics (nuevo)

**Jenkins**: OpenTelemetry soporte añadido en 2021 (remoting-monitoring-otel).

**Bastion v2**: Health metrics nativos desde el diseño:

```protobuf
message HealthReport {
  double cpu_usage_percent = 1;
  int64 memory_used_bytes = 2;
  int64 memory_total_bytes = 3;
  int64 disk_free_bytes = 4;
  int64 uptime_seconds = 5;
  int32 active_commands = 6;
  int32 pending_commands = 7;
}
```

El worker envía `HealthReport` periódicamente (cada heartbeat):

```
Heartbeat cada 10s:
  Worker ─── PingRequest(ts=T, health=HealthReport) ──► Gateway
  Gateway ◄── PongResponse(ts=T) ──────────────────── Worker
```

### 5.5 OpenTelemetry Tracing

```rust
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[instrument(name = "run_command", skip(self, cmd))]
async fn execute(&self, cmd: RunCommandRequest) -> Vec<WorkerMessage> {
    let span = Span::current();
    span.set_attribute("command", cmd.command.clone());
    span.set_attribute("sandbox_id", self.sandbox_id.clone());

    let start = Instant::now();
    // ... execute ...
    span.set_attribute("exit_code", exit_code);
    span.set_attribute("duration_ms", start.elapsed().as_millis() as i64);
}
```

---

## 6. Proto v2 Completo — Con Todas las Mejoras

```protobuf
syntax = "proto3";
package bastion.worker.v2;

// ═══════════════════════════════════════════════════════════════
// Bastion Worker Protocol v2
// Inspired by Jenkins Remoting, improved with modern Rust/gRPC
// ═══════════════════════════════════════════════════════════════

service WorkerRegistry {
  // Step 1: Register with challenge-response auth
  rpc Register (RegisterRequest) returns (RegisterResponse);

  // Step 1b: Respond to challenge (if Status = CHALLENGE)
  rpc ChallengeResponse (ChallengeProof) returns (RegisterResponse);

  // Step 2: Bidirectional command stream (multiplexed over HTTP/2)
  rpc CommandStream (stream WorkerMessage) returns (stream GatewayCommand);
}

// ═══ Registration & Auth ═══

message ProtocolVersion {
  int32 major = 1;
  int32 minor = 2;
  int32 patch = 3;
}

message WorkerCapabilities {
  repeated string supported_operations = 1;
  int32 max_concurrent_commands = 2;       // Default: 4
  int64 max_output_bytes = 3;              // Default: 10MB
  int64 max_file_size_bytes = 4;           // Default: 100MB
  bool supports_streaming = 5;
  bool supports_compression = 6;
  string os = 7;                           // "linux", "darwin", "windows"
  string arch = 8;                         // "x86_64", "aarch64"
}

message RegisterRequest {
  string sandbox_id = 1;
  ProtocolVersion protocol_version = 2;
  WorkerCapabilities capabilities = 3;
  bytes worker_nonce = 4;                  // 32 bytes random
  string worker_version = 5;
}

message RegisterResponse {
  enum Status {
    ACCEPTED = 0;        // Auth OK, here's your token
    CHALLENGE = 1;       // Prove you have the secret
    REJECTED = 2;        // Go away
    VERSION_MISMATCH = 3;// Protocol version incompatible
  }
  Status status = 1;
  bytes gateway_nonce = 2;                 // 32 bytes (if CHALLENGE)
  string session_token = 3;                // JWT (if ACCEPTED)
  ProtocolVersion negotiated_version = 4;  // Agreed version
  int64 heartbeat_interval_ms = 5;         // Default: 10000
  int64 command_timeout_ms = 6;            // Default: 30000
  int64 session_expiry_ms = 7;             // Default: 3600000 (1h)
  string gateway_version = 8;
}

message ChallengeProof {
  bytes proof = 1;                         // HMAC-SHA256(secret, worker_nonce || gateway_nonce)
}

// ═══ Gateway → Worker (Commands) ═══

message GatewayCommand {
  string command_id = 1;                   // UUID
  string session_token = 2;                // Must match registered token
  oneof payload {
    RunCommandRequest run = 10;
    ReadFileRequest read = 11;
    WriteFileRequest write = 12;
    ListFilesRequest list = 13;
    PingRequest ping = 14;
    ShutdownRequest shutdown = 15;
    CancelRequest cancel = 16;
  }
}

message RunCommandRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;
  map<string, string> env = 4;
  int64 timeout_ms = 5;
  bool persistent = 6;                     // Process survives between commands
  string session_id = 7;                   // Group commands in same process
}

message ReadFileRequest {
  string path = 1;
  int64 offset = 2;                        // For seek/resume
  int64 length = 3;                        // -1 = entire file
}

message WriteFileRequest {
  string path = 1;
  int32 mode = 2;                          // Unix permissions
  int64 total_size = 3;
  int32 chunk_index = 4;
  int32 total_chunks = 5;                  // -1 = streaming
  bytes content = 6;                       // Max 4MB per chunk
}

message ListFilesRequest {
  string directory = 1;
  bool recursive = 2;
  int32 max_depth = 3;                     // Limit recursion depth
}

message PingRequest {
  int64 timestamp = 1;
}

message ShutdownRequest {
  enum GraceLevel {
    GRACEFUL = 0;     // Wait for in-flight commands
    DRAINING = 1;     // No new commands, wait for current
    FORCEFUL = 2;     // Kill immediately
  }
  GraceLevel grace_level = 1;
  int64 timeout_ms = 2;
  string reason = 3;
}

message CancelRequest {
  string target_command_id = 1;            // Cancel a running command
  string reason = 2;
}

// ═══ Worker → Gateway (Responses) ═══

message WorkerMessage {
  string command_id = 1;
  oneof payload {
    ReadySignal ready = 10;
    CommandAck ack = 11;
    StdoutChunk stdout = 12;
    StderrChunk stderr = 13;
    ExitResult exit = 14;
    FileChunk file_chunk = 15;             // Chunked file content
    FileList file_list = 16;
    ErrorResult error = 17;
    PongResponse pong = 18;
    ShutdownAck shutdown_ack = 19;
    HealthReport health = 20;
    CancelAck cancel_ack = 21;
  }
}

message ReadySignal {
  string session_token = 1;
  string working_dir = 2;
}

message CommandAck {
  enum State {
    RECEIVED = 0;
    EXECUTING = 1;
    QUEUED = 2;
  }
  State state = 1;
}

message StdoutChunk {
  bytes data = 1;
  int64 sequence = 2;
}

message StderrChunk {
  bytes data = 1;
  int64 sequence = 2;
}

message ExitResult {
  int32 exit_code = 1;
  int64 duration_ms = 2;
  bool timed_out = 3;
  string signal = 4;                       // "SIGKILL", "SIGTERM", etc.
}

message FileChunk {
  bytes content = 1;
  int64 offset = 2;
  bool is_last = 3;
  int32 chunk_index = 4;
  int32 total_chunks = 5;
}

message FileList {
  repeated FileEntry entries = 1;
}

message FileEntry {
  string path = 1;
  bool is_directory = 2;
  int64 size_bytes = 3;
  string permissions = 4;
  int64 modified_epoch_ms = 5;
  string file_type = 6;                    // "file", "dir", "symlink", "other"
}

message ErrorResult {
  string error = 1;
  string error_kind = 2;                   // "timeout", "not_found", "permission", "internal", "cancelled"
  int32 errno = 3;                         // System errno if available
}

message PongResponse {
  int64 ping_timestamp = 1;
  int64 worker_timestamp = 2;
  HealthReport health = 3;                 // Piggyback health on pong
}

message ShutdownAck {
  int32 pending_commands = 1;
  bool will_drain = 2;
}

message CancelAck {
  bool cancelled = 1;
  string error = 2;                        // If cancellation failed
}

message HealthReport {
  double cpu_usage_percent = 1;
  int64 memory_used_bytes = 2;
  int64 memory_total_bytes = 3;
  int64 disk_free_bytes = 4;
  int64 uptime_seconds = 5;
  int32 active_commands = 6;
  int32 pending_commands = 7;
  double load_average_1m = 8;
}
```

---

## 7. Resumen de Mejoras — Jenkins vs Bastion v2

| Dimensión | Jenkins JNLP4 | Bastion v2 | Mejora |
|-----------|--------------|-----------|--------|
| **Wire format** | Java Serialization | Protobuf v2 | Schema-safe, 3-10x más pequeño |
| **Auth** | Secret en texto | Challenge-response HMAC | Secret nunca transita |
| **Session** | Sin sesión | JWT con expiry (1h) | Auto-expiración |
| **TLS** | Opcional (JNLP3) | Obligatorio (rustls) | No hay modo inseguro |
| **Commands** | RPC genérico | Whitelist explícita | No arbitrary execution |
| **Path safety** | Sin validación | Path traversal protection | Sandbox confinement |
| **Rate limit** | Ninguno | Token bucket per sandbox | DoS protection |
| **Reconnect** | Manual | Exponential backoff + jitter | Auto-healing |
| **Circuit breaker** | Ninguno | 3 failures → Open | Failure isolation |
| **Shutdown** | Kill sin aviso | Graceful drain protocol | No lost commands |
| **File transfer** | Full memory | Chunked 4MB streaming | No OOM |
| **Compression** | Ninguna | gRPC gzip | 60-80% bandwidth savings |
| **Flow control** | Ninguno | HTTP/2 + max_output_bytes | Backpressure |
| **Concurrency** | 1 executor = 1 thread | Semaphore-based (configurable) | Parallel execution |
| **Health** | Ping interval | Rich HealthReport | Observability |
| **Audit** | Logging básico | Structured audit trail | Compliance-ready |
| **Binary size** | 170KB + JVM 200MB | ~5-15MB static | 10-20x más ligero |
| **Startup** | 2-5s (JVM) | <50ms (native) | 50-100x más rápido |
| **Memory** | 100-200MB idle | 2-5MB idle | 30-50x menos |
| **Versioning** | Monolítico incompatible | SemVer negociado | Backward compatible |

---

## 8. Plan de Implementación por Fases

### Phase 1: Foundation (1-2 días)
- [ ] Proto v2 completo (con auth, capabilities, health)
- [ ] Worker como gRPC CLIENT
- [ ] Gateway Registry gRPC SERVER
- [ ] Register con challenge-response
- [ ] CommandStream bidi básico
- [ ] Bind-mount en Podman

### Phase 2: Command Routing (1 día)
- [ ] PendingCommands HashMap
- [ ] Response aggregation (stdout chunks + stderr + exit)
- [ ] RunCommand a través de CommandStream
- [ ] FileOps a través de CommandStream
- [ ] CommandAck (RECEIVED → EXECUTING)

### Phase 3: Reliability (1 día)
- [ ] Reconnection con exponential backoff + jitter
- [ ] Circuit breaker
- [ ] Heartbeat (Ping/Pong con health)
- [ ] Graceful shutdown (Drain protocol)
- [ ] CancelRequest para comandos en vuelo

### Phase 4: Security (1 día)
- [ ] TLS con rustls (mandatory)
- [ ] Secret por sandbox (generado en sandbox_create)
- [ ] JWT session tokens
- [ ] Rate limiting
- [ ] Path traversal protection
- [ ] Audit trail

### Phase 5: Performance (1 día)
- [ ] gRPC compression (gzip)
- [ ] File chunking (4MB)
- [ ] Concurrent command execution
- [ ] HealthReport + OpenTelemetry
- [ ] Static binary MUSL build
