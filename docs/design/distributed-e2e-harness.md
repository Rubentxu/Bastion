# Exploration: Distributed Systems E2E Testing Harness

**Date**: 2026-05-02
**Status**: Complete
**Confidence**: High (estimated)

---

## 1. Context

Investigar el diseño de un crate de E2E testing harness que sea:
1. Abstracto y reutilizable para múltiples tipos de sistemas distribuidos
2. No acoplado a Bastion específicamente
3. capaz de probar Gateway MCP + Providers (Podman, Firecracker, gVisor) + Worker Registry

---

## 2. Current State Analysis

### 2.1 Bastion's Current E2E Testing

Los tests E2E actuales en Bastion presentan varios problemas:

**Location**: `crates/bastion-gateway/tests/e2e_test.rs`

**Pattern actual (problemático)**:

```rust
fn spawn_gateway() -> (std::process::Child, impl Write, impl BufRead) {
    let binary = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        // ... calculate path to binary ...
    let mut cmd = Command::new(&binary);
    cmd.arg("--image").arg("debian:bookworm-slim");
    let child = cmd.spawn().expect("Failed to spawn");
    // No health check!
    (child, stdin, stdout)
}

#[test]
fn test_gateway_e2e_lifecycle() {
    let (mut child, mut stdin, mut reader) = spawn_gateway();
    std::thread::sleep(Duration::from_millis(500)); // Magic number!
    // ... test code ...
    let _ = child.kill(); // Manual cleanup
}
```

**Problemas identificados**:

| Problem | Severity | Impact |
|---------|----------|--------|
| No health checks | HIGH | Flaky tests, race conditions |
| Magic sleep (500ms) | MEDIUM | Test brittleness |
| Manual process kill | MEDIUM | Orphaned processes on panic |
| Stdio::null() for stderr | MEDIUM | No log capture for debugging |
| Coupled to Bastion | HIGH | Non-reusable |

### 2.2 Bastion's Provider Architecture (Positive Pattern)

Bastion ya tiene un buen patrón de abstracción con `SandboxProvider`:

**Location**: `crates/bastion-domain/src/provider/port.rs`

```rust
#[async_trait]
pub trait SandboxProvider: Send + Sync + std::fmt::Debug {
    async fn create(...) -> Result<Sandbox, DomainError>;
    async fn terminate(...) -> Result<(), DomainError>;
    async fn is_alive(...) -> Result<bool, DomainError>;
    async fn run_command(...) -> Result<CommandResult, DomainError>;
    fn capabilities() -> ProviderCapabilities;
    fn name() -> &str;
}
```

Este trait sigue **Dependency Inversion Principle** (DIP) - el dominio define la interfaz, infraestructura la implementa.

---

## 3. State of the Art Analysis

### 3.1 testcontainers-rs (Rust)

**Repository**: https://github.com/testcontainers/testcontainers-rs

**Architecture**:

```
Image (trait)
├── GenericImage (concrete)
├── with_exposed_port() -> Self
├── with_wait_for(WaitFor) -> Self
├── with_health_check(Healthcheck) -> Self
└── start() -> Container

WaitFor (trait)
├── message_on_stdout(String)
├── message_on_stderr(String)
├── healthcheck()
└── seconds(u64)

Healthcheck (struct)
├── cmd(["mysqladmin", "ping"])
├── with_interval(Duration)
├── with_timeout(Duration)
└── with_retries(u32)
```

**Key Patterns**:
- **Builder pattern** para configuración fluente
- **Automatic cleanup via Drop** para containers
- **Composition** sobre inheritance
- **AsyncRunner trait** para abstraer runtime

**Example**:

```rust
let redis = GenericImage::new("redis", "7.2.4")
    .with_exposed_port(6379.tcp())
    .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
    .start()
    .await?;

let port = redis.get_host_port_ipv4(6379.tcp()).await?;
// Automatic cleanup when `redis` goes out of scope
```

### 3.2 dockertest (Go - ORY)

**Repository**: https://github.com/ory/dockertest

