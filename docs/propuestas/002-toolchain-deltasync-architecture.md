# Propuesta 002 — Toolchains, Delta Sync y ejecución homogénea

## Objetivo

Hacer que Bastion sea útil para agentes de IA y usuarios humanos cuando necesitan
preparar entornos reales de desarrollo — Java/Maven, Node, Python, Go, Rust,
etc. — manteniendo:

- **Homogeneidad**: todo sandbox ejecuta mediante `bastion-worker` y gRPC.
- **Portabilidad**: Podman, Docker, Firecracker, gVisor y Kubernetes comparten
  el mismo modelo lógico.
- **Alto rendimiento**: evitar instalaciones repetidas, copias completas y
  roundtrips innecesarios.
- **Zero-cost cuando sea posible**: aprovechar page cache, hardlinks,
  content-addressing, capas inmutables y streaming.
- **Seguridad**: minimizar superficie de ataque, verificar integridad, controlar
  permisos y mantener auditoría.

---

## Principio rector

No diseñar alrededor de Docker, Podman, rsync o asdf. Diseñar alrededor de una
abstracción estable:

> Un sandbox es un entorno aislado que ejecuta un `bastion-worker` autenticado
> por gRPC. Todo lo demás — contenedor local, microVM, pod remoto, imagen con
> tools preinstaladas, layer overlay, rsync — es estrategia interna del provider.

Esto evita que una optimización local rompa Firecracker, gVisor o Kubernetes.

---

## Abstracciones invariantes

### 1. WorkerDelivery

El worker no siempre se inyecta igual.

| Provider | Estrategia posible |
|----------|--------------------|
| Podman/Docker local | bind mount, copy, layer, image baked |
| Firecracker | baked en rootfs, init fetch, vsock/HTTP bootstrap |
| gVisor | imagen baked, sidecar, copy-on-create |
| Kubernetes | image baked, initContainer, projected volume, sidecar |

Por tanto conviene explicitar la abstracción:

```rust
#[async_trait]
pub trait WorkerDelivery: Send + Sync {
    async fn ensure_worker(
        &self,
        sandbox: &SandboxId,
        spec: &WorkerSpec,
    ) -> Result<WorkerEndpoint, DomainError>;
}
```

El objetivo no es exponer esto al agente, sino garantizar que todos los providers
terminan con lo mismo: un `bastion-worker` conectado al Registry gRPC.

### 2. Toolchain, no “tool suelto”

Un agente no quiere “instala java”. Quiere capacidad:

```json
{
  "capability": "jvm-build",
  "constraints": {
    "java": "17",
    "build_tool": "maven",
    "network": true
  }
}
```

Bastion debe resolver eso a una toolchain concreta.

```rust
pub struct ToolchainRequest {
    pub capability: String,          // "jvm-build", "node-build", "python-test"
    pub constraints: ToolConstraints,
    pub policy: ToolchainPolicy,
}

pub struct ToolchainPlan {
    pub layers: Vec<ToolLayerRef>,
    pub managers: Vec<ToolManagerStep>,
    pub env: HashMap<String, String>,
    pub verification: Vec<VerificationStep>,
}
```

### 3. ToolManagerAdapter

asdf, sdkman, brew, nix, apt, npm, pip, cargo no deben aparecer como lógica
especial dispersa. Son adapters.

```rust
#[async_trait]
pub trait ToolManagerAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn supports(&self, req: &ToolchainRequest) -> SupportLevel;
    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolManagerPlan, DomainError>;
    async fn verify(&self, sandbox: &SandboxId, plan: &ToolManagerPlan) -> Result<(), DomainError>;
}
```

Adapters iniciales:

- `AptAdapter` — rápido y simple en Debian/Ubuntu.
- `AsdfAdapter` — útil para múltiples runtimes, pero hay que controlar shims y
  entorno.
