# Propuestas de Mejora - Análisis Profundo v3

## Corrections

### Worker Injection: No es Uniforme

La inyección del worker binary varía según el provider:

| Provider | Worker Injection | Método |
|----------|-----------------|--------|
| **Podman** | bind-mount | Worker binario se monta en `/usr/local/bin/bastion-worker` |
| **Docker** | bind-mount | Mismo que Podman |
| **Firecracker** | HTTP fetch | Worker se baja via HTTP después del boot del microVM |
| **GVisor** | sidecar | Worker corre como proceso sidecar en el mismo namespace |
| **Kubernetes** | init container | Worker se copia via `kubectl cp` antes del container main |

**Implicación**: TLI no puede asumir bind-mount. Debe ser abstracto.

### Tool Managers: No es Solo "Instalar Binarios"

Hay varias categorías de tool management:

```
┌────────────────────────────────────────────────────────────────┐
│  Tool Management Systems                                         │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐           │
│  │ Version     │  │ Package     │  │ System      │           │
│  │ Managers    │  │ Managers    │  │ Package Mgr │           │
│  ├─────────────┤  ├─────────────┤  ├─────────────┤           │
│  │ asdf        │  │ apt/yum    │  │ brew        │           │
│  │ sdk-man     │  │ dnf        │  │ conda       │           │
│  │ nix         │  │ apk        │  │ pip/npm     │           │
│  │ rtx          │  │ pacman    │  │ cargo       │           │
│  └─────────────┘  └─────────────┘  └─────────────┘           │
│                                                                 │
│  Cada uno:                                                      │
│  • Instala binarios en ubicaciones específicas                │
│  • Maneja symlinks y shims                                    │
│  • Tiene su propio registry de versions                      │
│  • Gestiona dependencies entre tools                         │
└────────────────────────────────────────────────────────────────┘
```

---

## RSync: Análisis Profundo

### Qué es RSync

```
rsync -avz --delete source/ destination/

Opciones clave:
-a: archive mode (preserve permissions, times, etc)
-v: verbose
-z: compress during transfer
--delete: delete extraneous files from destination
--partial: resume interrupted transfers
--progress: show transfer progress
```

### RSync para Bastion: Pros y Contras

#### Pros ✅

| Aspecto | Evaluación |
|---------|-----------|
| **Delta transfers** | Solo transfiere cambios, no archivos completos |
| **Compression** | Reduce ancho de banda |
| **Resume support** | `--partial` permite continuar transferencias interrumpidas |
| **Permissions** | Preserva ownership y permissions |
| **Delete sync** | `--delete` mantiene destino idéntico a origen |

#### Contras ❌

| Aspecto | Problema |
|---------|----------|
| **Requiere SSH** | Para remote sync necesita daemon SSH corriendo |
| **No content-addressable** | RSync compara por timestamp/size, no por hash |
| **Metadata overhead** | Para archivos pequeños, el overhead de comparación es alto |
| **No deduplication** | Si 10 sandboxes tienen mismo tool, RSync lo transfiere 10 veces |
| **Latency** | Handshake SSH + rsync protocol = ~100-200ms overhead mínimo |
| **One-way** | RSync es push o pull, no bidirectional real-time |

### Cuándo RSync ES Útil

```
┌────────────────────────────────────────────────────────────────┐
│  CASOS DE USO DONDE RSYNC APORTA VALOR                          │
│                                                                 │
│  1. Sync de código fuente entre host y sandbox                 │
│     • El agente edita código en el host                        │
│     • RSync empuja cambios al sandbox                          │
│     • Solo diffs se transfieren                               │
│                                                                 │
│  2. Backup de archivos del sandbox al host                     │
│     • Resultados, logs, artifacts                               │
│     • Transferencia incremental                                  │
│                                                                 │
│  3. Sincronización de configuración                            │
│     • dotfiles, .bashrc, etc                                  │
│     • Pequños cambios frecuentes                               │
└────────────────────────────────────────────────────────────────┘
```

### Cuándo RSync NO ES Útil

