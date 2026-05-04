# Propuesta 004 — TemplateArtifacts cross-provider

## Objetivo

Impulsar templates, imágenes y layers preconfigurados sin acoplar Bastion a una
tecnología concreta. La misma capability (`jvm-build`, `node-build`,
`python-test`, etc.) debe poder materializarse en:

- Contenedores: Docker, Podman, containerd, Kubernetes, gVisor.
- Máquinas virtuales tradicionales.
- MicroVMs: Firecracker.
- FaaS / Lambda-like functions.

Manteniendo:

- Worker homogéneo vía gRPC.
- Alto rendimiento.
- Zero-cost cuando el provider lo permita.
- Seguridad por hashes, firmas, policy y aislamiento.
- APIs simples para agentes IA.

---

## Investigación resumida

### OCI containers

OCI images se componen de manifest, config y capas filesystem content-addressed.
Las capas se identifican por digest y son reutilizables entre imágenes.

**Implicación para Bastion**: OCI es un excelente formato común para almacenar
toolchains y templates, incluso si luego se materializan como container image,
tar rootfs, layer para VM o artifact genérico.

### Lazy-pull / remote snapshotters

Stargz snapshotter y enfoques similares permiten arrancar un container sin bajar
toda la imagen: se montan capas remotas y el contenido se descarga bajo demanda.

**Implicación para Bastion**: para toolchains grandes, el objetivo no debe ser
“copiar todo al sandbox”, sino permitir acceso lazy/cached cuando el provider lo
soporte.

### Firecracker snapshots

Firecracker puede restaurar microVMs desde snapshot. La memoria se mapea con
`MAP_PRIVATE` y carga páginas bajo demanda, con COW en escrituras. Pero los
snapshots tienen consideraciones de seguridad: estado único, entropy, tokens y
conexiones no deben clonarse sin reset.

**Implicación para Bastion**: snapshots de microVM son útiles para cold start,
pero deben ser “sanitized snapshots”: sin secretos, sin sesiones vivas, con
regeneración de identidad del worker al reanudar.

### Lambda layers

AWS Lambda layers son zip archives inmutables versionados que se extraen en
`/opt`; permiten compartir dependencias y separar código de runtime/deps.
Hay límite de layers por función, y para Go/Rust AWS recomienda incluir deps en
el binario para rendimiento.

**Implicación para Bastion**: para FaaS, el equivalente natural de un template es
un conjunto de layers montados/extraídos en `/opt/bastion/...`, más env explícito.

---

## Principio central

No modelar “Docker image”, “VM snapshot” o “Lambda layer” directamente. Modelar:

> **TemplateArtifact**: un artifact versionado, verificable y materializable que
> proporciona capabilities.

Cada provider implementa cómo materializarlo.

---

## TemplateArtifact

### Modelo lógico

```rust
pub struct TemplateArtifact {
    pub id: ArtifactId,
    pub name: String,
    pub version: String,
    pub digest: String,
    pub media_type: ArtifactMediaType,
    pub capabilities: Vec<CapabilityDescriptor>,
    pub env: PreparedEnvironmentSpec,
    pub entrypoints: Vec<EntryPointSpec>,
    pub security: ArtifactSecurityMetadata,
    pub provider_hints: Vec<ProviderHint>,
}

pub enum ArtifactMediaType {
    OciImage,
    OciLayer,
    OciArtifact,
    RootfsTar,
    VmDisk,
    MicroVmSnapshot,
    LambdaLayerZip,
    WasmModule,
}

pub struct CapabilityDescriptor {
    pub name: String,              // "jvm-build"
    pub tools: Vec<ToolDescriptor>,
    pub verification: Vec<VerificationStep>,
}
```

### Ejemplo: JVM build template

```json
{
  "name": "bastion/jvm-build",
  "version": "java17-maven3.9-v1",
  "digest": "sha256:...",
  "media_type": "oci_artifact",
  "capabilities": [
    {
      "name": "jvm-build",
      "tools": [
        {"name": "java", "version": "17"},
        {"name": "maven", "version": "3.9"},
        {"name": "git", "version": "any"}
      ]
    }
  ],
  "env": {
    "JAVA_HOME": "/opt/bastion/toolchains/jvm/java17",
    "PATH_PREFIX": "/opt/bastion/toolchains/jvm/java17/bin:/opt/bastion/toolchains/maven/bin"
  },
  "security": {
    "signed": true,
    "sbom": true,
    "readonly": true
  }
}
```

