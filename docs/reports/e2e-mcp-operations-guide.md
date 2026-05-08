# E2E MCP Operations & Test Guide — Bastion

> **Fecha**: 2026-05-08
> **Proyecto**: Bastion MCP Gateway
> **Idioma**: Esta guía documenta operaciones E2E MCP y requisitos para testing con enriquecimiento.

---

## 1. Cómo Levantar Bastion MCP para OpenCode

### 1.1 Con Just (Recomendado)

```bash
# Construir binarios release y arrancar gateway MCP
just mcp-start

# Verificar estado (PID + TCP check)
just mcp-status

# Health check MCP (initialize + sandbox_health)
just mcp-health

# Ver logs del gateway
just mcp-logs

# Parar el gateway
just mcp-stop

# Reiniciar
just mcp-restart
```

### 1.2 Configuración del Host

El Justfile usa estas variables de entorno (con valores por defecto):

| Variable | Default | Descripción |
|----------|---------|-------------|
| `BASTION_MCP_HOST` | `127.0.0.1` | Host del gateway |
| `BASTION_MCP_PORT` | `18765` | Puerto HTTP del gateway |
| `BASTION_GATEWAY_BIN` | `target/release/bastion-gateway` | Binario del gateway |
| `BASTION_WORKER_BIN` | `target/release/bastion-worker` | Binario del worker |
| `BASTION_CONFIG` | `config/sandbox-gateway.toml` | Archivo de configuración |
| `BASTION_CONFIG_DIR` | `.bastion` | Directorio de configuración |

### 1.3 Verificación desde OpenCode

OpenCode tiene Bastion configurado como MCP remoto en `http://127.0.0.1:18765`. El servidor remote **no auto-arranca** el proceso — requiere:

1. `just mcp-start` manual antes de usar OpenCode
2. O arrancar el gateway manualmente:

```bash
RUST_LOG=bastion=info ./target/release/bastion-gateway \
  --transport http \
  --http-port 18765 \
  --worker-binary target/release/bastion-worker \
  --config config/sandbox-gateway.toml
```

---

## 2. Prerrequisitos E2E

### 2.1 Podman Socket

**Requerido para**: Tests de provider, tests de lifecycle, enrichment e2e.

```bash
# Verificar que el socket existe
ls -la /run/user/1000/podman/podman.sock

# Si no existe, crear y arrancar:
mkdir -p $XDG_RUNTIME_DIR/podman
podman system service --time 3600 unix://$XDG_RUNTIME_DIR/podman/podman.sock &

# Alternativa (systemd user):
systemctl --user enable --now podman.socket
systemctl --user status podman.socket
```

### 2.2 Binarios Release

**Requerido para**: MCP real (no para tests debug).

```bash
# Construir binarios release
just build-release
# Equivalente a: cargo build --release

# Verificar
ls -la target/release/bastion-gateway
ls -la target/release/bastion-worker
```

### 2.3 Variables de Entorno para Enrichment

Los enrichment e2e tests requieren:

| Variable | Valor | Descripción |
|----------|-------|-------------|
| `BASTION_E2E_ENRICHMENT` | `1` | Activa los tests de enrichment ignorados |
| `BASTION_DATABASE_URL` | `sqlite:.bastion/enrichment.db` | (Opcional) Base de datos para recorder |
| `BASTION_ADVICE` | `0` o `1` | (Opcional) Des/habilita advice engine |

**Nota**: Los enrichment tools (`enrichment_optimizer_report`, `enrichment_retention_info`, `enrichment_retention_cleanup`, `enrichment_health`) pueden devolver errores si el recorder no está configurado. Esto es **esperado** — no indica fallo del test.

### 2.4 Resumen de Prerrequisitos por Tipo de Test

