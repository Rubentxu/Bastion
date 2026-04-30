# Análisis Profundo: Jenkins Remoting / JNLP — Lecciones para el Rediseño de Bastion

> **Autor**: SDD Explore Phase  
> **Fecha**: 2026-04-30  
> **Estado**: Investigación completa  
> **Proyecto**: Bastion — MCP Gateway para orquestación de sandboxes AI

---

## 1. Resumen Ejecutivo

1. **El modelo "inbound agent" de Jenkins es exactamente lo que Bastion necesita**: El worker inicia la conexión hacia el gateway (outbound desde el contenedor), eliminando la necesidad de exponer puertos o hacer port-mapping complejo. Esto resuelve el problema #1 de Bastion.

2. **La evolución de protocolos de Jenkins (JNLP1→4→WebSocket) demuestra que gRPC/HTTP2 es la elección correcta**: Jenkins evolucionó de Java-serialization crudo a TLS personalizado (JNLP4) a WebSocket. Nosotros empezamos con gRPC sobre HTTP/2, que ya resuelve framing, multiplexación y encryption de forma nativa.

3. **El `agent.jar` (170KB) se distribuye por HTTP download desde el controller**: Bastion actualmente usa tar+upload; la lección es usar bind-mount del binary en el contenedor, eliminando la latencia de upload y los problemas de versioning.

4. **La negociación de capacidades (Capability negotiation) es crítica para multi-provider**: Jenkins lo hace en el handshake inicial; Bastion necesita un mecanismo equivalente para distinguir Podman vs Firecracker vs gVisor vs K8s.

5. **El modelo de seguridad Agent→Controller de Jenkins nos advierte sobre la dirección de confianza**: Jenkins asume que el agente es _untrusted_ y filtra lo que puede hacer hacia el controller. En Bastion, el gateway debe ser paranoico sobre lo que el worker puede solicitar.

---

## 2. Arquitectura Jenkins Remoting

### 2.1 Modelo de Conexión

#### Conexión Inicial (Inbound Agent / antiguo "JNLP")

El flujo de conexión de un inbound agent en Jenkins es un proceso de dos pasos:

```
┌──────────┐                                    ┌──────────────┐
│  Agent   │                                    │  Controller  │
│(agent.jar│                                    │  (Jenkins)   │
└────┬─────┘                                    └──────┬───────┘
     │   1. HTTP GET /jnlpJars/agent.jar              │
     │ ──────────────────────────────────────────────►│
     │   ← agent.jar (170KB download)                 │
     │◄────────────────────────────────────────────── │
     │                                                 │
     │   2. HTTP GET slave-agent.jnlp                  │
     │      (o parámetros -url -secret -name)          │
     │ ──────────────────────────────────────────────►│
     │   ← Connection metadata (port, protocols)      │
     │◄────────────────────────────────────────────── │
     │                                                 │
     │   3. TCP connect to controller:PORT             │
     │      (o WebSocket wss://host/wsagents/)         │
     │ ──────────────────────────────────────────────►│
     │   TLS handshake (JNLP4)                        │
     │   Secret authentication                         │
     │   Capability negotiation                        │
     │   ═══ CHANNEL ESTABLISHED ═══                   │
```

**Roles de los componentes:**

| Componente | Rol | Tamaño |
|---|---|---|
| `agent.jar` | Ejecutable standalone que contiene la librería Remoting + bootstrap code | ~170KB |
| `remoting.jar` | La librería subyacente (misma codebase, nombre diferente en Maven) | Mismo JAR |
| `slave-agent.jnlp` | Archivo JNLP con metadata de conexión (obsoleto, reemplazado por CLI args) | XML |

**Nota importante**: "agent.jar" y "remoting.jar" son el mismo JAR. En Jenkins, `agent.jar` = `remoting.jar` renombrado. Se descarga desde `${JENKINS_URL}/jnlpJars/agent.jar`.

#### Autenticación del Agente

El agente se autentica mediante un **secret key** (HMAC del nombre del agente + secreto del controller):

```bash
java -jar agent.jar \
  -url https://jenkins.example.com \
  -secret d6a84df1fc4f45ddc9c6ab34b08f13391983ff... \
  -name buildNode1 \
  -workDir /home/jenkins/agent
```

- El secret es **determinístico** para un nombre dado en un controller dado
- Si el secret se compromete, NO se debe reusar el nombre del agente
- En JNLP4-connect, se usa TLS + el secret
- En WebSocket, la encryption la provee HTTPS (no hay capa extra)

#### Reconexión

Jenkins NO tiene reconexión automática sofisticada en la librería Remoting:

- El proceso del agente simplemente **termina** cuando pierde la conexión
- Es responsabilidad del administrador reiniciarlo (cron, systemd, Windows Service, Docker restart policy)
- El parámetro `pingIntervalSec` (default: 0 = disabled) controla health checks
- El parámetro `pingTimeoutSec` (default: 240s) determina cuándo se declara "muerto"

### 2.2 Arquitectura del Channel

#### ¿Qué es un "Channel"?

Un `Channel` en Jenkins Remoting es el abstraction central que conecta dos JVMs. Es esencialmente:

```
┌─────────────── Channel ─────────────────┐
│                                          │
│  ┌──────────┐     ┌──────────────────┐  │
│  │ Command  │────►│  Export Table    │  │
│  │ Queue    │     │  (object refs)   │  │
│  └──────────┘     └──────────────────┘  │
│                                          │
│  ┌──────────────────────────────────────┐│
│  │        Virtual Streams               ││
│  │  Stream{1} Stream{2} ... Stream{N}  ││
│  └──────────────────────────────────────┘│
│                                          │
│  ┌──────────────────────────────────────┐│
│  │        Transport Layer               ││
│  │  TCP / WebSocket / (Kafka)           ││
│  └──────────────────────────────────────┘│
└──────────────────────────────────────────┘
```

