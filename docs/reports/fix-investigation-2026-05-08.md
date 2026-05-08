# Investigación de Fixes — Bastion E2E Failures
> **Fecha**: 2026-05-08 | **Contexto**: Análisis post E2E full feature test

---

## F-010: `sandbox_read` — Base64 Decode Failure

### Causa Raíz (CONFIRMADA)

**Archivo**: `crates/bastion-infrastructure/src/provider/podman.rs:583`

```rust
let shell_cmd = format!("base64 {}", path);
let (stdout, _, exit_code) = self.exec_in_container(&container_name, &shell_cmd).await?;
// ...
let decoded = base64::engine::general_purpose::STANDARD
    .decode(&stdout)  // ← stdout incluye newlines de `base64` (wraps a 76 chars)
    .map_err(|e| DomainError::Internal(format!("Failed to decode base64: {}", e)))?;
```

El comando `base64` de GNU coreutils envuelve la salida en líneas de 76 caracteres (inserta `\n`). El decoder Rust `STANDARD` no tolera newlines dentro del base64.

**El mismo bug existe en**: `gvisor.rs:816` (usa `base64 -w0` como fallback pero no siempre funciona), `docker.rs:583` (idéntico al de podman).

### Fix — 2 opciones

#### Opción A (preferida): Usar `base64 -w0` en el comando shell

```rust
// ANTES:
let shell_cmd = format!("base64 {}", path);

// DESPUÉS:
let shell_cmd = format!("base64 -w0 {}", path);
```

`-w0` desactiva el wrapping. Esto evita newlines en la salida del comando.

**Impacto**: 1 línea en `podman.rs:583` + 1 línea en `docker.rs:583`. El `gvisor.rs:816` ya usa `-w0` como primer intento.

#### Opción B (más robusta): Strip whitespace antes de decodificar

```rust
// ANTES:
let decoded = base64::engine::general_purpose::STANDARD
    .decode(&stdout)

// DESPUÉS:
let cleaned: Vec<u8> = stdout.iter().copied().filter(|&b| b != b'\n' && b != b'\r').collect();
let decoded = base64::engine::general_purpose::STANDARD
    .decode(&cleaned)
```

**Recomendación**: Opción A + B combinadas. `-w0` es la solución principal, y el strip es un safety net.

### Archivos a modificar

| Archivo | Cambio |
|---------|--------|
| `crates/bastion-infrastructure/src/provider/podman.rs:583` | `base64 {}` → `base64 -w0 {}` |
| `crates/bastion-infrastructure/src/provider/docker.rs:583` | `base64 {}` → `base64 -w0 {}` |
| `crates/bastion-infrastructure/src/provider/podman.rs:596` | Strip whitespace antes de decode |
| `crates/bastion-infrastructure/src/provider/docker.rs:596` | Strip whitespace antes de decode |

**Esfuerzo**: Tiny (4 líneas)

---

## F-SYNC: `sandbox_sync` — Tar Decompression Failure

### Causa Raíz (CONFIRMADA)

**Archivo**: `crates/bastion-gateway/src/sandbox_tools.rs:1421-1434`

```rust
("pull", SyncBackend::Tar) | ("pull", SyncBackend::Auto) => {
    let cmd = format!(
        "podman exec {} tar czf - -C \"$(dirname '{}')\" \"$(basename '{}')\" 2>/dev/null | tar xzf - -C \"{}\"",
        container_name, source, source, target
    );
    tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output(),
    )
    .await
}
```

**El problema**: `$(dirname '{}')` y `$(basename '{}')` se ejecutan en el **host**, no dentro del container. Si `source` es `/tmp/spring-petclinic/target`:
- Host evalúa `dirname '/tmp/spring-petclinic/target'` → `/tmp/spring-petclinic`
- Host evalúa `basename '/tmp/spring-petclinic/target'` → `target`
- Pero el tar se ejecuta **dentro del container** con `podman exec`