```
┌────────────────────────────────────────────────────────────────┐
│  CASOS DE USO DONDE RSYNC NO APORTA                            │
│                                                                 │
│  1. Tool installation desde Tool Store                          │
│     • Content-addressable: si hash existe, no transferir        │
│     • Hard links son más eficientes que rsync                    │
│                                                                 │
│  2. Worker binary deployment                                    │
│     • El worker ya está en el container/VM                      │
│     • No necesita sync, es parte de la imagen                  │
│                                                                 │
│  3. Sandboxes between themselves                              │
│     • No hay comunicación directa sandbox-to-sandbox            │
│     • Todo pasa por el gateway                                 │
└────────────────────────────────────────────────────────────────┘
```

### Decisión sobre RSync

**Para Tool Layer Injection**: NO usar RSync
- Hard links + content-addressable store es más eficiente
- No hay overhead de SSH/daemon
- Deduplicación nativa

**Para File Sync (código fuente)**: SÍ considerar RSync
- Implementar como tool `sandbox_sync` que usa rsync internamente
- Útil para workflows donde el agente edita en host y ejecuta en sandbox

---

## TLI: Abstracción para Tool Managers

### Tool Manager Abstraction

```
┌────────────────────────────────────────────────────────────────┐
│  ToolManager Trait                                              │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │ trait ToolManager {                                     │  │
│  │     fn name(&self) -> &str;                           │  │
│  │     async fn install(&self, spec: &ToolSpec)           │  │
│  │         -> Result<ToolInstallation, Error>;              │  │
│  │     async fn uninstall(&self, tool: &str) -> Result;   │  │
│  │     async fn list_installed(&self) -> Vec<Installed>;   │  │
│  │     fn shim_path(&self, tool: &str) -> PathBuf;      │  │
│  │ }                                                      │  │
│  └─────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

### Implementaciones

```rust
// 1. NullToolManager - no-op para tools que ya están en la imagen
struct NullToolManager;

impl ToolManager for NullToolManager {
    fn name(&self) -> &str { "null" }

    async fn install(&self, spec: &ToolSpec) -> Result<ToolInstallation, Error> {
        // Tool ya está en el base image, solo verificar que existe
        let path = PathBuf::from(&spec.install_path).join("bin").join(&spec.name);
        if !path.exists() {
            return Err(Error::ToolNotFound(spec.name.clone()));
        }
        Ok(ToolInstallation { path, verified: true })
    }
}

// 2. SystemPackageManager - apt, yum, dnf, apk
struct SystemPackageManager {
    backend: SystemPackageBackend,
}

impl SystemPackageManager {
    fn new(backend: SystemPackageBackend) -> Self {
        Self { backend }
    }
}

impl ToolManager for SystemPackageManager {
    fn name(&self) -> &str {
        match self.backend {
            SystemPackageBackend::Apt => "apt",
            SystemPackageBackend::Dnf => "dnf",
            SystemPackageBackend::Apk => "apk",
            _ => "unknown",
        }
    }

    async fn install(&self, spec: &ToolSpec) -> Result<ToolInstallation, Error> {
        match self.backend {
            SystemPackageBackend::Apt => {
                let output = Command::new("apt-get")
                    .args(["install", "-y", &spec.name])
                    .output()
                    .await?;
                // ... verificar instalación
            }
            // ...
        }
    }
}

// 3. VersionManager - asdf, sdk-man, rtx
struct VersionManagerAdapter {
    manager: VersionManagerType,
    install_root: PathBuf,
}

enum VersionManagerType {
    Asdf,
    SdkMan,
    Rtx,
}

impl VersionManagerAdapter {
    fn install_java(&self, version: &str) -> Result<ToolInstallation> {
        match self.manager {
            VersionManagerType::Asdf => {
                // asdf plugin add java
                // asdf install java <version>
                // asdf global java <version>
            }
            VersionManagerType::SdkMan => {
                // sdk install java <version>
            }
            VersionManagerType::Rtx => {
                // rtx install java@<version>
            }
        }
    }
}

impl ToolManager for VersionManagerAdapter {
    fn shim_path(&self, tool: &str) -> PathBuf {
        match self.manager {
            VersionManagerType::Asdf => self.install_root.join(".asdf/shims").join(tool),
            VersionManagerType::SdkMan => self.install_root.join(".sdkman/shims").join(tool),
            VersionManagerType::Rtx => self.install_root.join(".local/shims").join(tool),
        }
    }
}