#### Multiplexación de Streams Virtuales

La multiplexación funciona sobre una conexión TCP con **length-prefixed framing**:

1. Cada stream virtual tiene un ID numérico único
2. Los frames se prefijan con: `[stream_id][length][payload]`
3. Un stream especial (ID=0) se usa para el control channel (Commands/RPCs)
4. Los streams de datos (stdout, stderr, file transfer) usan IDs > 0

#### Patrón "Command" (RPCs)

Jenkins implementa RPCs via el patrón `Callable`:

```java
// El controller envía un Callable al agente:
channel.call(new MyCallable());

// El agente lo ejecuta y devuelve el resultado serializado
```

- Cada `Callable` tiene un ID único
- La respuesta se enruta por ID al caller original
- Soporta invocación síncrona (`call()`) y asíncrona (`callAsync()`)
- El resultado se serializa con Java Object Serialization (o un ClassFilter restringido)

#### Comunicación Bidireccional

- **El controller puede llamar al agente** (enviar Callable para ejecutar)
- **El agente puede llamar al controller** (via `Channel` obtenido del lado del agente)
- Esto es simétrico: ambos lados tienen un `Channel` object
- El agente usa esto para cargar clases dinámicamente del controller (RemoteClassLoader)

#### Negociación de Capacidades (Capability)

La negociación ocurre durante el handshake del protocolo:

```
Agent → Controller:  [Protocol Header] [Agent Capabilities bitmap]
Controller → Agent:  [Response Header] [Controller Capabilities bitmap]
```

Capabilities incluyen bits para:
- Soporte para multi-release JAR
- Soporte para fragmentación de grandes payloads
- Versión del protocolo
- Funcionalidades de seguridad

### 2.3 Stack de Protocolos

```
┌─────────────────────────────────┐
│     Application Layer           │  ← Commands, Callables, RPCs
├─────────────────────────────────┤
│     Filter Layers               │  ← Optional: compression, logging
├─────────────────────────────────┤
│     Protocol Layer              │  ← JNLP4-connect / WebSocket
├─────────────────────────────────┤
│     Network Layer               │  ← TCP / HTTP(S) / WebSocket
└─────────────────────────────────┘
```

Cada capa es un "filter" en una doubly-linked list, permitiendo composición flexible.

---

## 3. Protocolo JNLP — Análisis Detallado

### 3.1 Evolución de Protocolos

| Protocolo | Versión | Wire Format | Problema que Resolvió |
|---|---|---|---|
| JNLP1 | Remoting 1.x | Java Serialization + length-prefixed TCP | Protocolo original, sin encryption |
| JNLP2 | Remoting 2.x | Java Serialization + NIO | Performance (non-blocking I/O) |
| JNLP3-connect | Remoting 2.x | Java Serialization + NIO + custom framing | Bottleneck de threads (un thread por conexión) |
| JNLP4-connect | Remoting 3.0 | **TLS handshake** + Java Serialization | **Seguridad**: encryption antes de enviar secret |
| WebSocket | Remoting 4.0 | WebSocket binary frames sobre HTTP(S) | Reverse proxies, elimina puerto TCP dedicado |

### 3.2 JNLP4-connect (Protocolo Actual Recomendado)

El handshake JNLP4-connect:

```
Agent                              Controller
  │                                    │
  │──── TCP Connect ─────────────────►│
  │                                    │
  │──── ClientHello ──────────────────►│  (agent name, secret, capabilities)
  │                                    │
  │◄─── ServerHello ─────────────────│  (accepted, controller capabilities)
  │                                    │
  │◄═══ TLS Handshake ════════════════►│  (SSLEngine upgrade)
  │                                    │
  │◄═══ Channel establido ════════════►│  (multiplexed stream)
```

**Wire format**: Después del TLS handshake, los mensajes son Java Serialization con length-prefixed framing:
- 4 bytes: longitud del mensaje
- N bytes: objeto Java serializado

### 3.3 WebSocket (Remoting 4.0+, JEP-222)

El protocolo WebSocket soluciona el problema de reverse proxies:

```
Agent                              Controller
  │                                    │
  │─── HTTP GET /wsagents/ ──────────►│  (upgrade to WebSocket)
  │    Header: Authorization: Bearer   │
  │◄─── 101 Switching Protocols ──────│
  │                                    │
  │◄═══ WebSocket frames ═════════════►│  (binary frames, bi-directional)
```

Ventajas sobre TCP:
- No necesita puerto dedicado (usa el mismo puerto HTTP/443)
- WebSocket provee framing nativo (no se necesita length-prefix)
- Encryption via HTTPS (no se necesita TLS personalizado)
- Compatible con L7 load balancers y reverse proxies

### 3.4 Manejo de Grandes Payloads

- Jenkins usa **fragmentación** de payloads grandes
- Los `Callable` results se serializan y se envían como un stream de chunks
- El `ClassFilter` restringe qué clases pueden deserializarse (seguridad anti-deserialization)
- File transfers usan streams virtuales dedicados con backpressure implícito

---

## 4. Modelo de Ejecución

### 4.1 Despacho de Comandos

El controller envía un `Callable` serializado al agente:

```
Controller                           Agent
    │                                  │
    │─── Callable(A) ─────────────────►│  (serializado sobre stream ID=0)
    │                                  │─── Ejecuta A.call()
    │◄── Result(A) ───────────────────│  (serializado de vuelta)
    │                                  │
```

### 4.2 Streaming de stdout/stderr

- Los procesos lanzados en el agente capturan stdout/stderr
- Estos se envían de vuelta al controller via **streams virtuales** (no el command channel)
- El controller los expone como `InputStream` normales
- Hay un `FlightRecorderInputStream` que guarda un ring buffer de 1MB para debugging

### 4.3 Directorio de Trabajo

Desde Remoting 3.8, el working directory se configura con `-workDir`:

```
${WORKDIR}/
  remoting/            ← internal dir
    jarCache/          ← JAR cache for RemoteClassLoader
    logs/              ← Persistent logs
```

Antes de 3.8: no había concepto de working directory, todo se almacenaba en `${user.home}/.jenkins`.

### 4.4 Variables de Entorno

- Se pasan como `KEY=VALUE` en la creación del contenedor
- No hay un mecanismo de envío post-creación via Remoting
- Para cambios en runtime, se usan `Callable` que ejecutan `export`

### 4.5 Comandos Concurrentes

- Jenkins soporta múltiples executors por agente (threads)
- Cada executor procesa un `Callable` de forma independiente
- Los Callables se serializan sobre el mismo channel (multiplexed)
- No hay locks a nivel de protocolo — la concurrencia la maneja la aplicación

---

## 5. Seguridad

### 5.1 Prevención de Ejecución Arbitraria

Jenkins ha sufrido **múltiples vulnerabilidades críticas** por Remoting:

| Año | Advisory | Problema | Mitigación |
|---|---|---|---|
| 2015 | SECURITY-218 | Deserialization de clases arbitrarias | **ClassFilter**: whitelist/blacklist de clases serializables |
| 2016 | SECURITY-286 | Agent→Controller escalation | **Agent→Controller Access Control**: filtra callbacks del agente |
| 2017 | SECURITY-522 | Remoting CLI mode permitía RCE | **Remoto CLI eliminado** completamente |
| 2019 | JEP-235 | Agent→Controller bypass | **Siempre habilitado** desde Jenkins 2.326 |

### 5.2 Capa de Seguridad Remoting

El modelo de seguridad tiene tres capas:

```
┌─────────────────────────────────┐
│  1. Transport Encryption         │  ← TLS (JNLP4) o HTTPS (WebSocket)
├─────────────────────────────────┤
│  2. Authentication              │  ← Secret HMAC (agent name + controller secret)
├─────────────────────────────────┤
│  3. Authorization (Agent→Ctrl)  │  ← ClassFilter + FilePath whitelist
└─────────────────────────────────┘
```

**Agent → Controller Access Control** (crítico):

Desde Jenkins 2.326, siempre habilitado:
- Filtra qué clases pueden deserializarse (`ClassFilter`)
- Restringe acceso al filesystem del controller desde el agente
- Restringe qué commands puede ejecutar el agente en el controller
- Los administradores pueden definir exemptions específicas

### 5.3 Gestión de Credenciales

- El **secret** del agente es un HMAC generado por el controller
- Se almacena en la configuración del nodo (no es un JWT, no tiene expiry)
- Para renegar acceso: eliminar el nodo y recrear con nombre diferente
- Las credenciales de build (SSH keys, tokens) se manejan via el Credentials Plugin, no via Remoting directamente

### 5.4 Lecciones de Seguridad para Bastion

1. **Nunca confiar en el worker** — el contenedor puede ser comprometido
2. **Whitelist de comandos** — solo permitir operaciones definidas en el proto
3. **No enviar credenciales al worker** — mantenerlas en el gateway
4. **Encryption por defecto** — TLS o equivalent
5. **ClassFilter equivalente** — restringir qué tipos de mensajes acepta el gateway

---

## 6. Distribución del Binario

### 6.1 Métodos de Distribución de agent.jar

| Método | Flujo | Ventajas | Desventajas |
|---|---|---|---|
| **HTTP Download** | `wget $JENKINS_URL/jnlpJars/agent.jar` | Simple, siempre versión correcta | Requiere red HTTP |
| **Docker Image** | `jenkins/inbound-agent` (pre-bundled) | Zero-config, reproducible | Version lock a imagen |
| **Pre-instalado** | Copia manual al host | Offline-capable | Version drift |
| **JNLP URL** | `javaws slave-agent.jnlp` | Auto-download | Java Web Start eliminado en Java 11+ |

### 6.2 Version Compatibility

- Jenkins siempre sirve la versión **correcta** de `agent.jar` desde el endpoint HTTP
- La URL `${JENKINS_URL}/jnlpJars/agent.jar` devuelve la versión que matchea el controller
- No hay negociación de versión — si hay mismatch, el agente falla al conectar
- El agent.jar contiene un check de versión mínima en el handshake

### 6.3 Relación agent.jar vs remoting.jar

Son **el mismo JAR**, solo renombrado:
- `remoting.jar` = nombre Maven del artefacto (`org.jenkins-ci.main:remoting`)
- `agent.jar` = nombre de distribución para end-users
- En Jenkins, el controller sirve `remoting.jar` renombrado como `agent.jar` en `/jnlpJars/agent.jar`

---

## 7. Fiabilidad y Manejo de Errores

### 7.1 Desconexiones

El proceso de shutdown de un Channel:

**Ordenado (CloseCommand):**
```
Side A                              Side B
  │──── CloseCommand (FIN) ────────►│
  │◄─── CloseCommand (FIN-ACK) ─────│
  │                                  │
  ╳  Channel closed cleanly  ╳
```

**Desordenado (EOF/timeout):**
```
Side A                              Side B
  │         (connection drops)       │
  │◄─── EOF detectado ─────────────│
  │                                  │
  │  Channel.terminate()             │
  │  → Marca ambos direction dead   │
  ╳  Channel terminated abruptly  ╳
```

### 7.2 Comandos en Vuelo (In-flight)

- **No hay mecanismo de recovery** para Callables en vuelo
- Si el channel se cae, los Callables pendientes lanzan `ChannelClosedException`
- La aplicación (Jenkins) es responsable de reintentar a nivel superior
- Los builds fallidos se marcan como `ABORTED` o se reintentan

### 7.3 Heartbeat/Keepalive

Configuración via system properties:

| Propiedad | Default | Descripción |
|---|---|---|
| `hudson.remoting.Launcher.pingIntervalSec` | 0 (disabled) | Segundos entre pings |
| `hudson.remoting.Launcher.pingTimeoutSec` | 240 | Timeout para declarar muerto |
| `hudson.remoting.Engine.socketTimeout` | 30 min | Socket read timeout |

Nota: El ping está **disabled por defecto** desde Remoting 2.60. El keepalive en WebSocket lo maneja el protocolo (ping/pong frames).

### 7.4 Shutdown Graceful

- El agente envía `CloseCommand` antes de desconectar
- El controller marca el nodo como `temporarilyOffline`
- Los builds en ejecución se interrumpen
- No hay drain mode nativo en la librería Remoting

---

## 8. Lecciones para Bastion

### 8.1 Qué Adoptar de Jenkins

| Feature Jenkins | Cómo Aplicar en Bastion |
|---|---|
| **Inbound connection model** | Worker como gRPC CLIENT, no SERVER |
| **Capability negotiation** | Handshake inicial con bitmap de capacidades |
| **Secret-based auth** | Token HMAC por sandbox_id + gateway secret |
| **Bidirectional streaming** | gRPC bidirectional stream (ya en nuestro proto) |
| **Virtual stream multiplexing** | gRPC HTTP/2 multiplexación nativa |
| **Working directory** | `-workDir` equivalente para logs y cache del worker |
| **ClassFilter / message filtering** | Whitelist estricta de message types |

### 8.2 Qué NO Adoptar ( Jenkins → Mejoras de Bastion)

| Problema Jenkins | Solución Bastion |
|---|---|
| **Java Serialization** (vulnerable, pesada) | **Protobuf** (schema-defined, compacto, seguro) |
| **Sin reconexión automática** | **Exponential backoff con jitter** en el worker |
| **Un thread por conexión** (JNLP3) | **Tokio async runtime** (millones de conexiones) |
| **Sin drain mode** | **Graceful shutdown** con timeout y command drain |
| **Secret estático** (no expira) | **JWT con TTL** para tokens de sandbox |
| **Sin streaming de files** | **Chunked file transfer** en proto |
| **Sin metricas nativas** | **OpenTelemetry** integrado desde el inicio |

### 8.3 Problemas Actuales de Bastion y Soluciones

| # | Problema | Estado Actual | Solución Propuesta |
|---|---|---|---|
| 1 | Worker como gRPC SERVER (inbound) | `worker/src/main.rs` lanza `Server::builder()` | Worker como gRPC CLIENT (outbound stream) |
| 2 | Binary via tar+upload | `PodmanProvider::inject_and_start_worker()` usa tar | Bind-mount del binary en `/usr/local/bin/bastion-worker` |
| 3 | Sin reconexión | Worker muere si pierde conexión | Retry loop con exponential backoff |
| 4 | Sin capability negotiation | `RegisterWorkerRequest` solo envía version | Capability bitmap en handshake |
| 5 | Sin multiplexación de comandos | `CommandStream` es un stream bidireccional simple | Per-command ID con response routing |
| 6 | Sin seguridad | No auth, no encryption | TLS + JWT + message whitelist |
| 7 | Sin heartbeat | No keepalive | gRPC keepalive ping interval |
| 8 | Port mapping complejo | `port_bindings: 50051/tcp → random host port` | Worker conecta al gateway — sin port mapping |

---

## 9. Propuesta de Diseño — Bastion Rediseñado

### 9.1 Nuevas Definiciones Proto3