---

## Materialización por provider

### ProviderMaterializer trait

```rust
#[async_trait]
pub trait ProviderMaterializer: Send + Sync {
    fn provider_kind(&self) -> ProviderKind;

    async fn can_materialize(
        &self,
        artifact: &TemplateArtifact,
    ) -> MaterializationSupport;

    async fn materialize(
        &self,
        sandbox: &SandboxId,
        artifact: &TemplateArtifact,
        mode: MaterializationMode,
    ) -> Result<MaterializedArtifact, DomainError>;
}

pub enum MaterializationMode {
    Auto,
    MountReadonly,
    Extract,
    BakeIntoImage,
    RestoreSnapshot,
    AttachLayer,
    LazyRemote,
}
```

### Tabla por provider

| Provider | Mejor materialización | Fallback universal |
|----------|-----------------------|--------------------|
| Podman/Docker | OCI layer readonly / bind mount / overlay | tar extract vía worker |
| containerd/K8s | OCI image/layer + snapshotter lazy-pull | initContainer extract |
| gVisor | OCI image/layer compatible | extract vía worker |
| Firecracker | rootfs layer o sanitized microVM snapshot | HTTP fetch + extract dentro VM |
| VM tradicional | disk image layer / cloud-init artifact | fetch + extract |
| Lambda/FaaS | Lambda-like layer zip en `/opt` | include in deployment package |

---

## StrategyResolver

### Objetivo

Elegir automáticamente la forma más eficiente y segura para cada provider.

```rust
pub struct MaterializationStrategyResolver;

impl MaterializationStrategyResolver {
    pub fn choose(
        provider: ProviderKind,
        artifact: &TemplateArtifact,
        node_caps: &NodeCapabilities,
        policy: &Policy,
    ) -> MaterializationMode {
        // 1. Preferir readonly/lazy si está disponible.
        // 2. Preferir cache local por digest.
        // 3. Evitar extract si hay mount/lazy.
        // 4. Evitar snapshots si policy exige fresh entropy.
        // 5. Fallback: tar/zip extract vía worker gRPC.
    }
}
```

### Orden general de preferencia

1. **Already present**: capability ya está en template/base.
2. **Readonly local mount**: zero-copy local.
3. **Lazy remote layer**: stargz/nydus/soci-like si está soportado.
4. **Provider-native layer**: Lambda layer, K8s volume, VM disk attach.
5. **Snapshot restore**: solo si es sanitized y compatible.
6. **Extract via worker**: fallback universal.
7. **Install via package manager**: último recurso.

---

## FaaS / Lambda-like model

### Problema

FaaS no tiene necesariamente filesystem persistente completo ni proceso worker
vivo como un container tradicional. Pero sí suele tener:

- Deployment package.
- Layers inmutables.
- `/tmp` temporal.
- `/opt` para deps/layers.
- Cold start + warm reuse.

### Abstracción propuesta

```rust
pub struct FunctionLayerArtifact {
    pub layer_name: String,
    pub version: String,
    pub digest: String,
    pub mount_path: String, // typically /opt/bastion/...
    pub env: HashMap<String, String>,
}
```

### Aplicación

Para un provider FaaS:

1. `TemplateArtifact` se empaqueta como zip layer.
2. Se publica como layer versionado.
3. La función Bastion worker/runtime referencia la layer.
4. En cold start, el runtime ve `/opt/bastion/...`.
5. `PreparedEnvironment` apunta a esos paths.

### Seguridad FaaS

- Layers inmutables y versionadas.
- Digest verificado antes de publicar.
- No meter secretos en layer.
- No asumir persistencia más allá de `/tmp`.

### Performance FaaS

- Bueno para dependencias comunes.
- Malo para binarios Go/Rust si fuerza carga dinámica innecesaria; mejor baked
  en deployment package cuando sea executable estático.

---

## Templates vs Toolchains vs Workspaces

Separar tres tipos de artifact evita confusión:

| Tipo | Qué contiene | Mutabilidad | Ejemplo |
|------|--------------|------------|---------|
| TemplateArtifact | OS/runtime/tools base | Inmutable | `jvm-build` layer |
| ToolchainArtifact | Solo tools + env | Inmutable | Java+Maven |
| WorkspaceArtifact | Código usuario/artifacts | Mutable | repo PetClinic |