// 4. ContentAddressableToolManager - para tools pre-descargados
struct ContentAddressableToolManager {
    store_path: PathBuf,
}

impl ToolManager for ContentAddressableToolManager {
    async fn install(&self, spec: &ToolSpec) -> Result<ToolInstallation, Error> {
        let hash = &spec.content_hash;
        let archive_path = self.store_path.join(format!("{}.tar.gz", hash));

        if !archive_path.exists() {
            // Descargar desde Tool Store
            download_tool(&spec.download_url, &archive_path).await?;
        }

        // Verificar hash
        verify_hash(&archive_path, hash)?;

        // Extraer a ubicación de instalación
        let install_path = PathBuf::from(&spec.install_path);
        extract_to(&archive_path, &install_path)?;

        // Crear hard link o symlink en ubicación estándar
        let bin_path = install_path.join("bin").join(&spec.name);
        std::fs::hard_link(&bin_path, &self.store_path.join(hash).join("bin").join(&spec.name))?;

        Ok(ToolInstallation {
            path: bin_path,
            verified: true,
        })
    }
}
```

### ToolResolver: Selecciona el Manager Apropiado

```rust
struct ToolResolver {
    managers: Vec<Arc<dyn ToolManager>>,
    ca_store: Arc<ContentAddressableToolManager>,
}

impl ToolResolver {
    async fn resolve_and_install(&self, spec: &ToolSpec) -> Result<ToolInstallation> {
        // 1. Si el tool ya está instalado (cualquier manager), devolver
        for manager in &self.managers {
            if let Ok(installed) = manager.list_installed().await {
                if installed.iter().any(|i| i.name == spec.name && i.version == spec.version) {
                    return Ok(manager.shim_path(&spec.name));
                }
            }
        }

        // 2. Si tenemos el tool en CA store, usar ContentAddressableToolManager
        if let Some(hash) = self.find_in_ca_store(spec) {
            return self.ca_store.install(&spec).await;
        }

        // 3. Intentar system package manager
        if let Some(pm) = self.system_package_manager_for(spec) {
            return pm.install(spec).await;
        }

        // 4. Intentar version managers
        if let Some(vm) = self.version_manager_for(spec) {
            return vm.install(spec).await;
        }

        Err(Error::ToolNotSupported(spec.name.clone()))
    }

    fn version_manager_for(&self, spec: &ToolSpec) -> Option<Arc<dyn ToolManager>> {
        // Detectar qué version manager puede manejar este tool
        match spec.category {
            Category::Java => Some(Arc::new(VersionManagerAdapter {
                manager: VersionManagerType::Asdf,
                install_root: PathBuf::from("/root/.asdf"),
            })),
            Category::Node => Some(Arc::new(VersionManagerAdapter {
                manager: VersionManagerType::Asdf,
                install_root: PathBuf::from("/root/.asdf"),
            })),
            Category::Python => Some(Arc::new(VersionManagerAdapter {
                manager: VersionManagerType::Rtx,
                install_root: PathBuf::from("/root/.local"),
            })),
            _ => None,
        }
    }
}
```

### Integration con SandboxProvider

```rust
// El SandboxProvider recibe ToolSpecs y usa ToolResolver
#[async_trait]
impl ToolLayerInjection for PodmanProvider {
    async fn inject_tools(
        &self,
        id: &SandboxId,
        tools: &[ToolSpec],
    ) -> Result<(), DomainError> {
        let resolver = ToolResolver::new(
            self.tool_store.clone(),
            self.managers.clone(),
        );

        for tool_spec in tools {
            match resolver.resolve_and_install(tool_spec).await {
                Ok(installation) => {
                    // Montar el tool en el sandbox via overlay o bind mount
                    self.mount_tool(id, &installation).await?;
                }
                Err(e) => {
                    tracing::warn!("Failed to install tool {}: {}", tool_spec.name, e);
                    // Continuar con otros tools, no fallar toda la operación
                }
            }
        }
        Ok(())
    }
}
```

### Tool Spec Estructura

```rust
pub struct ToolSpec {
    pub name: String,                    // "java", "maven", "node"
    pub version: String,                  // "17.0.8", "3.9.5", "20.0.0"
    pub category: Category,               // Java, Node, Python, System
    pub manager_preference: Vec<ManagerType>, // ["asdf", "apt", "ca-store"]