- `SdkmanAdapter` — especialmente útil para JVM: Java, Maven, Gradle.
- `BrewAdapter` — Linuxbrew para entornos homogéneos fuera de Debian.
- `NixAdapter` — ideal a futuro para reproducibilidad y content-addressing.
- `CaStoreAdapter` — herramientas ya preparadas por hash.

---

## Replanteamiento: no “instalar”, sino resolver capacidad

### Modelo propuesto

```
Agent/User request
   │
   ▼
ToolchainResolver
   │
   ├─ checks existing sandbox capabilities
   ├─ checks template capabilities
   ├─ checks local content-addressable cache
   ├─ chooses tool manager strategy
   └─ emits ToolchainPlan
   │
   ▼
Provider applies plan
   │
   ├─ mount layer if possible
   ├─ copy/fetch if remote provider needs it
   ├─ run manager install if unavoidable
   └─ verify via worker gRPC
```

El agente ve una operación simple:

```python
sandbox_prepare(
  sandbox_id,
  capability="jvm-build",
  constraints={"java": "17", "maven": ">=3.8"}
)
```

No necesita saber si internamente se usó apt, sdkman, asdf, una capa overlay,
una imagen baked o una restauración de snapshot.

---

## Estrategias de toolchain

### Estrategia A — Base image mínima + instalación bajo demanda

**Uso:** desarrollo rápido, fallback universal.

**Ejemplo:** `apt-get install default-jdk maven git`.

**Pros**

- Simple.
- Funciona hoy.
- Fácil de depurar.

**Contras**

- Lento.
- Repite trabajo.
- Depende de red externa.
- Riesgo de drift por repositorios cambiantes.

**Seguridad**

- Requiere allowlist de repos.
- Verificación débil salvo firma de paquetes del distro.

### Estrategia B — Toolchain layer content-addressed

**Uso:** alto rendimiento y repetibilidad.

Una toolchain se empaqueta como layer inmutable:

```
toolchain:jvm-build/java17-maven3.9
  hash: sha256:...
  files: /opt/bastion/toolchains/jvm/java17-maven3.9
  env: JAVA_HOME, MAVEN_HOME, PATH
  verifies: java -version, mvn -version
```

**Pros**

- Montaje/copia mucho más rápido que instalar.
- Dedupe por hash.
- Verificación fuerte.
- Ideal para cache global.

**Contras**

- Hay que construir y mantener layers.
- Providers remotos pueden necesitar fetch/copy en vez de mount.

**Zero-cost local**

- Podman/Docker local: bind/overlay readonly.
- Page cache comparte binarios entre sandboxes.

**Remote cost**

- Firecracker/K8s remoto: fetch layer si no está en node cache.
- Aun así se evita resolver paquetes en runtime.

### Estrategia C — Version managers controlados

**Uso:** cuando el usuario pide toolchains dinámicas o versiones exactas.

Adapters:

- `SdkmanAdapter` para JVM probablemente mejor que asdf en Java/Maven.
- `AsdfAdapter` útil para Node, Python, Ruby, Go, etc.
- `BrewAdapter` útil para CLI tools multiplataforma.
- `NixAdapter` ideal a futuro si aceptamos su complejidad.

**Pros**

- Muy flexible.
- Familiar para usuarios.
- Permite versiones exactas.

**Contras**

- Shims y shell init son fuente frecuente de bugs.
- Más difícil de asegurar.
- Puede requerir red y scripts remotos.

**Requisito de diseño**

Nunca depender de `.bashrc` implícito. El plan debe producir `env` explícito:

```json
{
  "PATH": "/root/.sdkman/candidates/java/current/bin:/root/.sdkman/candidates/maven/current/bin:$PATH",
  "JAVA_HOME": "/root/.sdkman/candidates/java/current"
}
```

El worker debe ejecutar con `CommandSpec.env`, no esperar a shells login.

---

## Rsync / Delta Sync

### Evaluación honesta

Rsync es excelente para sincronizar árboles de archivos mutables, pero no debe
ser el núcleo de instalación de toolchains.