**Architecture**:

```go
Pool
├── NewPoolT(t, "") // Ties to test lifecycle
├── RunT(t, "postgres", WithTag("14"))
├── CreateNetworkT(t, name)
└── Close(ctx) // Cleanup all

Resource
├── GetHostPort("5432/tcp")
├── GetIPInNetwork(net)
├── Close(ctx)
└── ConnectToNetwork(net)
```

**Key Patterns**:
- **Test lifecycle integration** - `NewPoolT(t, "")` conecta cleanup al test
- **`t.Cleanup()`** integration automática
- **Pool reuse** - containers se reutilizan entre tests si tienen mismo image:tag
- **Network isolation** - networks creadas y limpiadas con el pool
- **Retry helper** - `pool.Retry(timeout, func)` para esperar servicios

**Example**:

```go
pool, _ := dockertest.NewPoolT(t, "")
postgres := pool.RunT(t, "postgres",
    dockertest.WithTag("14-alpine"),
    dockertest.WithEnv([]string{"POSTGRES_PASSWORD=secret"}),
)
defer pool.Close(nil)

dsn := fmt.Sprintf("postgres://...%s/testdb", postgres.GetHostPort("5432/tcp"))
db, _ := sql.Open("postgres", dsn)
pool.Retry(t.Context(), 30*time.Second, func() error {
    return db.Ping()
})
```

### 3.3 wiremock-rs (Rust)

**Repository**: https://github.com/lukemathwalker/wiremock-rs

**Architecture**:

```rust
MockServer
├── start() -> Self (background server)
├── uri() -> String
└── uri_for(path) -> String

Mock
├── given(RequestMatcher)
├── and(RequestMatcher)
├── respond_with(ResponseTemplate)
└── mount(&MockServer)

matchers (module)
├── method("GET")
├── path("/hello")
├── query_param("q", "rust")
└── header("Authorization", "Bearer ...")

ResponseTemplate
├── new(200)
├── set_body_json(serde_json::Value)
└── insert_header(key, value)
```

**Key Patterns**:
- **Matcher composition** - `Mock::given(method("GET")).and(path("/api")).respond_with(...)`
- **Closure-based dynamic responses** - `respond_with(|req: &Request| ResponseTemplate::new(200))`
- **Background server** - MockServer arranca en background
- **Verification** - verificar qué requests se hicieron

**Example**:

```rust
let mock_server = MockServer::start().await;

Mock::given(method("POST"))
    .and(path("/api/data"))
    .and(body_json(&expected_request))
    .respond_with(ResponseTemplate::new(200).set_body_json(&response))
    .mount(&mock_server)
    .await;

let status = reqwest::get(format!("{}/hello", &mock_server.uri())).await?;
```

---

## 4. Proposed Architecture

### 4.1 Design Principles

1. **Dependency Inversion**: El harness define traits, backends concretos implementan
2. **Composition sobre Inheritance**: Traits pequeños y composables
3. **Test Framework Integration**: Como dockertest, cleanup automático via Rust drop
4. **Async-first**: Tokio como runtime por defecto, pero抽象 permite otros
5. **Zero-cost Abstractions**: Traits bien diseñados para que backends concretos no paguen overhead

### 4.2 Core Traits

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Core Abstractions (dist-test-harness)
// ═══════════════════════════════════════════════════════════════════════════

use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

// ── Resource Identifiers ───────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceId(String);

impl ResourceId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

// ── Port Mapping ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PortMapping {
    pub container_port: u16,
    pub host_port: Option<u16>,  // None = auto-assign
    pub protocol: Protocol,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Protocol { Tcp, Udp, #[default] Sctp }

impl PortMapping {
    pub fn tcp(container: u16) -> Self {
        Self { container_port: container, host_port: None, protocol: Protocol::Tcp }
    }
}

// ── Service Configuration ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub image: String,
    pub command: Option<Vec<String>>,
    pub env_vars: HashMap<String, String>,
    pub ports: Vec<PortMapping>,
    pub binds: Vec<String>,           // volume mounts
    pub network: Option<String>,      // join existing network
    pub labels: HashMap<String, String>,
}