| Test | Podman Socket | Release Binaries | BASTION_E2E_ENRICHMENT |
|------|--------------|------------------|------------------------|
| Unit tests (`cargo test --lib`) | No | No | No |
| Gateway e2e (`cargo test -p bastion-gateway`) | Sí (para lifecycle) | No (usa debug) | No |
| Provider lifecycle (`cargo test -p bastion-infrastructure`) | Sí | Sí | No |
| **Enrichment e2e** (`--test enrichment_e2e -- --ignored`) | Sí | No (usa debug) | **Sí** |
| MCP real (OpenCode) | Sí | **Sí** | No |

---

## 3. Cómo Ejecutar Pruebas MCP Reales

### 3.1 Tests que Validan MCP Realmente

Los siguientes **SÍ** usan el protocolo MCP sobre stdio:

```bash
# Gateway E2E tests — usan spawn_gateway() con stdio
cargo test -p bastion-gateway --test e2e_test

# Provider lifecycle tests —usan spawn_provider() con API directa
cargo test -p bastion-infrastructure --test podman_lifecycle

# Enrichment E2E tests — usan spawn_gateway() con stdio + JSON-RPC
BASTION_E2E_ENRICHMENT=1 cargo test -p bastion-gateway --test enrichment_e2e -- --ignored
```

### 3.2 Scripts que NO Validan MCP

Estos scripts/test harness **NO** usan MCP — ejecutan Podman directamente:

```bash
# ❌ NO es un test MCP — usa Podman exec directamente
cargo test -p bastion-infrastructure --test podman_optimized_test

# ❌ NO es un test MCP — verificación de output de proceso
python3 tests/e2e_template_artifacts.py
```

**Diferencia clave**:
- **MCP tests**: Envían JSON-RPC sobre stdio/http al gateway, que actúa como servidor MCP
- **Direct Podman tests**: Ejecutan `podman exec` directamente sin pasar por el gateway

### 3.3 Comandos Exactos para Tests

```bash
# === Tests Unitarios (sin I/O real) ===
cargo test --lib

# === Tests de Gateway E2E (MCP real) ===
cargo test -p bastion-gateway --test e2e_test

# === Tests de Provider Lifecycle (Podman API directa) ===
cargo test -p bastion-infrastructure --test podman_lifecycle

# === Tests de Enrichment E2E (MCP + enrichment) ===
# Requiere: Podman socket + BASTION_E2E_ENRICHMENT=1
BASTION_E2E_ENRICHMENT=1 cargo test -p bastion-gateway --test enrichment_e2e -- --ignored

# === Health check del gateway (MCP real) ===
just mcp-health

# === Verificación de MCP tools disponibles ===
# Usar OpenCode para llamar tools/list y tools/call
```

---

## 4. Cómo Ejecutar Enrichment E2E (Ignored/Gated)

### 4.1 tests/enrichment_e2e.rs

Hay 5 tests en `crates/bastion-gateway/tests/enrichment_e2e.rs`:

| Test | Qué Valida | Requiere Podman |
|-----|-----------|-----------------|
| `test_maven_enrichment_sandbox` | Maven build → facts + build_status + enricher_id | **Sí** |
| `test_optimizer_report` | Tool `enrichment_optimizer_report` | No (state-only) |
| `test_retention_info` | Tool `enrichment_retention_info` | No (state-only) |
| `test_retention_cleanup` | Tool `enrichment_retention_cleanup` | No (state-only) |
| `test_enrichment_health` | Tool `enrichment_health` | No (state-only) |

### 4.2 Ejecución

```bash
# Activar enrichment tests
export BASTION_E2E_ENRICHMENT=1

# Ejecutar todos los enrichment e2e (tests igno
cargo test -p bastion-gateway --test enrichment_e2e -- --ignored

# Ejecutar un test específico
cargo test -p bastion-gateway --test enrichment_e2e -- --ignored test_maven_enrichment_sandbox

# Ver output de un test
BASTION_E2E_ENRICHMENT=1 cargo test -p bastion-gateway --test enrichment_e2e test_maven_enrichment_sandbox -- --ignored --nocapture
```