```protobuf
syntax = "proto3";
package bastion.worker.v2;

// ═══════════════════════════════════════════════════════════════
// Gateway Service — expuesto por el Gateway, consumido por Workers
// El Worker (dentro del contenedor) ES el gRPC CLIENT
// El Gateway (en el host) ES el gRPC SERVER
// ═══════════════════════════════════════════════════════════════

service WorkerGateway {
  // ── Handshake ──────────────────────────────────────────────
  // El worker se registra UNA VEZ al iniciar.
  // Incluye capabilities, version, y token de autenticación.
  rpc RegisterWorker (RegisterWorkerRequest) returns (RegisterWorkerResponse);

  // ── Canal Bidireccional ────────────────────────────────────
  // El worker abre UN stream persistente después del registro.
  // El gateway envía comandos (RunCommand, FileOps, Ping).
  // El worker envía respuestas (stdout, stderr, exit, file content, Pong).
  // Cada mensaje tiene un command_id para correlación.
  rpc CommandChannel (stream WorkerUpstream) returns (stream GatewayDownstream);
}

// ── Registro ──────────────────────────────────────────────

message RegisterWorkerRequest {
  string sandbox_id = 1;
  string worker_version = 2;
  
  // Autenticación: JWT firmado por el gateway al crear el sandbox
  string auth_token = 3;
  
  // Capabilities bitmap (inspirado en Jenkins)
  WorkerCapabilities capabilities = 4;
}

message WorkerCapabilities {
  bool supports_streaming = 1;       // streaming command output
  bool supports_file_ops = 2;        // read/write/list files
  bool supports_snapshots = 3;       // pause/resume
  uint32 max_command_timeout_ms = 4; // maximum timeout per command
  uint32 max_file_size_bytes = 5;    // maximum single file size
  uint32 max_concurrent_commands = 6; // parallel command capacity
  string os = 7;                      // "linux", "windows", etc.
  string arch = 8;                    // "x86_64", "aarch64"
}

message RegisterWorkerResponse {
  bool accepted = 1;
  string gateway_version = 2;
  string session_id = 3;             // unique session for this connection
  uint64 heartbeat_interval_ms = 4;  // gateway requests this keepalive
  GatewayCapabilities capabilities = 5;
}

message GatewayCapabilities {
  bool supports_cancel = 1;          // can cancel in-flight commands
  bool supports_env_injection = 2;   // can inject env vars post-creation
  uint32 max_message_size_bytes = 3;  // maximum proto message size
}

// ── Canal Bidireccional ──────────────────────────────────

// Worker → Gateway
message WorkerUpstream {
  // Identificador de correlación para responses
  string command_id = 1;
  
  oneof payload {
    // ── Command Responses ──
    StdoutChunk stdout = 10;
    StderrChunk stderr = 11;
    ExitResult exit = 12;
    
    // ── File Responses ──
    FileContent file_content = 20;
    FileList file_list = 21;
    FileWriteAck file_write_ack = 22;
    
    // ── Errors ──
    ErrorResult error = 30;
    
    // ── Heartbeat ──
    PongMessage pong = 40;
    
    // ── Registration Ack ──
    ReadyMessage ready = 50;         // worker listo para recibir comandos
  }
}

// Gateway → Worker
message GatewayDownstream {
  // Identificador de correlación para comandos
  string command_id = 1;
  
  oneof payload {
    // ── Commands ──
    RunCommandRequest run_command = 10;
    
    // ── File Operations ──
    ReadFileRequest read_file = 20;
    WriteFileRequest write_file = 21;
    ListFilesRequest list_files = 22;
    
    // ── Lifecycle ──
    ShutdownRequest shutdown = 30;
    CancelRequest cancel = 31;
    
    // ── Heartbeat ──
    PingMessage ping = 40;
    
    // ── Environment ──
    SetEnvRequest set_env = 50;
  }
}

// ── Command Messages ──────────────────────────────────────

message RunCommandRequest {
  string command = 1;
  repeated string args = 2;
  string working_dir = 3;            // directorio de trabajo
  map<string, string> env = 4;       // env vars adicionales
  uint64 timeout_ms = 5;
}

message StdoutChunk {
  bytes data = 1;
  bool is_final = 2;                 // último chunk de stdout
}

message StderrChunk {
  bytes data = 1;
  bool is_final = 2;
}

message ExitResult {
  int32 exit_code = 1;
  uint64 duration_ms = 2;
  bool timed_out = 3;
  string signal = 4;                 // "SIGKILL", "SIGTERM", etc.
}

message CancelRequest {
  string target_command_id = 1;      // cancelar un comando en vuelo
  string reason = 2;
}

message ErrorResult {
  string error_code = 1;             // "TIMEOUT", "PERMISSION_DENIED", etc.
  string message = 2;
  bool retryable = 3;                // el gateway puede reintentar
}

// ── File Messages ──────────────────────────────────────────

message ReadFileRequest {
  string path = 1;
  uint64 offset_bytes = 2;           // para lectura parcial
  uint64 max_bytes = 3;              // limitar tamaño
}

message WriteFileRequest {
  string path = 1;
  bytes content = 2;
  uint32 mode = 3;                   // permisos Unix (e.g. 0o755)
  bool append = 4;                   // append vs overwrite
}

message FileContent {
  bytes content = 1;
  uint64 total_size = 2;             // tamaño total del archivo
  bool truncated = 3;                // se truncó por max_bytes
}

message FileList {
  repeated FileEntry entries = 1;
}

message FileEntry {
  string path = 1;
  bool is_directory = 2;
  uint64 size_bytes = 3;
  string permissions = 4;
  int64 modified_at_ms = 5;          // epoch millis
}

message FileWriteAck {
  uint64 bytes_written = 1;
}

// ── Lifecycle ─────────────────────────────────────────────

message ShutdownRequest {
  string reason = 1;
  bool force = 2;                    // SIGKILL vs graceful
  uint64 grace_period_ms = 3;        // tiempo para completar comandos
}

message SetEnvRequest {
  map<string, string> env_vars = 1;
  bool clear_existing = 2;           // reemplazar todo vs append
}

// ── Heartbeat ─────────────────────────────────────────────

message PingMessage {
  uint64 timestamp_ms = 1;           // epoch millis del gateway
}

message PongMessage {
  uint64 ping_timestamp_ms = 1;      // echo del ping
  uint64 worker_timestamp_ms = 2;    // timestamp del worker
  WorkerStatus status = 3;
}

message WorkerStatus {
  uint32 active_commands = 1;
  uint64 uptime_ms = 2;
  double cpu_usage = 3;              // 0.0 - 1.0
  uint64 memory_used_bytes = 4;
  uint64 memory_total_bytes = 5;
}

message ReadyMessage {
  string session_id = 1;             // matchea RegisterWorkerResponse
}
```