El tar dentro del container intenta empaquetar algo que puede no existir o estar vacío, y el pipe al host recibe datos incompletos → gzip unexpected EOF.

Además, el target directory `$(dirname '{}')` del host side tar también se evalúa en el host — si el directorio no existe en el host, el tar falla.

### Fix — Usar comillas correctas y crear directorio destino

```rust
("pull", SyncBackend::Tar) | ("pull", SyncBackend::Auto) => {
    // Fix: eval dirname/basename correctly, create target dir on host
    let cmd = format!(
        "mkdir -p \"{}\" && podman exec {} tar czf - -C {} {} 2>/dev/null | tar xzf - -C \"{}\"",
        target,                    // mkdir on host
        container_name,
        shell_words::quote(&format!("{}/..", source)),  // dirname inside container
        shell_words::quote(source_filename),             // basename
        target
    );
}
```

Alternativa más simple: usar `podman cp` como backend por defecto en lugar de tar:

```rust
("pull", SyncBackend::Tar) | ("pull", SyncBackend::Auto) => {
    // Simpler: use podman cp which handles paths correctly
    let cmd = format!(
        "mkdir -p \"{}\" && podman cp {}:{} \"{}\"",
        target,
        container_name,
        source,
        format!("{}/", target)  // trailing slash = copy contents
    );
}
```

**Recomendación**: Cambiar `SyncBackend::Auto` para que resuelva a `PodmanCp` en lugar de `Tar` para pull, ya que `podman cp` maneja paths correctamente sin shell quoting issues. Mantener `Tar` como fallback para push (donde el tar se genera en el host).

### Fix alternativo para push

El push tiene un problema similar — el tar se genera en el host y se pipea al container:

```rust
("push", SyncBackend::Tar) | ("push", SyncBackend::Auto) => {
    let cmd = format!(
        "tar czf - -C \"$(dirname '{}')\" \"$(basename '{}')\" 2>/dev/null | podman exec -i {} tar xzf - -C \"{}\"",
        source, source, container_name, target
    );
}
```

Aquí `$(dirname)` y `$(basename)` se evalúan en el host, lo cual es correcto para el push. Pero el `target` se usa dentro del container con `podman exec` — debe existir en el container.

### Archivos a modificar

| Archivo | Cambio |
|---------|--------|
| `crates/bastion-gateway/src/sandbox_tools.rs:1387-1389` | Auto → PodmanCp para pull |
| `crates/bastion-gateway/src/sandbox_tools.rs:1421-1434` | Fix pull: usar podman cp o corregir paths |
| `crates/bastion-gateway/src/sandbox_tools.rs:1406-1420` | Fix push: crear target dir en container |
| `crates/bastion-gateway/src/sandbox_tools.rs:1436-1452` | PodmanCp: agregar `mkdir -p` antes de cp |

**Esfuerzo**: Medium (~20 líneas)

---

## F-ENV: `env_ref` / `CommandSpec.env_vars` — Variables de Entorno Perdidas

### Causa Raíz (CONFIRMADA — BUG CRÍTICO)

**Flujo del bug**:

1. `sandbox_prepare` genera un `ToolchainPlan` con `env = {"JAVA_HOME": "/usr/lib/jvm/default-java"}` y `path_prefix = ["/usr/lib/jvm/default-java/bin"]`
2. El gateway guarda el plan.env en `self.prepared_environments` como `HashMap<String, String>` (solo `JAVA_HOME`, **SIN PATH**)
3. `sandbox_run` resuelve `env_ref` → obtiene `{"JAVA_HOME": "/usr/lib/jvm/default-java"}`
4. `sandbox_run` llama a `command_spec.with_env("JAVA_HOME", "/usr/lib/jvm/default-java")`
5. `PodmanProvider::run_command()` construye `shell_cmd` y llama a `self.exec_in_container(&container_name, &shell_cmd)`
6. **`exec_in_container` NO acepta `env_vars`** → las variables de entorno se pierden

**Dos problemas en cadena**:

#### Problema 1: `path_prefix` no se propaga al `env_ref`

En `sandbox_tools.rs:1092-1098`:
```rust
// Generate env_ref and store the environment for later use by sandbox_run
let env_ref = format!("registry:{}:{}", sandbox_id, capability);
{
    let mut envs = self.prepared_environments.write().await;
    envs.insert(env_ref.clone(), plan.env.clone());  // ← Solo plan.env (sin PATH)
}
```

`plan.env` tiene `{"JAVA_HOME": "/usr/lib/jvm/default-java"}` pero `plan.path_prefix` tiene `["/usr/lib/jvm/default-java/bin"]` que NO se convierte a un `PATH` env var.

#### Problema 2: `exec_in_container` ignora `env_vars`

En `podman.rs:428-429`:
```rust
let (stdout, stderr, exit_code) =
    self.exec_in_container(&container_name, &shell_cmd).await?;
```

`exec_in_container` signature es:
```rust
async fn exec_in_container(&self, container_name: &str, command: &str) -> Result<...>
```

No hay parámetro `env_vars`. El `CreateExecOptions` usa `..Default::default()` sin `env`.

### Fix — 3 cambios necesarios

#### Fix 1: Propagar `path_prefix` como `PATH` en el env_ref

```rust
// En sandbox_tools.rs, donde se genera el env_ref:
let mut env = plan.env.clone();
if !plan.path_prefix.is_empty() {
    let path_prefix = plan.path_prefix.join(":");
    env.insert("PATH".to_string(), format!("{}:$PATH", path_prefix));
}
{
    let mut envs = self.prepared_environments.write().await;
    envs.insert(env_ref.clone(), env);  // Ahora incluye PATH
}
```

#### Fix 2: Modificar `exec_in_container` para aceptar env_vars

```rust
// ANTES:
async fn exec_in_container(&self, container_name: &str, command: &str) -> Result<...>

// DESPUÉS:
async fn exec_in_container(
    &self,
    container_name: &str,
    command: &str,
    env_vars: Option<&HashMap<String, String>>,
) -> Result<...> {
    let env = env_vars.map(|vars| {
        vars.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>()
    });
    let exec_config = bollard::exec::CreateExecOptions {
        cmd: Some(vec!["sh".to_string(), "-c".to_string(), command.to_string()]),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        env,  // ← Agregar env vars
        ..Default::default()
    };
```

#### Fix 3: Pasar `command.env_vars` en `run_command`

```rust
// ANTES (podman.rs:428-429):
let (stdout, stderr, exit_code) =
    self.exec_in_container(&container_name, &shell_cmd).await?;

// DESPUÉS:
let (stdout, stderr, exit_code) =
    self.exec_in_container(&container_name, &shell_cmd, Some(&command.env_vars)).await?;
```

### Archivos a modificar

| Archivo | Cambio |
|---------|--------|
| `crates/bastion-infrastructure/src/provider/podman.rs:103-162` | Agregar `env_vars` param a `exec_in_container` |
| `crates/bastion-infrastructure/src/provider/podman.rs:428-429` | Pasar `command.env_vars` |
| `crates/bastion-infrastructure/src/provider/docker.rs:103+` | Idem docker provider |
| `crates/bastion-infrastructure/src/provider/gvisor.rs:115+` | Idem gvisor provider |
| `crates/bastion-gateway/src/sandbox_tools.rs:1092-1098` | Agregar PATH al env_ref |
| `crates/bastion-gateway/src/sandbox_tools.rs:1182-1191` | Idem para resolver path |
| `crates/bastion-infrastructure/src/template/adapters/asdf.rs:16` | `ASDF_SOURCE` → usar `ASDF_DIR` export |

**Esfuerzo**: Medium (~30 líneas)

---

## F-ASDF: `AsdfAdapter` no setea `$ASDF_DIR`

### Causa Raíz

**Archivo**: `crates/bastion-infrastructure/src/template/adapters/asdf.rs:16`