Regla:

- Templates/toolchains: content-addressed, readonly, cacheables.
- Workspaces: delta sync, mutable, TTL corto.

RSync/DeltaSync aplica a **WorkspaceArtifact**, no a TemplateArtifact.

---

## Seguridad

### ArtifactSecurityMetadata

```rust
pub struct ArtifactSecurityMetadata {
    pub digest: String,
    pub signature: Option<String>,
    pub sbom_ref: Option<String>,
    pub provenance_ref: Option<String>,
    pub readonly: bool,
    pub allowed_network: Vec<String>,
    pub allowed_writes: Vec<String>,
    pub contains_secrets: bool,
}
```

### Políticas

1. No materializar artifact sin digest.
2. Preferir artifacts firmados.
3. Bloquear artifacts con `contains_secrets=true` como template compartido.
4. Snapshots de VM deben ser sanitized:
   - sin tokens de sesión,
   - sin conexiones vivas,
   - worker identity regenerada,
   - entropy reseeded.
5. Layers readonly por defecto.

---

## Alto rendimiento / zero-cost por tecnología

### Contenedores locales

Mejor caso:

- OCI layers.
- OverlayFS.
- Bind mount readonly desde cache content-addressed.
- Page cache comparte binarios entre sandboxes.

### Kubernetes/containerd

Mejor caso:

- OCI images/artifacts.
- Lazy-pull snapshotters si disponibles.
- Node-local cache por digest.
- InitContainer solo como fallback.

### Firecracker

Mejor caso:

- Rootfs prebuilt por capability.
- Snapshots sanitized para cold start.
- Memory snapshot MAP_PRIVATE permite page-fault lazy + COW.

Riesgo:

- Snapshot reuse puede duplicar estado único; hay que resetear worker/session.

### FaaS

Mejor caso:

- Layers versionadas para deps compartidas.
- Deployment package para ejecutables estáticos.
- `/tmp` cache warm oportunista, no garantía.

---

## API de alto nivel para agentes

### Crear sandbox con template/capability

```python
sandbox_create(
  provider="auto",
  capabilities=["jvm-build"],
  template_strategy="auto"
)
```

Respuesta:

```json
{
  "sandbox_id": "...",
  "provider": "podman",
  "materialization": {
    "capability": "jvm-build",
    "artifact": "bastion/jvm-build:java17-maven3.9-v1",
    "mode": "mount_readonly",
    "cache": "hit"
  },
  "env_ref": "prepared:jvm-build:sha256-..."
}
```

### Preparar capability en sandbox existente

```python
sandbox_prepare(
  sandbox_id,
  capability="jvm-build",
  strategy="auto"
)
```

---

## Plan incremental

### Fase A — Catálogo de artifacts

- Definir `TemplateArtifact`, `CapabilityDescriptor`, `ArtifactSecurityMetadata`.
- Guardar manifests locales en `config/templates/` o `~/.bastion/artifacts`.
- No materializar aún.

### Fase B — Materializer universal extract

- Implementar fallback universal:
  - artifact tar/zip,
  - enviar por worker gRPC,
  - extraer en `/opt/bastion/artifacts/<digest>`.
- Funciona en todos los providers.

### Fase C — Podman/Docker optimized materializer

- Bind mount readonly desde cache local.
- Overlay si disponible.
- Zero-copy vía page cache.

### Fase D — K8s/containerd materializer

- InitContainer extract fallback.
- OCI artifact pull por digest.
- Soporte futuro para lazy snapshotter si node lo tiene.

### Fase E — Firecracker materializer

- Rootfs prepared por capability.
- Snapshot sanitized experimental.
- Worker identity reset al restore.

### Fase F — FaaS layer materializer

- Empaquetar TemplateArtifact como zip layer.
- Montar en `/opt/bastion`.
- Resolver env explícito.

---

## Decisión clave

La abstracción unificadora debe ser **TemplateArtifact + Materializer**, no
“Docker image” ni “Lambda layer”.

Cada provider elige el mecanismo más barato:

- local container → mount/overlay,
- k8s → OCI/lazy/init,
- microVM → rootfs/snapshot,
- FaaS → layer zip,
- fallback universal → extract vía worker gRPC.

Así mantenemos API simple, seguridad centralizada y optimizaciones provider-
specific sin romper homogeneidad.