    // Para ContentAddressable store
    pub content_hash: Option<String>,
    pub download_url: Option<String>,

    // Para installation paths
    pub install_path: String,            // "/usr/local", "/root/.asdf"
    pub bin_path: Option<String>,         // "bin/java" relative to install_path

    // Metadata
    pub verify_checksum: bool,
    pub extract_archive: bool,
}

pub enum Category {
    Java,
    Node,
    Python,
    Ruby,
    Go,
    Rust,
    System,
    Generic,
}

pub enum ManagerType {
    Asdf,
    SdkMan,
    Rtx,
    Nix,
    Apt,
    Dnf,
    Apk,
    Brew,
    Pip,
    Npm,
    Cargo,
    CaStore,  // Content-Addressable Store
}
```

---

## Propuesta Integrada: TLI v2

### Arquitectura

```
┌────────────────────────────────────────────────────────────────┐
│  Tool Layer Injection (TLI) v2                                  │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ ToolResolver                                            │  │
│  │  1. Check if already installed (any manager)          │  │
│  │  2. Try ContentAddressableStore                        │  │
│  │  3. Try SystemPackageManager (apt, dnf, apk)         │  │
│  │  4. Try VersionManagers (asdf, sdkman, rtx)          │  │
│  │  5. Return installation path or error                 │  │
│  └─────────────────────┬────────────────────────────────────┘  │
│                        │                                           │
│                        ▼                                           │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ ToolInstaller                                           │  │
│  │  • ContentAddressableToolManager (hard links)          │  │
│  │  • SystemPackageManager (apt-get, apk, etc)            │  │
│  │  • VersionManagerAdapter (asdf, sdkman, rtx, nix)    │  │
│  │  • NullToolManager (tools already in base image)      │  │
│  └─────────────────────┬────────────────────────────────────┘  │
│                        │                                           │
│                        ▼                                           │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ ToolMounter                                              │  │
│  │  • Mount into sandbox filesystem                        │  │
│  │  • Via overlayfs (layered approach)                   │  │
│  │  • Via bind mount (simple approach)                    │  │
│  │  • Verify permissions and integrity                    │  │
│  └──────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

### Provider-Specific Tool Mounting

```rust
// Tool mounting strategy depends on provider capabilities
#[async_trait]
trait ToolMounting {
    async fn mount_tool(
        &self,
        sandbox_id: &SandboxId,
        installation: &ToolInstallation,
    ) -> Result<(), Error>;

    async fn unmount_tool(
        &self,
        sandbox_id: &SandboxId,
        tool_name: &str,
    ) -> Result<(), Error>;
}

// Podman: usa overlayfs o bind mount
struct PodmanToolMount;

#[async_trait]
impl ToolMounting for PodmanToolMount {
    async fn mount_tool(
        &self,
        sandbox_id: &SandboxId,
        installation: &ToolInstallation,
    ) -> Result<(), Error> {
        // Obtener path del tool
        let tool_path = &installation.path;

        // Para ContentAddressable tools: crear bind mount readonly
        if installation.source == Source::CaStore {
            self.podman.exec(sandbox_id, |container| {
                // podman mount <container>
                let rootfs = podman.get_rootfs(container.id)?;
                let target = format!("{}/{}", rootfs, tool_path);

                // bind mount readonly
                self.bind_mount_readonly(source, target)?;
            }).await?;
        }

        // Para system packages: no mount necesario, ya están en el FS
        Ok(())
    }
}

// Firecracker: herramientas en la imagen del microVM
// No se "montan" en runtime - ya están en la imagen
struct FirecrackerToolMount;

#[async_trait]
impl ToolMounting for FirecrackerToolMount {
    async fn mount_tool(
        &self,
        _sandbox_id: &SandboxId,
        installation: &ToolInstallation,
    ) -> Result<(), Error> {
        // En Firecracker, el tool ya está en la imagen del microVM
        // No hay nada que montar en runtime
        // La verificación es que el path existe
        if !installation.path.exists() {
            return Err(Error::ToolNotFound(installation.name.clone()));
        }
        Ok(())
    }
}
```