### 4.3 Interpretación de Resultados

**Éxito**:
```
✅ enrichment test passed: enricher_id=maven, source=..., timestamp=..., build_status=BUILD SUCCESS, facts=5
```

**Error esperado (recorder no configurado)**:
```
ℹ️ enrichment_optimizer_report returned error (recorder may not be configured): "enrichment recorder not configured"
```

**Fallo ambiental (Podman no disponible)**:
```
SKIPPED: Podman not available
```

---

## 5. Cómo Interpretar Fallos: Ambientales vs Regresiones

### 5.1 Árbol de Decisión para Fallos de Test

```
┌─────────────────────────────────────────────────────────────────┐
│ ¿El test falla en CI pero no en tu máquina?                     │
├─────────────────────────────────────────────────────────────────┤
│ SÍ → Probablemente fallo ambiental                               │
└─────────────────────────┬───────────────────────────────────────┘
                          NO
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ ¿El test requiere Podman y tu `podman info` funciona?           │
├─────────────────────────────────────────────────────────────────┤
│ NO → Fallo ambiental: arrancar Podman socket                     │
│      systemctl --user start podman.socket                       │
└─────────────────────────┬───────────────────────────────────────┘
                          SÍ
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ ¿El test usa `#[ignore]` y tienes `BASTION_E2E_ENRICHMENT=1`? │
├─────────────────────────────────────────────────────────────────┤
│ NO → Ejecutar con: export BASTION_E2E_ENRICHMENT=1             │
└─────────────────────────┬───────────────────────────────────────┘
                          SÍ
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ ¿Es un test de enrichment tools (optimizer_report, retention_*)?│
├─────────────────────────────────────────────────────────────────┤
│ SÍ → ¿Devuelve error "recorder not configured"?                │
│      → ES ESPERADO si no hay DB configurada                     │
│      → No es regresión                                         │
└─────────────────────────┬───────────────────────────────────────┘
                          SÍ
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ ¿El error menciona "connection refused" o "sandbox not found"?│
├─────────────────────────────────────────────────────────────────┤
│ SÍ → El gateway no está corriendo o la sandbox expiró           │
│      just mcp-start                                             │
│      just mcp-health                                            │
└─────────────────────────┬───────────────────────────────────────┘
                          SÍ
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ ¿El error es "No such image" o timeout de apt-get?              │
├─────────────────────────────────────────────────────────────────┤
│ SÍ → Fallo ambiental: imagen no cacheada, red lenta             │
│      Reintentar: el test tiene retry loop                       │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Señales de Fallo Ambiental

| Síntoma | Causa Probable | Solución |
|---------|---------------|----------|
| `SKIPPED: Podman not available` | Socket no existe | `podman system service` o `systemctl --user start podman.socket` |
| `Connection refused` en `sandbox_run` | Gateway no corriendo | `just mcp-start` |
| `No such image` en create | Imagen no pulleada | `podman pull debian:bookworm-slim` |
| Timeout en apt-get | Red lenta, lock dpkg | Reintentar (test tiene retry) |
| `Request timed out` (~10s) | MCP timeout < operation | Timeout del test exceeded — no es bug |
| `SKIPPED: Set BASTION_E2E_ENRICHMENT=1` | Falta env var | `export BASTION_E2E_ENRICHMENT=1` |

### 5.3 Señales de Regresión Real

| Síntoma | Indicación |
|---------|-----------|
| `Expected 'enricher_id' == 'maven', got '...'` | Enriquecimiento no funciona |
| `Expected non-empty facts` | Extractor no extrae facts |
| `Sandbox not found` en todas las llamadas | Repository corrupto o reset |
| `base64 decode error` | Encoding issue en sandbox_read (bug F-010 conocido) |
| `snapshot restore` → sandbox invisible | Bug F-012 conocido |

### 5.4 Debugging Paso a Paso

