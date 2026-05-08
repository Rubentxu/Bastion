# Bastion E2E Full Feature Test — Comprehensive Report

> **Fecha**: 2026-05-08
> **Método**: MCP HTTP Streamable (puerto 18765), Python script `scripts/mcp-e2e-full.py`
> **Sesión ID**: 34ba890e-c079-4de5-b260-af590a168bf6
> **Ambiente**: Bastion 0.1.0, Podman rootless, debian:bookworm-slim

---

## 1. Resumen Ejecutivo

| Métrica | Valor |
|---------|-------|
| ✅ PASS | 27 |
| ❌ FAIL | 8 |
| ⚠️ DEBT | 4 |
| ⏱️ PERF | 5 datos |
| 🔧 OPS | 0 |
| **Tasa de éxito** | **77%** (27/35) |

### Funcionalidades verificadas

| Feature | Estado | Detalle |
|---------|--------|---------|
| MCP Protocol (initialize + notifications) | ✅ | Session ID + `notifications/initialized` |
| sandbox_health | ✅ | Healthy: podman + pool (4 idle) |
| sandbox_list | ✅ | 14-16 sandboxes listadas |
| sandbox_pool_stats | ✅ | 4 total, 3 idle, 1 active |
| sandbox_metrics | ✅ | Prometheus format, 0 errors |
| sandbox_list_templates | ✅ | 85 templates detectadas (7.14s lento) |
| sandbox_create | ✅ | from_pool=True, instantáneo |
| sandbox_info | ✅ | status=running |
| sandbox_run (basic) | ✅ | echo, uname, whoami — 0.16-0.18s |
| sandbox_write | ✅ | ok, 0.16s |
| sandbox_read | ❌ | **BUG F-010**: base64 decode failure |
| sandbox_list_files | ✅ | count, entries con permisos |
| sandbox_prepare (apt) | ✅ | jvm-build, 64-87s |
| sandbox_run con env_ref | ⚠️ | Java verificado, pero Maven build falla |
| enrichment_health | ✅ | enabled=True, 0 enrichers, 22 runs |
| enrichment_optimizer_report | ✅ | 22 runs analyzed |
| enrichment_retention_info | ✅ | 22 rows, retención configurada |
| enrichment_retention_cleanup | ✅ | 0 deleted (ya limpia) |
| sandbox_snapshot create | ✅ | 109.49s (muy lento) |
| sandbox_snapshot list | ✅ | 4 snapshots |
| sandbox_sync (pull) | ❌ | **BUG**: tar decompression failure |
| sandbox_register_artifact | ✅ | registered |
| sandbox_cancel | ✅ | respondió |
| sandbox_terminate | ✅ | pooled |

---

## 2. Fallos Críticos (❌)

### F-010: `sandbox_read` — Base64 Decode Failure (CONFIRMADO)

**Severidad**: 🔴 CRÍTICO
**Observado**:
```
sandbox_read(sandbox_id, "/etc/hostname") → {"error":"Failed to decode base64: Invalid symbol 10, offset 16."}
```
"Symbol 10" = `\n` (LF). El contenido devuelto por `podman exec cat` incluye newlines que el base64 decoder rechaza.

**Impacto**: Imposible leer archivos del sandbox. Inoperable para cualquier flujo de extracción de artefactos.

**Fix propuesto**: Strip whitespace/newlines del string base64 antes de decodificar, o usar `base64 -w0` en el comando para evitar newlines en el encoding.

### F-SYNC: `sandbox_sync` — Tar Decompression Failure

**Severidad**: 🔴 CRÍTICO
**Observado**:
```
sandbox_sync(pull) → {"backend":"tar","error":"Sync failed: ","exit_code":2,"stderr":"gzip: stdin: unexpected end of file\ntar: Child returned status 1\ntar: Error is not recoverable: exiting now\n"}
```
**Impacto**: No se pueden sincronizar archivos entre host y sandbox.

**Análisis**: El backend tar falla al descomprimir. Probablemente el contenido que se genera vía `podman exec` para el tar no se transmite correctamente (binary data corruption).

### F-ASDF: asdf-vm no funciona en `sandbox_run`

**Severidad**: 🟡 MEDIO (workaround: usar apt)
**Observado**:
```
asdf: Error: Source directory could not be calculated. Please set $ASDF_DIR manually before sourcing this file.
```
**Causa raíz**: Cada `sandbox_run` ejecuta un `podman exec` en una shell fresh. La variable `$ASDF_DIR` no se persiste entre llamadas. El `. ~/.asdf/asdf.sh` intenta calcular su directorio usando `$BASH_SOURCE` pero en `podman exec -c '...'` no está disponible.

**Workaround**: Usar `export ASDF_DIR="$HOME/.asdf" && . "$ASDF_DIR/asdf.sh"` en lugar de `. ~/.asdf/asdf.sh`.

**Impacto**: La instalación de Java/Maven vía asdf falla si no se setea `$ASDF_DIR` explícitamente. El flujo de `sandbox_prepare(jvm-build, strategy=system_package)` funciona correctamente (usa apt).