### Para Agentes IA

```python
# El agente especifica tools que necesita
sandbox_create(
    template="debian:bookworm-slim",
    tools=[
        ToolSpec(name="java", version="17.0.8", manager_preference=["asdf", "apt", "ca-store"]),
        ToolSpec(name="maven", version="3.9.5", manager_preference=["asdf", "apt", "ca-store"]),
        ToolSpec(name="node", version="20", manager_preference=["nvm", "apt", "ca-store"]),
    ]
)

# El ToolResolver automáticamente:
# 1. Para Java 17: intenta asdf primero, si falla apt, si falla CA store
# 2. Para Maven 3.9.5: mismo proceso
# 3. Los tools se montan en el sandbox según el provider
```

### Métricas

| Tool | Manager usado | Tiempo |
|------|--------------|--------|
| java-17 | asdf (primero) | ~30s primera vez, ~2s siguientes (cacheado) |
| maven-3.9 | apt | ~20s |
| node-20 | nvm | ~45s |
| python-3.11 | rtx | ~15s |

---

## File Sync Tool: RSync Wrapper

### Para Qué Sí Sirve

```
┌────────────────────────────────────────────────────────────────┐
│  sandbox_sync - Sync bidireccional entre host y sandbox           │
│                                                                 │
│  casos de uso:                                                  │
│  • Agente edita código en host (IDE, git)                      │
│  • Agente quiere que el código esté en el sandbox              │
│  • Sync incremental: solo cambios                               │
│                                                                 │
│  comando:                                                       │
│  sandbox_sync(sandbox_id, direction="push", local_path, remote_path)
│  sandbox_sync(sandbox_id, direction="pull", remote_path, local_path)
└────────────────────────────────────────────────────────────────┘
```

### Implementación

```rust
// MCP Tool: sandbox_sync
async fn sandbox_sync(
    sandbox_id: &str,
    direction: SyncDirection,
    source: &Path,
    destination: &Path,
    options: SyncOptions,
) -> Result<SyncResult, Error> {
    // 1. Verificar que el sandbox existe y está alive
    // 2. Ejecutar rsync dentro del sandbox (push) o desde el sandbox (pull)
    // 3. Retornar resultado con stats

    let rsync = Rsync::new()
        .archive(options.preserve_permissions)
        .compress(options.compress)
        .delete(options.delete_extraneous)
        .exclude(options.exclude.unwrap_or_default());

    match direction {
        SyncDirection::Push => {
            // rsync local -> sandbox
            let remote = format!("{}@sandbox:/{}", user, destination);
            rsync.source(source).destination(&remote).await?;
        }
        SyncDirection::Pull => {
            // rsync sandbox -> local
            let remote = format!("{}@sandbox:/{}", user, source);
            rsync.source(&remote).destination(destination).await?;
        }
    }
}
```

### No Es Para Tools

**RSync NO se usa para tool installation** porque:
- Overhead de SSH/daemon
- No hay deduplicación entre sandboxes
- Content-addressable con hard links es más eficiente

---

## Resumen: TLI v2 + RSync

### Propuestas Actualizadas

| # | Nombre | Descripción | Prioridad |
|---|--------|-------------|-----------|
| 001 | **TLI v2** | Tool Layer Injection con abstracción ToolManager (asdf, apt, brew, ca-store) | Alta |
| 002 | **ToolResolver** | Selecciona el manager apropiado por tool | Alta |
| 003 | **RSync Wrapper** | Sync de archivos host↔sandbox (NO para tools) | Media |
| 004 | **Template Cloning** | Snapshots pre-preparados con COW | Media |
| 005 | **Streaming Diags** | Unified logging via gRPC | Baja |

### Compatibilidad con Providers

| Feature | Podman | Firecracker | GVisor | K8s |
|---------|--------|-------------|--------|-----|
| TLI v2 | ✅ bind mount | ✅ baked-in image | ✅ sidecar | ✅ init container |
| ToolResolver | ✅ | ✅ | ✅ | ✅ |
| RSync Wrapper | ✅ exec | ✅ exec | ✅ exec | ⚠️ kubectl exec |
| Template Cloning | ✅ overlay COW | ✅ snapshot | ⚠️ limited | ⚠️ PVC |