### 9.2 Sketch de Implementación Rust — Worker (gRPC Client)

```rust
//! bastion-worker/src/main.rs
//!
//! Worker que corre DENTRO del sandbox como gRPC CLIENT.
//! Se conecta al Gateway en el host via outbound connection.

use anyhow::Result;
use bastion_worker::WorkerConfig;
use clap::Parser;
use tokio::signal;
use tonic::transport::Channel;

mod sandbox; // proto-generated
mod connection;
mod executor;
mod file_ops;

use sandbox::v2::worker_gateway_client::WorkerGatewayClient;

#[derive(Parser)]
#[command(name = "bastion-worker", version)]
struct Args {
    /// Gateway address (host:port)
    #[arg(long, env = "BASTION_GATEWAY_ADDR")]
    gateway_addr: String,

    /// Sandbox ID (injected by gateway at container creation)
    #[arg(long, env = "BASTION_SANDBOX_ID")]
    sandbox_id: String,

    /// Auth token (JWT, injected by gateway)
    #[arg(long, env = "BASTION_AUTH_TOKEN")]
    auth_token: String,

    /// Working directory for logs and cache
    #[arg(long, default_value = "/tmp/bastion-worker")]
    work_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("bastion_worker=debug")
        .init();

    let args = Args::parse();
    
    let config = WorkerConfig {
        gateway_addr: args.gateway_addr,
        sandbox_id: args.sandbox_id,
        auth_token: args.auth_token,
        work_dir: args.work_dir,
        ..Default::default()
    };

    // Conectar al gateway con reconnection loop
    let mut worker = connection::ConnectionManager::new(config);
    
    // El ConnectionManager maneja:
    // - Registro inicial (RegisterWorker RPC)
    // - Apertura del CommandChannel (bidirectional stream)
    // - Reconnection con exponential backoff + jitter
    // - Heartbeat automático
    
    worker.run().await?;
    
    Ok(())
}
```

```rust
//! bastion-worker/src/connection.rs
//!
//! Connection manager con reconnection logic inspirado en Jenkins
//! pero mejorado con exponential backoff y graceful shutdown.

use tokio::time::{duration_until, Instant};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct ConnectionManager {
    config: WorkerConfig,
    state: ConnectionState,
}

enum ConnectionState {
    Disconnected,
    Registering,
    Connected { session_id: String },
    Reconnecting { attempt: u32, next_retry: Instant },
}

impl ConnectionManager {
    pub async fn run(&mut self) -> anyhow::Result<()> {
        loop {
            match self.state {
                ConnectionState::Disconnected | ConnectionState::Reconnecting { .. } => {
                    match self.try_connect().await {
                        Ok(session_id) => {
                            self.state = ConnectionState::Connected { session_id };
                            tracing::info!("Connected to gateway");
                        }
                        Err(e) => {
                            let (attempt, delay) = self.calculate_backoff();
                            self.state = ConnectionState::Reconnecting { 
                                attempt, 
                                next_retry: Instant::now() + delay 
                            };
                            tracing::warn!(
                                attempt,
                                retry_in_ms = delay.as_millis(),
                                error = %e,
                                "Connection failed, retrying"
                            );
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
                
                ConnectionState::Connected { ref session_id } => {
                    // Run command loop until disconnected
                    if let Err(e) = self.command_loop(session_id.clone()).await {
                        tracing::error!(error = %e, "Command loop failed");
                        self.state = ConnectionState::Reconnecting {
                            attempt: 0,
                            next_retry: Instant::now(),
                        };
                    }
                }
                
                ConnectionState::Registering => unreachable!(),
            }
        }
    }

    /// Exponential backoff con jitter: 1s, 2s, 4s, 8s, 16s, 30s, 30s, ...
    fn calculate_backoff(&self) -> (u32, Duration) {
        let attempt = match &self.state {
            ConnectionState::Reconnecting { attempt, .. } => attempt + 1,
            _ => 1,
        };
        
        let base_secs = 1u64;
        let max_secs = 30u64;
        let delay_secs = (base_secs * 2u64.pow(attempt.saturating_sub(1))).min(max_secs);
        
        // Jitter: random ±20%
        let jitter = (delay_secs as f64 * 0.2 * (rand::random::<f64>() - 0.5)).abs();
        let final_secs = ((delay_secs as f64 + jitter).max(1.0)) as u64;
        
        (attempt, Duration::from_secs(final_secs))
    }
}
```

### 9.3 Distribución del Binario por Backend

| Backend | Estrategia de Distribución | Ventaja |
|---|---|---|
| **Podman** | Bind-mount: `-v /path/to/bastion-worker:/usr/local/bin/bastion-worker:ro` | Zero-copy, siempre versión correcta |
| **Firecracker** | Incluir en el rootfs de la VM image (build-time) | No hay runtime injection |
| **gVisor** | Bind-mount igual que Podman (compatible con runsc) | Compatible |
| **Kubernetes** | **Init container** que copia el binary desde una ConfigMap/emptyDir | Kubernetes-native, versionable |

**Ejemplo Podman:**
```bash
podman run -d \
  --name sandbox-abc123 \
  -v /usr/local/bin/bastion-worker:/usr/local/bin/bastion-worker:ro,Z \
  -e BASTION_GATEWAY_ADDR=host.containers.internal:50051 \
  -e BASTION_SANDBOX_ID=abc123 \
  -e BASTION_AUTH_TOKEN=eyJhbGciOiJSUzI1NiIs... \
  debian:bookworm-slim \
  /usr/local/bin/bastion-worker
```