impl ServiceConfig {
    pub fn new(name: impl Into<String>, image: impl Into<String>) -> Self { ... }
    pub fn with_port(mut self, port: PortMapping) -> Self { ... }
    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self { ... }
    pub fn with_command(mut self, cmd: Vec<String>) -> Self { ... }
    pub fn on_network(mut self, network: impl Into<String>) -> Self { ... }
}

// ── Wait Strategies ────────────────────────────────────────────────────────

#[async_trait]
pub trait WaitStrategy: Send + Sync + std::fmt::Debug {
    async fn wait_until_ready<C: ServiceClient + 'static>(&self, client: C) -> Result<(), WaitError>;
}

#[derive(Debug, thiserror::Error)]
pub enum WaitError {
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("service not ready: {0}")]
    NotReady(String),
    #[error("check failed: {0}")]
    CheckFailed(String),
}

// Concrete strategies:

/// Wait for HTTP endpoint to return 200
pub struct HttpWaitStrategy {
    pub url: String,
    pub timeout: Duration,
    pub interval: Duration,
}

/// Wait for log message to appear
pub struct LogMessageWaitStrategy {
    pub pattern: String,
    pub timeout: Duration,
    pub interval: Duration,
}

/// Wait for fixed duration (fallback)
pub struct DurationWaitStrategy(pub Duration);

// ── Health Checks ──────────────────────────────────────────────────────────

#[async_trait]
pub trait HealthCheck: Send + Sync + std::fmt::Debug {
    async fn is_healthy(&self) -> Result<bool, HealthError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    #[error("check failed: {0}")]
    CheckFailed(String),
    #[error("service unavailable")]
    Unavailable,
}

// Concrete checks:

/// TCP port health check
pub struct TcpHealthCheck { pub host: String, pub port: u16 }

/// HTTP health check
pub struct HttpHealthCheck { pub url: String }

// ── Log Collector ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LogLine {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub stream: LogStream,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogStream { Stdout, Stderr }

#[async_trait]
pub trait LogCollector: Send + Sync {
    async fn logs(&self) -> Result<Vec<LogLine>, LogError>;
    fn contains(&self, logs: &[LogLine], pattern: &str) -> bool {
        logs.iter().any(|l| l.message.contains(pattern))
    }
}

// ── Service Client (for wait strategies) ──────────────────────────────────

#[async_trait]
pub trait ServiceClient: Send + Sync + Clone + std::fmt::Debug {
    async fn logs(&mut self) -> Result<Vec<LogLine>, LogError>;
    async fn ping(&self) -> Result<bool, ()>;
}

// ── Service Handle ─────────────────────────────────────────────────────────

pub struct ServiceHandle<C: ServiceClient = DockerServiceClient> {
    pub id: ResourceId,
    pub host: String,
    pub ports: HashMap<u16, u16>,  // container_port -> host_port
    pub client: C,
}

// ── Test Harness ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct TestHarness<M: ResourceManager> {
    manager: M,
    services: Vec<ServiceHandle<M::Client>>,
    networks: Vec<NetworkHandle>,
}

#[async_trait]
pub trait ResourceManager: Send + Sync + std::fmt::Debug {
    type Error: std::error::Error + Send + Sync + 'static;
    type Client: ServiceClient + 'static;

    async fn start(&self, config: &ServiceConfig) -> Result<ResourceId, Self::Error>;
    async fn stop(&self, id: &ResourceId) -> Result<(), Self::Error>;
    async fn is_ready(&self, id: &ResourceId) -> Result<bool, Self::Error>;

    async fn client_for(&self, id: &ResourceId) -> Result<Self::Client, Self::Error>;
    async fn host_for(&self, id: &ResourceId) -> Result<String, Self::Error>;
    async fn ports_for(&self, id: &ResourceId) -> Result<Vec<PortMapping>, Self::Error>;

