# Propuesta 003 — Plan de implementación: Toolchains y DeltaSync

## Objetivos mantenidos

Este plan aterriza la propuesta 002 manteniendo los objetivos marcados:

1. **Útil para agentes IA y usuarios**: APIs de alto nivel (`sandbox_prepare`,
   `sandbox_sync`, `env_ref`) sin obligar a razonar sobre `.bashrc`, shims,
   ca-certificates o gestores concretos.
2. **Homogéneo**: todo se ejecuta por `bastion-worker` y gRPC; los providers
   solo adaptan entrega/montaje.
3. **Backend-agnostic**: Podman, Docker, Firecracker, gVisor y K8s comparten
   modelo lógico.
4. **Alto rendimiento**: resolver/cachear antes de instalar, transferir deltas,
   usar content addressing y evitar roundtrips.
5. **Zero-cost cuando sea posible**: mounts/overlays/hardlinks/cache local; si
   no es posible, fallback universal vía gRPC chunks.
6. **Seguro**: policy engine, allowlists, verificación, env explícito, auditoría.

---

## Resultado esperado para el usuario/agente

### API ideal final

```python
sandbox = sandbox_create(template="debian:bookworm-slim")

prep = sandbox_prepare(
  sandbox.id,
  capability="jvm-build",
  constraints={"java": "17", "maven": ">=3.8"},
  strategy="auto"
)

sandbox_sync(
  sandbox.id,
  direction="push",
  source="./spring-petclinic",
  target="/workspace/spring-petclinic",
  exclude=["target", ".git"]
)

sandbox_run(
  sandbox.id,
  command="mvn clean package -DskipTests",
  cwd="/workspace/spring-petclinic",
  env_ref=prep.env_ref
)
```

### Respuesta de `sandbox_prepare`

```json
{
  "status": "ready",
  "env_ref": "prepared:jvm-build:sha256-abc",
  "capabilities": ["java", "maven", "git"],
  "strategy_used": "apt",
  "cache": {"toolchain": "miss", "verification": "stored"},
  "env": {
    "JAVA_HOME": "/usr/lib/jvm/default-java",
    "PATH_PREFIX": "/usr/lib/jvm/default-java/bin:/usr/share/maven/bin"
  },
  "verification": [
    {"name": "java", "cmd": "java -version", "status": "passed"},
    {"name": "maven", "cmd": "mvn -version", "status": "passed"}
  ]
}
```

---

## Arquitectura a implementar

```text
MCP Tool: sandbox_prepare
      │
      ▼
Application Use Case: PrepareSandboxToolchain
      │
      ▼
ToolchainResolver
      ├─ CapabilityCatalog
      ├─ PolicyEngine
      ├─ ToolchainCache
      └─ ToolManagerAdapters
            ├─ AptAdapter        (fase 1)
            ├─ SdkmanAdapter     (fase 4)
            ├─ AsdfAdapter       (fase 4)
            └─ CaStoreAdapter    (fase 3)
      │
      ▼
Provider/Worker execution
      ├─ SandboxProvider::run_command / run_command_stream
      └─ Worker gRPC command execution
```

Para sync:

```text
MCP Tool: sandbox_sync
      │
      ▼
Application Use Case: SyncSandboxWorkspace
      │
      ▼
DeltaSyncProvider
      ├─ TarStreamBackend     (universal vía gRPC chunks, fase 2)
      ├─ RsyncBackend         (opcional local/remoto compatible, fase 2b)
      └─ ContentHashBackend   (futuro, fase 5)
```

---

## Fase 0 — Baseline y contratos

### Objetivo

Crear tipos y contratos sin cambiar comportamiento.

### Cambios

#### `bastion-domain`

Añadir módulo `toolchain`:

```rust
pub struct ToolchainRequest {
    pub sandbox_id: SandboxId,
    pub capability: String,
    pub constraints: HashMap<String, String>,
    pub strategy: ToolchainStrategy,
}

pub enum ToolchainStrategy {
    Auto,
    SystemPackage,
    VersionManager,
    ContentAddressed,
}

pub struct PreparedEnvironment {
    pub env_ref: String,
    pub env: HashMap<String, String>,
    pub path_prefix: Vec<String>,
    pub capabilities: Vec<String>,
    pub verification: Vec<VerificationResult>,
}

pub struct ToolchainPlan {
    pub id: String,
    pub capability: String,
    pub steps: Vec<ToolchainStep>,
    pub required_network: Vec<String>,
    pub writes: Vec<String>,
}
```

Añadir módulo `sync`:

```rust
pub struct SyncRequest {
    pub sandbox_id: SandboxId,
    pub direction: SyncDirection,
    pub source: String,
    pub target: String,
    pub excludes: Vec<String>,
    pub mode: SyncMode,
}

pub enum SyncMode {
    Auto,
    TarStream,
    Rsync,
    ContentHash,
}
```