```bash
# 1. Verificar entorno básico
just mcp-status
just mcp-health

# 2. Ver logs del gateway
just mcp-logs

# 3. Verificar Podman
podman info --format json | head -20
ls -la /run/user/1000/podman/podman.sock

# 4. Tests con output detallado
cargo test -p bastion-gateway --test e2e_test -- --nocapture 2>&1 | head -100

# 5. Tests de enrichment con debug
RUST_LOG=bastion=debug BASTION_E2E_ENRICHMENT=1 \
  cargo test -p bastion-gateway --test enrichment_e2e test_maven_enrichment_sandbox \
  -- --ignored --nocapture 2>&1 | tail -50
```

---

## 6. Nota sobre S1860 False Positives

### 6.1 Contexto

CogniCode Quality reportó 11 Criticals clasificados como **S1860** (condición siempre true/false) en el análisis del proyecto.

### 6.2 Verificación Previa

El equipo de verify clasificó estos 11 issues como:
- **Falsos positivos**: patrones de código que CogniCode detecta como always-true/always-false pero que en realidad son válidos (e.g., проверка границ, использование `cfg!()`)
- **No bloqueantes**: la lógica de producción no está afectada

### 6.3 Recomendación

> **⚠️ NO suprimir S1860 automáticamente** salvo que se confirme un patrón estable de falsos positivos.

Criterios para supresión futura (solo si se cumplen TODOS):
1. ✅ El issue está verificado como falso positivo por un humano
2. ✅ El patrón es consistente (mismo código repite el issue)
3. ✅ Existe un mecanismo de supresión establecido en el proyecto (e.g., `#[allow(clippy::s1860)]` o filtro en `cognicode-quality.yml`)
4. ✅ La supresión se documenta con comentario explicando por qué es FP

### 6.4 Cómo Verificar un S1860 Sospechoso

```bash
# 1. Obtener detalles del issue
cognicode-quality_check_code_smell --rule_id S1860 --file_path <ruta>

# 2. Analizar el código manualmente
# Buscar: cfg!, const_evaluar_check, bound checks, etc.

# 3. Si es FP confirmado, suprimir localmente con:
#[allow(clippy::s1860)]
// código que genera el warning

# 4. NO usar supresiones globales sin confirmar
```

### 6.5 Tracking

Para tracks de S1860 en el futuro, usar Engram con:
- `topic_key`: `quality/s1860-false-positives`
- `type`: `bugfix` si se confirma como bug real, `discovery` si es FP

---

## 7. Resumen: Comandos Reproducibles

### 7.1 Setup Completo para MCP + Enrichment Testing

```bash
# 1. Arrancar Podman socket
systemctl --user start podman.socket

# 2. Construir binarios release (para MCP real)
cargo build --release

# 3. Arrancar gateway MCP
just mcp-start

# 4. Verificar
just mcp-health

# 5. Ejecutar tests de gateway e2e
cargo test -p bastion-gateway --test e2e_test

# 6. Ejecutar enrichment e2e (requiere ignore flag)
BASTION_E2E_ENRICHMENT=1 cargo test -p bastion-gateway --test enrichment_e2e -- --ignored

# 7. Parar gateway
just mcp-stop
```

### 7.2 Quick Reference

| Acción | Comando |
|--------|---------|
| Arrancar MCP | `just mcp-start` |
| Status MCP | `just mcp-status` |
| Health check | `just mcp-health` |
| Logs | `just mcp-logs` |
| Parar | `just mcp-stop` |
| Tests gateway | `cargo test -p bastion-gateway --test e2e_test` |
| Tests enrichment | `BASTION_E2E_ENRICHMENT=1 cargo test --test enrichment_e2e -- --ignored` |
| Verify Podman | `podman info --format json \| jq .version` |

---

*Documento generado: 2026-05-08*
*Última actualización: Follow-up E2E MCP/enrichment documentation + S1860 notes*