```rust
const ASDF_SOURCE: &str = ". ~/.asdf/asdf.sh";
```

`asdf.sh` intenta derivar `ASDF_DIR` de `$BASH_SOURCE` que no está disponible en `podman exec -c '...'` (no es un shell interactivo).

### Fix

```rust
// ANTES:
const ASDF_SOURCE: &str = ". ~/.asdf/asdf.sh";

// DESPUÉS:
const ASDF_SOURCE: &str = "export ASDF_DIR=\"$HOME/.asdf\" && . \"$ASDF_DIR/asdf.sh\"";
```

**Esfuerzo**: Tiny (1 línea)

---

## PERF-01: `snapshot_create` — 109 segundos

### Causa Raíz

`podman commit` crea una capa completa del filesystem del container. Para containers con Java/Maven (~500MB instalado), esto toma >100s.

### Fixes propuestos

#### Opción A: Usar `podman checkpoint` (CRIU) — instantáneo
```bash
podman container checkpoint <id> --export=/tmp/checkpoint.tar.gz
```
CRIU checkpoint es ~10x más rápido que commit para containers en ejecución. Requiere kernel support.

#### Opción B: Pre-built pool con Java/Maven ya instalado
En lugar de hacer snapshot después de `sandbox_prepare`, tener imágenes pre-built:
```toml
[pool.templates."debian:jvm-ready"]
base = "debian:bookworm-slim"
prepare = "jvm-build"
```
El pool mantiene N containers de esta imagen ya preparados.

#### Opción C: Lazy snapshot (copy-on-write)
En lugar de `podman commit`, crear un diff tar de solo los archivos modificados:
```bash
podman diff <id>  # lista de archivos cambiados
podman exec <id> tar czf - <cambiados> > snapshot.tar.gz
```

**Recomendación**: Opción B (pre-built pool images) a corto plazo. Opción A (CRIU) a mediano plazo.

**Esfuerzo**: Medium (Opción B) / Large (Opción A)

---

## PERF-02: `sandbox_list_templates` — 7 segundos

### Causa Raíz

`podman images --format json` escanea TODAS las imágenes locales (85 en esta máquina), incluyendo ancestros de builds multi-stage.

### Fix

Cachear resultados con TTL de 60s:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};

struct CachedTemplates {
    templates: Vec<TemplateInfo>,
    fetched_at: Instant,
}

// En sandbox_list_templates:
let cache = self.templates_cache.read().await;
if let Some(ref cached) = *cache {
    if cached.fetched_at.elapsed() < Duration::from_secs(60) {
        return serde_json::json!({...cached.templates...}).to_string();
    }
}
// Fetch fresh...
```

**Esfuerzo**: Small (~20 líneas)

---

## Resumen: Plan de Acción Priorizado

| # | Bug | Fix | Esfuerzo | Prioridad |
|---|-----|-----|----------|-----------|
| 1 | **F-010 sandbox_read base64** | `base64 -w0` + strip whitespace | **Tiny** (4 líneas) | 🔴 P0 |
| 2 | **F-ENV env_vars perdidas** | `exec_in_container` + env param | **Medium** (30 líneas) | 🔴 P0 |
| 3 | **F-ENV path_prefix** | Agregar PATH al env_ref | **Tiny** (3 líneas) | 🔴 P0 |
| 4 | **F-ASDF ASDF_DIR** | Export ASDF_DIR en ASDF_SOURCE | **Tiny** (1 línea) | 🟡 P1 |
| 5 | **F-SYNC sandbox_sync tar** | Auto→PodmanCp para pull | **Medium** (20 líneas) | 🟡 P1 |
| 6 | **PERF list_templates** | Cache con TTL 60s | **Small** (20 líneas) | 🟢 P2 |
| 7 | **PERF snapshot_create** | Pre-built pool images | **Medium** (50+ líneas) | 🟢 P3 |

**Total estimado**: ~130 líneas de cambios para fixes P0+P1.

*Reporte generado: 2026-05-08*