### F-MVN: Maven build falla con `env_ref`

**Severidad**: 🟡 MEDIO
**Observado**: `mvn package -DskipTests` devuelve exit=2 en 0.2s, sin construir nada.
**Análisis**: El `env_ref` no parece estar propagando el PATH correctamente en la sandbox nueva. Posiblemente el `sandbox_prepare` instala paquetes vía apt pero no configura el PATH de forma persistente para `sandbox_run` subsiguiente.

**Nota**: En la primera sesión E2E, `sandbox_run` con `env_ref=jvm-build` sí verificó Java (`java -version` OK), pero `mvn package` en una sandbox nueva falló.

---

## 3. Deuda Técnica (⚠️)

| ID | Descripción | Severidad | Estado previo |
|----|-------------|-----------|---------------|
| D-010 | `sandbox_read` base64 decode — newlines en data | 🔴 | Confirmado F-010 |
| D-SYNC | `sandbox_sync` tar decompression failure | 🔴 | Bug nuevo |
| D-ASDF | asdf no funciona sin `$ASDF_DIR` explícito | 🟡 | Documentado |
| D-MVN | `env_ref` no propaga PATH para Maven | 🟡 | Bug funcional |

### Deuda previa (de workflow-failures-and-improvements.md)

| ID | Descripción | Estado actual |
|----|-------------|---------------|
| F-012 | snapshot restore no registra en MCP repository | No probado en este ciclo |
| F-013 | snapshot list returned 4 (antes decía 0) | ✅ Listo (ahora muestra 4 snapshots) |
| F-014 | Solo Podman provider soportado | Sin cambios |
| F-005 | sandbox_prepare timeout 10s | Ahora usa 300s (mejorado) |

---

## 4. Performance (⏱️)

| Operación | Tiempo | Notas |
|-----------|--------|-------|
| sandbox_create | 0.00s | from_pool=True (instantáneo) |
| sandbox_run (echo) | 0.17s | Comando trivial |
| sandbox_run (whoami) | 0.16s | Comando trivial |
| sandbox_write | 0.16s | Archivo pequeño |
| sandbox_list_files | 0.18s | Directorio pequeño |
| sandbox_run (java verify) | 0.48s | Con env_ref |
| apt install curl+git | 8.50s | Network-dependent |
| sandbox_prepare (apt jvm-build) | 64-87s | Descarga e instala Java+Maven |
| snapshot create | **109.49s** | ⚠️ Muy lento — `podman commit` |
| sandbox_list_templates | 7.14s | ⚠️ Lento — `podman images` scan |
| git clone petclinic | ~2s (est.) | Shallow clone |
| mvn package (estimado) | ~60-120s | No completado en este ciclo |

### Cuellos de bottella identificados

1. **snapshot create (109s)**: `podman commit` + `podman save`. No hay forma de evitar el commit completo. Propuesta: snapshot incremental o lazy-copy.
2. **sandbox_prepare (64-87s)**: Dominado por apt-get install. Propuesta: pre-built pool images con Java/Maven ya instalado.
3. **sandbox_list_templates (7s)**: Escanea todas las imágenes. Propuesta: cachear resultados.

---

## 5. Excelencia Operativa

### 5.1 Lo que funciona bien

| Feature | Observación |
|---------|------------|
| Hot pool | `from_pool=True` da sandboxes instantáneamente (0s) |
| MCP HTTP transport | Estable, sin desconexion, session management funciona |
| Pool stats | Muestra hot/idle/active/template counts |
| Metrics Prometheus | Contadores de sandbox creados, comandos ejecutados, latency |
| Enrichment recorder | 22 runs registrados, retención funcional |
| Multiple providers in templates | 85 templates detectadas |
| `sandbox_cancel` | Responde graciosamente |
| `sandbox_terminate` | Retorna sandbox al pool (`status: pooled`) |
| `sandbox_list_templates` | Funciona, incluye suggested_name |

### 5.2 Lo que necesita mejora

| Área | Problema | Propuesta |
|------|----------|-----------|
| Session protocol | `Mcp-Session-Id` debe propagarse como header | Ya documentado y corregido en mcp-health.py |
| Error messages | `sandbox_read` devuelve "Invalid symbol 10" sin contexto | Incluir path del archivo y longitud del contenido |
| Performance snapshot | 109s es inaceptable para snapshots frecuentes | Implementar copy-on-write o checkpoint/restore (CRIU) |
| `sandbox_list_templates` pide `podman images` | Escaneo global de 85 imágenes toma 7s | Cachear resultados con TTL de 60s |
| `env_ref` no funciona para Maven build | PATH no se propaga correctamente en apt installs | Verificar que el script de prepare agrega al PATH globalmente |

---

## 6. Análisis de asdf-vm en Sandbox

### 6.1 Problema