    async fn create_network(&self, name: &str) -> Result<NetworkId, Self::Error>;
    async fn remove_network(&self, id: &NetworkId) -> Result<(), Self::Error>;
}

impl<M: ResourceManager> Drop for TestHarness<M> {
    fn drop(&mut self) {
        // Cleanup all services - best effort since we're in drop
        for service in &self.services {
            let _ = self.manager.stop(&service.id).now_or_never();
        }
        for network in &self.networks {
            let _ = self.manager.remove_network(&network.id).now_or_never();
        }
    }
}
```

### 4.3 Bastion-Specific Implementation

```rust
// ═══════════════════════════════════════════════════════════════════════════
// Bastion Test Harness Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Bastion's test harness with Podman backend
pub type BastionTestHarness = TestHarness<PodmanResourceManager>;

#[derive(Debug)]
pub struct PodmanResourceManager {
    socket_path: String,
    default_image: String,
}

impl PodmanResourceManager {
    pub fn new(socket_path: &str, default_image: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            default_image: default_image.to_string(),
        }
    }
}

#[async_trait]
impl ResourceManager for PodmanResourceManager {
    type Error = BastionTestError;
    type Client = PodmanServiceClient;

    async fn start(&self, config: &ServiceConfig) -> Result<ResourceId, Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;

        let container_name = config.name.clone();
        let image = if config.image.is_empty() { &self.default_image } else { &config.image };

        let env: Vec<String> = config.env_vars.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        let ports: HashMap<String, Option<Vec<bollard::models::PortBinding>>> = config.ports.iter()
            .map(|pm| {
                let key = format!("{}/tcp", pm.container_port);
                let binding = pm.host_port.map(|hp| vec![bollard::models::PortBinding {
                    host_ip: "0.0.0.0".to_string(),
                    host_port: hp.to_string(),
                }]);
                (key, binding)
            })
            .collect();