### Donde rsync sí aporta

#### 1. Sync de workspace host → sandbox

Cuando el agente edita localmente y quiere probar en sandbox:

```python
sandbox_sync_push(
  local="./repo",
  remote="/workspace/repo",
  excludes=["target", "node_modules", ".git"]
)
```

Ventajas:

- Delta transfer.
- Compresión.
- Preserva permisos.
- Muy útil para repos grandes.

#### 2. Sync de artifacts sandbox → host

```python
sandbox_sync_pull(
  remote="/workspace/repo/target",
  local="./artifacts"
)
```

#### 3. Checkpoint parcial de workspace

Antes de destruir sandbox, traer solo diffs importantes.

### Donde rsync NO aporta

#### 1. Instalación de tools compartidas

Para Java/Maven/Git como toolchains:

- Mejor content-addressed layer.
- Mejor hardlink/overlay local.
- Mejor node cache remoto.
- Rsync no deduplica globalmente.

#### 2. Worker delivery homogéneo

El worker debe seguir `WorkerDelivery`, no depender de rsync.

### Problemas de seguridad de rsync

- Si usa SSH, requiere servidor/clave dentro del sandbox.
- Si usa rsync daemon, abre superficie de red.
- Si se usa desde host con `podman cp`/exec, depende del provider.

### Recomendación

Implementar **DeltaSync abstraction**, no “rsync directly”.

```rust
#[async_trait]
pub trait DeltaSyncProvider: Send + Sync {
    async fn push(&self, sandbox: &SandboxId, req: SyncRequest) -> Result<SyncReport, DomainError>;
    async fn pull(&self, sandbox: &SandboxId, req: SyncRequest) -> Result<SyncReport, DomainError>;
}
```

Backends posibles:

| Backend | Uso |
|---------|-----|
| `RsyncBackend` | Local/SSH-capable, repos grandes |
| `TarStreamBackend` | Universal, simple, funciona vía gRPC chunks |
| `ContentHashBackend` | Futuro, dedupe por hash de bloques |
| `K8sCopyBackend` | Kubernetes fallback |

Así los agentes usan `sandbox_sync`, y Bastion elige estrategia.

---

## Alto rendimiento sin romper abstracciones

### 1. Resolver primero, instalar después

El mayor ahorro está en evitar trabajo:

1. ¿El sandbox ya tiene la capability?
2. ¿El template ya la tiene?
3. ¿El node/provider tiene layer cacheado?
4. ¿Existe content-addressed toolchain?
5. Solo entonces usar package/version manager.

### 2. Cache local por provider/node

```
/var/lib/bastion/cache/
  toolchains/
    sha256-abc-jvm17-maven39/
  workspaces/
    sha256-blocks/
  workers/
    bastion-worker-sha256...
```

### 3. Verificación barata

No hacer `mvn -version` siempre si ya se verificó el hash del layer. Hacer:

- Hash verification para layers.
- Smoke test solo en primera preparación.
- Cache de verification result.

### 4. Streaming progresivo

Instalaciones y builds deben emitir eventos gRPC:

- `toolchain_resolve_started`
- `cache_hit`
- `layer_mount`
- `manager_install_started`
- `verification_passed`

Los agentes necesitan progreso legible, no solo un timeout de 10 minutos.

---

## Seguridad

### Principios

1. **No ejecutar scripts remotos sin política**.
2. **Preferir artifacts por hash** sobre installers dinámicos.
3. **Tool managers bajo allowlist**.
4. **Entorno explícito**, no shell magic.
5. **Auditoría de cada toolchain plan**.

### ToolchainPlan firmado

Cada plan debe ser serializable y auditable:

```json
{
  "capability": "jvm-build",
  "steps": [
    {"type": "cache_lookup", "hash": "sha256:..."},
    {"type": "mount_layer", "readonly": true},
    {"type": "verify", "cmd": "java -version"}
  ],
  "network": ["repo.maven.apache.org", "github.com"],
  "writes": ["/workspace", "/tmp"],
  "signature": "..."
}
```