**Ejemplo Kubernetes:**
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: sandbox-abc123
spec:
  initContainers:
  - name: worker-injector
    image: bastion/worker:v0.1.0
    command: ["cp", "/usr/local/bin/bastion-worker", "/worker/bastion-worker"]
    volumeMounts:
    - name: worker-bin
      mountPath: /worker
  containers:
  - name: sandbox
    image: debian:bookworm-slim
    command: ["/worker/bastion-worker"]
    env:
    - name: BASTION_GATEWAY_ADDR
      value: "bastion-gateway:50051"
    - name: BASTION_SANDBOX_ID
      value: "abc123"
    - name: BASTION_AUTH_TOKEN
      valueFrom:
        secretKeyRef:
          name: sandbox-abc123-token
          key: token
    volumeMounts:
    - name: worker-bin
      mountPath: /worker
  volumes:
  - name: worker-bin
    emptyDir: {}
```

### 9.4 Modelo de Seguridad Propuesto

```
┌──────────────────────────────────────────────────────────┐
│                    SECURITY LAYERS                        │
├──────────────────────────────────────────────────────────┤
│                                                          │
│  Layer 1: Transport Encryption                           │
│  ┌────────────────────────────────────────────────┐     │
│  │  TLS 1.3 (mutual si es posible)                │     │
│  │  o alternativamente: WireGuard/SSH tunnel       │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  Layer 2: Authentication                                │
│  ┌────────────────────────────────────────────────┐     │
│  │  JWT firmado por el gateway                     │     │
│  │  - Claims: sandbox_id, created_at, expires_at   │     │
│  │  - Signing: Ed25519 (rápido, pequeño)           │     │
│  │  - TTL: = timeout del sandbox                   │     │
│  │  - Verificado en RegisterWorker                  │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  Layer 3: Message Authorization                         │
│  ┌────────────────────────────────────────────────┐     │
│  │  Proto oneof whitelist                          │     │
│  │  - Solo message types definidos en proto         │     │
│  │  - Sin serialización arbitraria (protobuf)       │     │
│  │  - Validación de tamaño por mensaje              │     │
│  │  - Rate limiting por sandbox                     │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  Layer 4: Sandbox Isolation                             │
│  ┌────────────────────────────────────────────────┐     │
│  │  Container/VM isolation (provider-level)        │     │
│  │  - No shared filesystem con host                 │     │
│  │  - Network namespace (sin acceso a gateway       │
│  │    interno, solo el endpoint expuesto)           │     │
│  │  - seccomp profile restrictivo                   │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**Flujo de autenticación:**

```
Gateway crea sandbox:
  1. Genera JWT {sandbox_id, created_at, expires_at}
  2. Pasa JWT como env var al contenedor
  3. Worker lee JWT al iniciar

Worker se registra:
  4. Worker → Gateway: RegisterWorker {sandbox_id, auth_token=JWT}
  5. Gateway verifica JWT signature + expiry + sandbox_id match
  6. Gateway → Worker: RegisterWorkerResponse {accepted=true, session_id}

Worker abre command channel:
  7. Worker → Gateway: CommandChannel stream
  8. Gateway valida session_id
  9. Channel establecido con session_id vinculada al sandbox_id
```

### 9.5 Diseño de Reconexión y Fiabilidad

```
                    ┌──────────────────┐
                    │   Start Worker   │
                    └────────┬─────────┘
                             │
                    ┌────────▼─────────┐
                    │  Connect to GW   │◄──────────────┐
                    └────────┬─────────┘               │
                             │                         │
                    ┌────────▼─────────┐               │
               ┌───►│  RegisterWorker  │               │
               │    └────────┬─────────┘               │
               │             │ accepted?                │
               │    No ──────┼──────► Error             │
               │             │ Yes                      │
               │    ┌────────▼─────────┐               │
               │    │ Open CmdChannel  │               │
               │    └────────┬─────────┘               │
               │             │                         │
               │    ┌────────▼─────────┐               │
               │    │  Command Loop    │               │
               │    │  (process cmds)  │               │
               │    └────────┬─────────┘               │
               │             │                         │
               │    ┌────────▼─────────┐               │
               │    │  Disconnected?   │─── Yes ───────┤
               │    └────────┬─────────┘               │
               │             │ No                      │
               │    ┌────────▼─────────┐               │
               │    │ Shutdown Signal? │─── Yes ──► Exit│
               │    └────────┬─────────┘               │
               │             │ No                      │
               │             └─────────► (loop)        │
               │                                       │
               │    Backoff: 1s → 2s → 4s → 8s → 16s → 30s max
               └──── Retry with exponential backoff + jitter
```

**In-flight commands durante desconexión:**
- El gateway mantiene un map de `command_id → sender` con TTL
- Si el worker se desconecta, los commands pendientes reciben `ErrorResult { retryable: true }`
- Al reconectar, el gateway puede optar por reenviar commands que no se completaron
- El worker NO necesita recordar estado entre reconexiones (stateless)

### 9.6 Comparación Proto: Actual vs Propuesto

**Actual (v1):**
```protobuf
service GatewayRegistry {
  rpc RegisterWorker (RegisterWorkerRequest) returns (RegisterWorkerResponse);
  rpc CommandStream (stream WorkerMessage) returns (stream GatewayCommand);
}
```