### Criterio de éxito

- Compila.
- No cambia comportamiento actual.
- Tipos serializables con `serde`.

---

## Fase 1 — `sandbox_prepare` con AptAdapter y env explícito

### Objetivo

Resolver el caso real detectado: preparar `jvm-build` de forma fiable y usable.

### Alcance

Implementar solo:

- Capability: `jvm-build`
- Adapter: `AptAdapter`
- Tools: `default-jdk`, `maven`, `git`, `curl`, `ca-certificates`
- Env explícito: `JAVA_HOME`, `PATH_PREFIX`

### Componentes

#### `bastion-application`

Nuevo use case:

```rust
pub struct PrepareSandboxToolchain;

impl PrepareSandboxToolchain {
    pub async fn execute(
        provider: &dyn SandboxProvider,
        request: ToolchainRequest,
        resolver: &ToolchainResolver,
    ) -> Result<PreparedEnvironment, DomainError>;
}
```

#### `bastion-infrastructure`

`AptAdapter`:

```rust
pub struct AptAdapter;

#[async_trait]
impl ToolManagerAdapter for AptAdapter {
    fn supports(&self, req: &ToolchainRequest) -> SupportLevel;
    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolManagerPlan, DomainError>;
}
```

Plan para `jvm-build`:

```bash
apt-get update
apt-get install -y --no-install-recommends default-jdk maven git curl ca-certificates
java -version
mvn -version
git --version
```

#### `bastion-gateway`

Nueva MCP tool:

- `sandbox_prepare`

Input:

```json
{
  "sandbox_id": "...",
  "capability": "jvm-build",
  "constraints": {"java": "17", "maven": ">=3.8"},
  "strategy": "auto"
}
```

Output: `PreparedEnvironment`.

### Env explícito en `sandbox_run`

Extender `CommandSpec` para aceptar:

```rust
pub struct CommandSpec {
    pub command: String,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub env_ref: Option<String>,
    pub timeout_ms: Option<u64>,
}
```

`env_ref` referencia un entorno preparado guardado en repository/cache.

### Seguridad

- `AptAdapter` solo habilitado si policy permite network y package manager.
- Dominios implícitos: repos Debian.
- No `curl | bash`.
- No desactivar SSL verification salvo flag explícito de emergencia.

### Pruebas

1. Crear sandbox.
2. `sandbox_prepare(... jvm-build ...)`.
3. `sandbox_run("java -version", env_ref)`.
4. `sandbox_run("mvn -version", env_ref)`.
5. Descargar y compilar PetClinic.

### Criterio de éxito

- PetClinic compila igual que la prueba manual.
- El agente no necesita escribir comandos de setup.
- No depende de `.bashrc`.

---

## Fase 2 — `sandbox_sync` universal con TarStreamBackend

### Objetivo

Permitir sync de workspace/artifacts sin depender de rsync ni SSH.

### Por qué TarStream primero

- Funciona con cualquier provider porque usa worker/gRPC.
- No requiere daemon SSH.
- Seguridad y permisos bajo control de Bastion.
- Puede usar compresión.

### Diseño

#### MCP tool

- `sandbox_sync`

Input:

```json
{
  "sandbox_id": "...",
  "direction": "push",
  "source": "./repo",
  "target": "/workspace/repo",
  "mode": "auto",
  "exclude": ["target", "node_modules", ".git"]
}
```

#### Protocolo inicial

- Host crea tar stream comprimido.
- Gateway manda chunks al worker por gRPC.
- Worker extrae en target.
- Para pull, worker crea tar stream y gateway escribe local.

### Seguridad

- Validar paths destino.
- Bloquear path traversal (`../`, symlinks peligrosos).
- Tamaño máximo configurable.
- Excludes por defecto: `.git`, `target`, `node_modules`, `.venv` si no se pide.

### Criterio de éxito

- Push de repo pequeño.
- Pull de `target/*.jar`.
- Works con Podman sin SSH.

---

## Fase 2b — RsyncBackend opcional

### Objetivo

Añadir rsync donde tenga sentido sin hacerlo dependencia central.

### Cuándo usarlo

- Provider local o remoto con canal compatible.
- Repos grandes con cambios pequeños.
- Usuario lo habilita o `mode=auto` detecta beneficio.

### No usarlo para

- Toolchains compartidas.
- Worker delivery.
- Templates.

### Criterio de decisión en runtime

```text
if mode == TarStream -> tar
if mode == Rsync -> rsync or error
if mode == Auto:
  if rsync_available && workspace_large && previous_sync_exists -> rsync
  else -> tarstream
```

### Seguridad

- No abrir rsync daemon por defecto.
- Preferir ejecución controlada por worker.
- Logs de archivos transferidos y bytes.