        let create_body = bollard::models::ContainerCreateBody {
            image: Some(image.to_string()),
            cmd: config.command.clone(),
            env: Some(env),
            host_config: Some(bollard::models::HostConfig {
                port_bindings: Some(ports),
                binds: Some(config.binds.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        docker.create_container(
            Some(CreateContainerOptions { name: &container_name, platform: None }),
            create_body
        ).await?;

        docker.start_container(&container_name, None).await?;

        Ok(ResourceId::new(container_name))
    }

    async fn stop(&self, id: &ResourceId) -> Result<(), Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;
        docker.stop_container(id.as_str(), Some(StopContainerOptions { t: 10 })).await?;
        docker.remove_container(id.as_str(), Some(RemoveContainerOptions { force: true, ..Default::default() })).await?;
        Ok(())
    }

    async fn is_ready(&self, id: &ResourceId) -> Result<bool, Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;
        match docker.inspect_container(id.as_str(), None).await {
            Ok(info) => Ok(info.state.map(|s| s.running == Some(true)).unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    async fn client_for(&self, id: &ResourceId) -> Result<Self::Client, Self::Error> {
        Ok(PodmanServiceClient { id: id.clone(), socket_path: self.socket_path.clone() })
    }

    async fn host_for(&self, _id: &ResourceId) -> Result<String, Self::Error> {
        Ok("host.containers.internal".to_string())
    }

    async fn ports_for(&self, id: &ResourceId) -> Result<Vec<PortMapping>, Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;
        let info = docker.inspect_container(id.as_str(), None).await?;
        // Parse ports from network_settings...
        Ok(vec![])
    }

    async fn create_network(&self, name: &str) -> Result<NetworkId, Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;
        let response = docker.create_network(CreateNetworkOptions { name, ..Default::default() }).await?;
        Ok(ResourceId::new(response.id))
    }

    async fn remove_network(&self, id: &NetworkId) -> Result<(), Self::Error> {
        let docker = Docker::connect_with_unix(&self.socket_path, 120, API_DEFAULT_VERSION)?;
        docker.remove_network(id.as_str()).await?;
        Ok(())
    }
}
```

### 4.4 Usage Example

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bastion_e2e_with_harness() {
        let mut harness = BastionTestHarness::new(
            PodmanResourceManager::new("/run/user/1000/podman/podman.sock", "debian:bookworm-slim")
        );

        // Create isolated network
        let network = harness.create_network("bastion-test").await.unwrap();

        // Launch gateway service
        let gateway = harness
            .launch(
                ServiceConfig::new("bastion-gateway", "bastion-gateway:latest")
                    .with_port(PortMapping::tcp(50051))
                    .with_env("RUST_LOG", "info")
                    .on_network(&network),
                HttpWaitStrategy {
                    url: "http://host.containers.internal:50051/health".to_string(),
                    timeout: Duration::from_secs(30),
                    interval: Duration::from_millis(500),
                },
            )
            .await
            .expect("Gateway should start");

        // Launch worker service
        let worker = harness
            .launch(
                ServiceConfig::new("bastion-worker", "bastion-worker:latest")
                    .with_env("GATEWAY_ADDR", "host.containers.internal:50051")
                    .on_network(&network),
                LogMessageWaitStrategy {
                    pattern: "Worker connected".to_string(),
                    timeout: Duration::from_secs(30),
                    interval: Duration::from_millis(500),
                },
            )
            .await
            .expect("Worker should start");

        // Test the actual functionality
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}:{}/rpc", gateway.host, gateway.ports[&50051]))
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "sandbox_create",
                "params": { "template": "debian:bookworm-slim" }
            }))
            .send()
            .await
            .expect("Request should succeed");

        assert!(resp.status().is_success());

        // Auto-cleanup happens when `harness` goes out of scope
    }
}
```

---

## 5. Approaches

### Approach 1: Incremental Improvement (Recommended for Bastion)

**Description**: Erweitert los patterns existentes con abstracciones livianas sin crear crate separado.

**Pros**:
- Bajo riesgo, puede implementarse gradualmente
- No requiere reescribir todos los tests de golpe
- Mantiene compatibilidad con código existente
- Suficiente para las necesidades actuales de Bastion

**Cons**:
- No será tan abstracto como Approach 2
- Acoplamiento依然 a Bastion
- Debt técnica que crecerá con el tiempo

**Complexity**: Medium

### Approach 2: Full Abstract Harness (For Other Projects)

**Description**: Crate genérico `dist-test-harness` con abstracciones completas, publicable en crates.io.

**Pros**:
- Totalmente reutilizable para otros proyectos
- Diseño limpio con traits bien definidos desde el inicio
- Soporte para múltiples backends (Docker, Podman, K8s, SSH, etc.)
- Potencial para contribución a la comunidad

**Cons**:
- Alto esfuerzo inicial (semanas vs días)
- Over-engineering para el caso de Bastion
- Requiere mantener crate separado con versioning, CI, docs
- Más difícil de iterar una vez publicado

**Complexity**: High

---

## 6. Recommendation

**Para Bastion**: Approach 1 (Incremental) es mejor.

**Rationale**:
1. Los tests actuales funcionan, no hay presión para reescribir todo
2. Approach 2 requeriría semanas de esfuerzo y no agrega valor inmediato para Bastion
3. El beneficio real de Approach 2 es para otros proyectos, no para Bastion

**Implementation Plan (Approach 1)**:

1. **Fase 1**: Crear `TestHarness` struct simple con Podman backend
   - `PodmanResourceManager` que usa bollard
   - `spawn_gateway_with_harness()` que usa el harness
   - Auto-cleanup via Drop

2. **Fase 2**: Agregar wait strategies básicos
   - `HttpWaitStrategy`
   - `LogMessageWaitStrategy`
   - Reemplazar `std::thread::sleep()` con wait strategies

3. **Fase 3**: Agregar health checks configurables
   - `TcpHealthCheck`
   - `HttpHealthCheck`
   -掉的 `Stdio::null()` - capturar logs

4. **Fase 4**: Si hay demanda, extraer a `dist-test-harness` crate

---

## 7. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Over-engineering Approach 2 | HIGH | MEDIUM | Stick to Approach 1 |
| Flaky tests sin proper health checks | HIGH | HIGH | Implement WaitStrategy antes de merge |
| Orphaned containers si Drop no called | MEDIUM | MEDIUM | Usar `tokio::spawn` con `Abortable` |
| Magic numbers para timeouts | MEDIUM | LOW | Documentar rationale o usar config |

---

## 8. Entropy Analysis (Connascence Landscape)

**Method**: Heuristic

| Component A | Component B | Connascence Type | I(bits) | Severity |
|------------|-------------|------------------|---------|----------|
| `e2e_test.rs` | `spawn_gateway()` | Meaning | 0.58 | ✅ OK |
| `SandboxProvider` | `PodmanProvider` | Type | 1.15 | ⚠️ MEDIUM |
| `TestHarness<M>` | `ResourceManager` | Name | 1.5 | ⚠️ MEDIUM |
| `bollard` crate | `PodmanProvider` | Meaning | 2.1 | ⚠️ MEDIUM |

**Critical Pairs (I > 3.0 bits)**: None

**Hidden Connascence (Meaning/Timing)**:
- `std::thread::sleep(Duration::from_millis(500))` — magic number 500ms para startup sin justification
- `child.kill()` en vez de graceful shutdown
- Hardcoded socket path `"/run/user/1000/podman/podman.sock"`

**Coupling Score**: H_external estimated

**Estimation Method**: Heuristic

**Confidence**: estimated

---

## 9. Files to Review for Next Steps

- `crates/bastion-gateway/tests/e2e_test.rs` — Test E2E actual
- `crates/bastion-domain/src/provider/port.rs` — Trait SandboxProvider (buen ejemplo de DIP)
- `crates/bastion-infrastructure/src/provider/podman.rs` — PodmanProvider implementation
- `crates/bastion-infrastructure/tests/e2e_worker_v2.rs` — Tests de worker
- `Cargo.toml` — Dependencies (bollard, tokio, async-trait ya disponibles)

---

## 10. Prototype Implementation (for Reference)

El código completo de la sección 4 puede usarse como base para un prototype. No requiere crear crate separado - puede vivir en `crates/bastion-test-harness/` y evolucionar.

**Suggested structure**:

```
crates/bastion-test-harness/
├── Cargo.toml          # dev-dependency only, not published
├── src/
│   ├── lib.rs          # exports
│   ├── harness.rs      # TestHarness<M>
│   ├── manager.rs      # ResourceManager trait + PodmanResourceManager
│   ├── wait.rs         # WaitStrategy traits + implementations
│   ├── health.rs       # HealthCheck traits + implementations
│   └── logs.rs         # LogCollector
└── tests/
    └── e2e_gateway.rs  # Migrated tests
```

---

## 11. Appendix: Comparison of Existing Solutions

| Feature | testcontainers-rs | dockertest | wiremock-rs | Proposed |
|---------|-------------------|------------|------------|----------|
| Container lifecycle | ✅ | ✅ | N/A | ✅ |
| Wait strategies | ✅ | ⚠️ (retry manual) | N/A | ✅ |
| Health checks | ✅ | ❌ | N/A | ✅ |
| Log capture | ✅ | ⚠️ (external) | ⚠️ (partial) | ✅ |
| Network isolation | ✅ | ✅ | N/A | ✅ |
| Cleanup on drop | ✅ | ✅ | ✅ (MockServer) | ✅ |
| Abstract backend | ⚠️ (Docker only) | ⚠️ (Docker only) | N/A | ✅ |
| Mock HTTP server | ❌ | ❌ | ✅ | ⚠️ (future) |
| Async support | ✅ | ⚠️ (Go) | ✅ | ✅ |
| Test framework integration | ❌ | ✅ | ❌ | ✅ |

**Conclusion**: La propuesta combina lo mejor de testcontainers-rs (abstracciones, wait strategies) con dockertest (test framework integration) y extiende para soporte multi-backend.