**Problemas del actual:**
- Sin auth token
- Sin capabilities
- Sin heartbeat
- Sin session tracking
- `WorkerMessage` y `GatewayCommand` son planos (no tienen command_id para correlación)
- Sin cancel support
- Sin env injection

**Propuesto (v2):**
```protobuf
service WorkerGateway {
  rpc RegisterWorker (RegisterWorkerRequest) returns (RegisterWorkerResponse);
  rpc CommandChannel (stream WorkerUpstream) returns (stream GatewayDownstream);
}
```

**Mejoras:**
- Auth token JWT en `RegisterWorkerRequest`
- `WorkerCapabilities` + `GatewayCapabilities` en handshake
- `command_id` en cada mensaje para correlación request-response
- `PingMessage`/`PongMessage` con `WorkerStatus` para health monitoring
- `CancelRequest` para abortar comandos en vuelo
- `ReadyMessage` para signaling explícito
- `SetEnvRequest` para inyección de env vars post-creación
- `ShutdownRequest.grace_period_ms` para drain controlado

---

## 10. Tabla Comparativa

| Aspecto | Jenkins Remoting | Bastion (Actual) | Bastion (Propuesto) |
|---|---|---|---|
| **Connection Model** | Agent → Controller (inbound TCP/WS) | Worker es gRPC SERVER (inbound) | Worker es gRPC CLIENT (outbound) |
| **Wire Format** | Java Serialization | Protobuf | Protobuf (mejorado) |
| **Framing** | Length-prefixed TCP / WebSocket frames | gRPC HTTP/2 | gRPC HTTP/2 |
| **Authentication** | HMAC secret (static) | Ninguna | JWT con TTL |
| **Encryption** | TLS (JNLP4) / HTTPS (WS) | Ninguna | TLS 1.3 |
| **Capability Neg.** | Bitmap en handshake | Version string | Structured WorkerCapabilities |
| **Multiplexing** | Virtual streams over TCP | gRPC stream (single) | gRPC stream + command_id routing |
| **Binary Dist.** | HTTP download (170KB JAR) | tar+upload | Bind-mount / init container |
| **Reconnection** | Manual (process restart) | Ninguna | Auto con exponential backoff |
| **Heartbeat** | Optional ping (disabled default) | Ninguno | Ping/Pong con WorkerStatus |
| **Concurrent Cmds** | Multiple executors (threads) | Una sola instancia | Múltiples via command_id |
| **Security Model** | Agent→Ctrl ACL + ClassFilter | Ninguno | JWT + TLS + proto whitelist |
| **File Transfer** | Virtual streams | gRPC bytes field | Chunked con offset/limit |
| **Graceful Shutdown** | CloseCommand | kill container | ShutdownRequest con grace_period |
| **Streaming Output** | Virtual streams | gRPC server stream | Bidirectional gRPC stream |
| **Error Handling** | ChannelClosedException | Container removal | ErrorResult con retryable flag |
| **Observability** | FlightRecorder (1MB ring buffer) | tracing logs | OpenTelemetry + WorkerStatus |
| **Runtime** | JVM (Java) | Tokio (Rust) | Tokio (Rust) |
| **Size** | 170KB (agent.jar) | ~5-10MB (Rust binary) | ~5-10MB (Rust binary) |

---

## Apéndice A: Referencias

- [Jenkins Remoting Project](https://www.jenkins.io/projects/remoting/)
- [Remoting Library Source](https://github.com/jenkinsci/remoting)
- [JNLP4-connect Protocol](https://github.com/jenkinsci/remoting/blob/master/docs/protocols.md)
- [JEP-222: WebSocket Support](https://github.com/jenkinsci/jep/blob/master/jep/222/README.adoc)
- [Inbound Agent Launch](https://github.com/jenkinsci/remoting/blob/master/docs/inbound-agent.md)
- [Remoting Configuration](https://github.com/jenkinsci/remoting/blob/master/docs/configuration.md)
- [Channel Shutdown Process](https://github.com/jenkinsci/remoting/blob/master/docs/close.md)
- [Controller Isolation / Security](https://www.jenkins.io/doc/book/security/controller-isolation/)
- [Jenkins Distributed Builds](https://www.jenkins.io/doc/book/managing/nodes/)

## Apéndice B: Archivos Analizados de Bastion

| Archivo | Propósito | Observación |
|---|---|---|
| `proto/sandbox/v1/sandbox.proto` | Definición gRPC actual | Necesita v2 con auth, capabilities |
| `crates/bastion-worker/src/main.rs` | Worker entry point | Actualmente lanza SERVER, debe ser CLIENT |
| `crates/bastion-gateway/src/registry.rs` | Registry service (gRPC server) | MVP incompleto, necesita CommandChannel routing |
| `crates/bastion-infrastructure/src/provider/podman.rs` | Podman adapter | Usa tar+upload + port mapping, debe usar bind-mount |
| `crates/bastion-infrastructure/src/grpc/client.rs` | gRPC client stub | Vacío, necesita implementación completa |
| `crates/bastion-domain/src/provider/port.rs` | SandboxProvider trait | Bien diseñado, no requiere cambios |
| `crates/bastion-domain/src/provider/capabilities.rs` | ProviderCapabilities | Buen punto de partida, ampliar |
| `crates/bastion-domain/src/execution/stream.rs` | CommandChunk types | Adecuado, mapea bien a proto |
| `crates/bastion-domain/src/sandbox/entity.rs` | Sandbox entity | No requiere cambios para rediseño |