Cada `sandbox_run` ejecuta un comando nuevo en `podman exec`. Las variables de entorno no persisten entre comandos. El script `. ~/.asdf/asdf.sh` intenta derivar `ASDF_DIR` de `$BASH_SOURCE`, que no está disponible en `podman exec -c '...'`.

### 6.2 Solución

```bash
# En lugar de:
. ~/.asdf/asdf.sh && asdf install java adoptopenjdk-17.0.8+7

# Usar:
export ASDF_DIR="$HOME/.asdf" && . "$ASDF_DIR/asdf.sh" && asdf install java adoptopenjdk-17.0.8+7
```

### 6.3 Recomendación arquitectónica

El `sandbox_run` debería soportar un campo `env` para pasar variables de entorno persistentes:

```json
{
  "sandbox_id": "...",
  "command": "asdf install java ...",
  "env": {"ASDF_DIR": "/root/.asdf", "PATH": "/root/.asdf/shims:/usr/local/bin:/usr/bin:/bin"}
}
```

Alternativamente, `sandbox_prepare` con `strategy=version_manager` debería configurar asdf globalmente en el sandbox (no solo en `.bashrc`).

---

## 7. Propuestas de Mejora Priorizadas

| # | Propuesta | Severidad | Esfuerzo | Feature |
|---|-----------|-----------|----------|---------|
| 1 | **Fix sandbox_read base64** (F-010) | 🔴 CRÍTICO | Small | Bug existente |
| 2 | **Fix sandbox_sync tar** | 🔴 CRÍTICO | Medium | Bug nuevo |
| 3 | **Fix env_ref PATH propagation** | 🟡 MEDIUM | Medium | Bug funcional |
| 4 | **Document asdf ASDF_DIR workaround** | 🟡 MEDIUM | Tiny | Docs |
| 5 | **Cache sandbox_list_templates** | 🟢 LOW | Small | Perf |
| 6 | **Optimize snapshot create** (incremental) | 🟢 LOW | Large | Feature |
| 7 | **Add `env` field to sandbox_run** | 🟢 LOW | Medium | Feature |
| 8 | **Pre-built pool images con Java/Maven** | 🟢 LOW | Medium | Perf |
| 9 | **Reconnect orphaned containers on startup** | 🟡 MEDIUM | Medium | Robustness |
| 10 | **MCP protocol session recovery** (auto-reconnect) | 🔴 HIGH | Large | OpenCode-side |

---

## 8. Comparación con Datos Previos (workflow-failures-and-improvements.md)

| ID | Falla previa | Estado actual |
|----|-------------|---------------|
| F-001 | MCP "Not Connected" after gateway crash | No observado (HTTP transport stable) |
| F-002 | Stdio transport "connection closed: initialize request" | No aplica (usamos HTTP) |
| F-003 | HTTP MCP tools don't auto-connect | ✅ Resuelto: session ID management |
| F-004 | Image not found (debian-slim) | ✅ sandbox_list_templates funciona |
| F-005 | sandbox_prepare timeout (10s) | ✅ Mejorado: timeout_ms configurable, 300s default |
| F-006 | Maven build timeout | ⚠️ Parcialmente: env_ref no propaga PATH |
| F-007 | git clone --depth1 syntax | No aplica (uso correcto) |
| F-008 | ToolResolver always picks apt | By design — apt es 3.7x más rápido |
| F-009 | AsdfAdapter wrong Java version | Confirmado: `$ASDF_DIR` no se setea |
| F-010 | sandbox_read base64 decoding | 🔴 Confirmado: Invalid symbol 10 |
| F-011 | sandbox_sync is a stub | 🔴 Ahora parcialmente implementado pero tar falla |
| F-012 | snapshot restore not registered | No probado este ciclo |
| F-013 | snapshot list empty | ✅ Arreglado: ahora muestra 4 snapshots |
| F-014 | Only podman provider | Sin cambios |
| F-015 | Snapshot create timeout | ⚠️ Ahora funciona pero 109s |

---

## 9. Hallazgo Nuevo: MCP Session Protocol

### 9.1 Descubrimiento

El MCP HTTP transport requiere que el cliente:
1. Envíe `initialize` → recibe `Mcp-Session-Id` en response headers
2. Envíe `notifications/initialized` con el session ID → recibe 202
3. Envíe `tools/call` con el session ID → funcional

Sin el session ID, el servidor trata cada request como nueva sesión y devuelve 422 "Unexpected message, expect initialize request".

### 9.2 Error original

```
HTTP 422: Unprocessable Entity — "Unexpected message, expect initialize request"
```

### 9.3 Fix

En Python con urllib, los headers se normalizan a lowercase. Usar `"mcp-session-id"` en lugar de `"Mcp-Session-Id"`.

### 9.4 Impacto

Todos los clientes MCP HTTP necesitan implementar el session management correctamente. Esto NO era documentado claramente en el spec previo.

---

*Reporte generado: 2026-05-08*
*Script de prueba: `scripts/mcp-e2e-full.py`*
*Datos raw: `/tmp/bastion-e2e-findings.json`*