### Policy engine

```rust
pub trait ToolchainPolicyEngine {
    fn authorize(&self, plan: &ToolchainPlan, user: &UserContext) -> PolicyDecision;
}
```

Políticas útiles:

- No permitir `curl | bash` salvo explicit allow.
- Solo dominios allowlisted.
- Solo managers permitidos por template.
- Forzar readonly layers.
- TTL para caches.

---

## Diseño útil para agentes de IA

### API de alto nivel

```python
sandbox_prepare(
  sandbox_id,
  capability="jvm-build",
  constraints={"java": "17", "maven": ">=3.8"},
  strategy="auto"
)
```

Respuesta:

```json
{
  "status": "ready",
  "capabilities": ["java", "maven", "git"],
  "env": {
    "JAVA_HOME": "/opt/bastion/toolchains/jvm/java17",
    "PATH_PREFIX": "/opt/bastion/toolchains/jvm/java17/bin:/opt/bastion/toolchains/maven/bin"
  },
  "cache": {
    "toolchain": "hit",
    "workspace": "miss"
  },
  "verification": [
    {"cmd": "java -version", "status": "passed"},
    {"cmd": "mvn -version", "status": "passed"}
  ]
}
```

### API de sync

```python
sandbox_sync(
  sandbox_id,
  direction="push",
  source="./repo",
  target="/workspace/repo",
  mode="auto",          # rsync | tarstream | contenthash
  exclude=["target", "node_modules", ".git"]
)
```

### API de build

```python
sandbox_run(
  sandbox_id,
  command="mvn clean package -DskipTests",
  cwd="/workspace/repo",
  env_ref="prepared:jvm-build"
)
```

El agente no razona sobre `.bashrc`, shims, `JAVA_HOME`, ca-certificates o
rsync flags.

---

## Plan de implementación incremental

### Fase 1 — Capabilities y entorno explícito

- Añadir `ToolchainRequest`, `ToolchainPlan`, `PreparedEnvironment`.
- Añadir MCP tool `sandbox_prepare`.
- Implementar `AptAdapter` para `jvm-build`.
- Worker ejecuta comandos con `env_ref`.

**Valor:** elimina bugs de shell/shims y simplifica para agentes.

### Fase 2 — DeltaSync abstraction

- Añadir `sandbox_sync`.
- Backend inicial: `TarStreamBackend` vía gRPC chunks.
- Backend opcional local: `RsyncBackend` para Podman/Docker.

**Valor:** workflows reales de repos grandes sin copiar todo.

### Fase 3 — Toolchain cache content-addressed

- Cache local de toolchains.
- Hash + manifest.
- Reuso entre sandboxes locales.

**Valor:** alto rendimiento sin depender de prebuilt images.

### Fase 4 — Adapters avanzados

- `SdkmanAdapter` para JVM.
- `AsdfAdapter` con env explícito.
- `BrewAdapter` opcional.
- `NixAdapter` experimental.

**Valor:** flexibilidad sin meter lógica ad-hoc.

### Fase 5 — Templates/snapshots

- Extender `create_snapshot` / `restore_snapshot`.
- Templates con capabilities declaradas.

**Valor:** cold starts rápidos por provider.

---

## Recomendación final

No empezar por COW ni por rsync. Empezar por **capabilities + toolchain plan +
env explícito**.

Razón:

- Es útil inmediatamente.
- Respeta todas las tecnologías de worker.
- Reduce bugs reales detectados en la prueba.
- Permite después optimizar con cache, rsync, layers, snapshots sin cambiar la
  API de usuario ni la API del agente.

La abstracción correcta no es “instalar Java”, sino:

> “Preparar un sandbox para capacidad `jvm-build` bajo política X, devolviendo
> un entorno verificado y reusable”.