---

## Fase 3 — Toolchain cache content-addressed

### Objetivo

Evitar instalaciones repetidas.

### Diseño mínimo

Cache local por provider/node:

```text
/var/lib/bastion/cache/toolchains/
  jvm-build/
    sha256-abc/
      manifest.json
      rootfs.tar.zst
      verified.json
```

Manifest:

```json
{
  "capability": "jvm-build",
  "tools": [
    {"name": "java", "version": "17.0.19"},
    {"name": "maven", "version": "3.8.7"}
  ],
  "env": {...},
  "hash": "sha256:...",
  "created_at": "..."
}
```

### Flujo

1. `sandbox_prepare` consulta cache.
2. Si hit y policy permite, usa cached toolchain.
3. Si miss, usa adapter y luego materializa cache.

### Estrategias por provider

| Provider | Cache apply |
|----------|-------------|
| Podman/Docker | bind/overlay readonly si posible |
| Firecracker | attach/extract en rootfs si no baked |
| gVisor | copy/extract controlado |
| K8s | initContainer o volume cache |

### Seguridad

- Hash verification antes de usar.
- Manifest firmado a futuro.
- Cache TTL.

---

## Fase 4 — ToolManagerAdapters avanzados

### Objetivo

Soportar asdf, sdkman, brew/nix de forma controlada.

### Orden recomendado

1. `SdkmanAdapter` para JVM.
2. `AsdfAdapter` para Node/Python/Go/Ruby.
3. `BrewAdapter` para CLI tools.
4. `NixAdapter` experimental.

### Regla crítica

Cada adapter debe devolver `PreparedEnvironment` explícito.

No se aceptan soluciones que dependan de:

- `.bashrc`
- shell login
- shims invisibles sin path controlado

### Pruebas por adapter

- install
- verify
- run command with `env_ref`
- cache materialization
- uninstall/cleanup si aplica

---

## Fase 5 — Templates y snapshots con capabilities

### Objetivo

Optimizar cold start para toolchains frecuentes.

### Diseño

Extender snapshots para incluir metadata de capabilities:

```json
{
  "template": "debian-jvm-build",
  "base": "debian:bookworm-slim",
  "capabilities": ["jvm-build"],
  "env_refs": ["prepared:jvm-build:sha256-abc"],
  "verified": true
}
```

### Flujo

1. Crear sandbox base.
2. `sandbox_prepare(jvm-build)`.
3. `sandbox_snapshot(name="debian-jvm-build")`.
4. Nuevos sandboxes pueden crearse desde ese template.

### Criterio de éxito

- Crear sandbox jvm-build en menos de 5s.
- PetClinic build sin setup previo.

---

## Observabilidad necesaria desde el principio

Cada operación debe emitir eventos estructurados:

```json
{"event":"toolchain.resolve.start","capability":"jvm-build"}
{"event":"toolchain.cache.miss"}
{"event":"toolchain.adapter.selected","adapter":"apt"}
{"event":"toolchain.install.progress","step":"apt-get install"}
{"event":"toolchain.verify.pass","tool":"java"}
{"event":"sync.push.complete","bytes":123456,"files":42}
```

Inicialmente pueden ser logs normales; luego se migran a gRPC streaming.

---

## Riesgos y mitigaciones

| Riesgo | Mitigación |
|--------|------------|
| Package managers lentos | cache content-addressed + templates |
| Shell/shims inconsistentes | env explícito obligatorio |
| RSync abre superficie | backend opcional, no daemon por defecto |
| Providers remotos no soportan mount | fallback tar/extract vía worker gRPC |
| Drift de versiones | constraints + verification + manifest |
| Red externa no confiable | cache, allowlist, mirrors configurables |

---

## MVP recomendado

Implementar en este orden:

1. **Fase 1 completa**: `sandbox_prepare` + `jvm-build` con AptAdapter.
2. **Fase 2 parcial**: `sandbox_sync` con TarStream push/pull.
3. **Test E2E**: PetClinic usando solo APIs nuevas.

### Test E2E objetivo

```text
create sandbox
prepare jvm-build
sync petclinic repo
run mvn package with env_ref
pull target jar
terminate sandbox
```

### Success criteria MVP

- Agente no escribe comandos de instalación manual.
- No se toca `.bashrc`.
- Funciona por worker/gRPC.
- PetClinic compila.
- Artifacts se pueden traer al host.

---

## Decisión final

La primera implementación no debe intentar resolverlo todo con COW, rsync o
asdf. Debe crear la capa semántica correcta:

> capability → plan → policy → execution → verified environment

Una vez esa API existe, podemos optimizar internamente con cache, layers,
rsync, snapshots o managers avanzados sin cambiar la experiencia del agente.
