# Proactive Enrichment Engine — Documento de Diseño

> **Proyecto**: Bastion MCP Gateway
> **Fecha**: Mayo 2026
> **Estado**: Propuesta — Fase de diseño
> **Autoría**: Arquitectura y decisiones de diseño del equipo Bastion

---

## 1. Resumen Ejecutivo

El **Proactive Enrichment Engine** transforma la experiencia del agente (LLM) al ejecutar comandos en sandboxes. Hoy el agente recibe salida cruda de `sandbox_run` y debe realizar múltiples llamadas adicionales (`assertion_run`, `advice_suggest`, `sandbox_run` para inspeccionar artefactos) para entender qué pasó. Cada llamada extra consume tokens, añade latencia y riesgo de error de interpretación.

El Enrichment Engine infiere la intención del agente a partir del comando ejecutado, y en **una sola respuesta** le entrega: resultado de assertions, artefactos extraídos, métricas de tests, coordenadas del proyecto y consejos contextuales. Sin llamadas extra. Sin carga cognitiva.

La pieza central no es solo un conjunto de reglas YAML. Es un **arnés semántico para agentes**: una capa de **lenguaje natural estructurado** que describe propósito, intención, utilidad, señales, semántica de éxito/fallo y el contexto que debe devolverse al agente. Esa capa se conecta con extractores, CEL y rules deterministas para producir contexto útil, seguro y reproducible.

---

## 2. Problema y Motivación

### 2.1 El flujo actual: información cruda

Cuando un agente ejecuta `sandbox_run(command="mvn package")`, recibe:

```json
{
  "exit_code": 0,
  "stdout": "...3000 líneas de log...",
  "stderr": "",
  "duration_ms": 12345
}
```

Nada más. Salida sin procesar. El agente (LLM) debe entonces:

1. **Parsear stdout** para determinar si apareció `BUILD SUCCESS` — trabajo de interpretación propenso a errores
2. Llamar `sandbox_run("ls target/*.jar")` para encontrar artefactos — **1 llamada LLM extra**
3. Llamar `sandbox_run("du -h target/*.jar")` para obtener tamaños — **otra llamada LLM extra**
4. Llamar `assertion_run("maven.build.success")` para validar — **otra llamada LLM extra**
5. Llamar `advice_suggest(...)` para obtener sugerencias — **otra llamada LLM extra**

### 2.2 Coste de cada llamada extra

| Concepto | Por llamada extra | Con 4-5 llamadas extra |
|----------|-------------------|------------------------|
| **Tokens** | ~500-2000 tokens ida/vuelta | 2.000-10.000 tokens desperdiciados |
| **Latencia** | ~2-5s (LLM round-trip) | 8-25s de espera total |
| **Coste** | Proporcional a tokens | 4-5x el coste necesario |
| **Errores** | Riesgo de interpretación errónea | Se acumulan |
| **Carga cognitiva** | El agente debe "saber" que existen las herramientas | Descubrimiento y decisión en cada sesión |

### 2.3 El problema fundamental

Los mecanismos existentes (assertions, doctors, advice) son **bajo demanda** (`on-demand`). El agente debe:

- **Saber** que existen estas herramientas
- **Decidir** llamarlas explícitamente
- **Interpretar** cuáles son relevantes para cada comando

Esto es carga cognitiva innecesaria. El sistema debería ayudar al agente, no darle más trabajo.

---

## 3. Estado Actual

Bastion ya dispone de un catálogo de componentes que operan sobre las experiencias registradas:

### 3.1 Experience Records

Cada `sandbox_run` genera un `ExperienceRecord` con:

- `stdout`, `stderr` — salida del comando
- `exit_code` — código de salida
- `duration` — duración de la ejecución
- `sandbox_id`, `trace_id` — contexto

Implementado en `bastion-domain/src/catalog/experience.rs`. El registro se realiza en `server.rs:266` (`record_experience`) y se invoca en múltiples puntos del gateway tras cada ejecución de comando.

### 3.2 Assertions

Validaciones definidas en TOML que se evalúan contra una experiencia. Ejemplo actual:

```toml
# .bastion/catalog/assertions/maven.build.success.toml
[assertion]
id = "maven.build.success"
name = "Maven Build Success"
description = "Maven build must exit with code 0 and stdout must contain BUILD SUCCESS"
category = "maven"

[[assertion.checks]]
type = "exit_code"
expected = 0

[[assertion.checks]]
type = "stdout_contains"
substring = "BUILD SUCCESS"
```

El agente debe llamar explícitamente `assertion_run("maven.build.success")`.

### 3.3 Doctors

Chequeos de pre-condición definidos en TOML. Ejemplo actual:

```toml
# .bastion/catalog/doctors/sandbox.alive.toml
[doctor]
id = "sandbox.alive"
name = "Sandbox Alive"
description = "Checks that a sandbox is alive and responsive"
category = "sandbox"
severity = "critical"

[[doctor.checks]]
type = "aliveness"
```

El agente debe llamar explícitamente `doctor_run("sandbox.alive")`.

### 3.4 Advice

Sugerencias contextuales definidas en TOML, disparadas por fallos de assertions, doctors o patrones de experiencia. Ejemplo actual:

```toml
# .bastion/catalog/advice/maven.build.failure.toml
schema_version = "1.0"

[advice]
id = "maven.build.failure"
name = "Maven Build Failure"
description = "Triggered when a Maven build assertion fails"
category = "maven"
severity = "warning"

[[advice.triggers]]
type = "assertion_failed"
assertion_id = "maven.build.success"

message = "Maven build failed. Check the output for compilation errors."
suggested_actions = [
    "Review Maven output for compilation errors",
    "Ensure all dependencies are available",
    "Check for syntax errors in recent changes",
    "Run `mvn clean` to clear cached artifacts"
]
hint = "Check the full build log for the first error — subsequent errors are often cascading"
```

El agente debe llamar explícitamente `advice_suggest(...)`.

### 3.5 Estructura del catálogo

```
.bastion/
├── catalog/
│   ├── assertions/        # Archivos .toml con validaciones
│   ├── doctors/           # Archivos .toml con pre-condiciones
│   └── advice/            # Archivos .toml con consejos
```

### 3.6 Limitación clave

Todos estos componentes son **reactivos y bajo demanda**. El agente debe descubrirlos, decidir usarlos y realizar llamadas explícitas. No hay mecanismo para que el sistema los orqueste proactivamente.

---

## 4. Solución: Enrichment Engine Proactivo

### 4.1 Principio fundamental

> **Ayuda al agente, no le des más trabajo.** El agente no debería necesitar conocer assertions, doctors, advice, patrones ni artefactos. Simplemente recibe respuestas enriquecidas.

### 4.2 Cómo funciona

```
Agente llama sandbox_run(command="mvn package")
        │
        ▼
┌─────────────────────────────────────────────┐
│  1. Pattern Matching                         │
│     ¿Existe un enricher que coincida         │
│     con el comando?                          │
│     regex: ^mvn\s+(package|install|verify)   │
└────────────────┬────────────────────────────┘
                 │ Sí
                 ▼
┌─────────────────────────────────────────────┐
│  2. PRE-ejecución: Pre-checks (Doctors)      │
│     - sandbox.alive                          │
│     - docker.daemon                          │
│     Si falla → respuesta inmediata con error │
└────────────────┬────────────────────────────┘
                 │ OK
                 ▼
┌─────────────────────────────────────────────┐
│  3. EJECUCIÓN del comando                    │
│     sandbox_run("mvn package")               │
│     → exit_code, stdout, stderr, duration    │
└────────────────┬────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────┐
│  4. POST-ejecución: Pipeline de enrichment   │
│     a) Assertions (maven.build.success)      │
│     b) Extractors (regex, glob, command)     │
│     c) Advice (generado desde resultados)    │
└────────────────┬────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────┐
│  5. RESPUESTA ENRIQUECIDA (una sola)         │
│     { exit_code, stdout, enrichment: {...} } │
└─────────────────────────────────────────────┘
```

### 4.3 El agente recibe todo en una respuesta

```json
{
  "exit_code": 0,
  "stdout": "...",
  "stderr": "...",
  "duration_ms": 12345,
  "enrichment": {
    "enricher_id": "maven.package",
    "assertions": [
      { "id": "maven.build.success", "passed": true }
    ],
    "build_status": { "status": "SUCCESS" },
    "tests": { "run": 42, "failed": 0, "errors": 0, "skipped": 0 },
    "artifacts": [{
      "name": "petclinic-3.4.0.jar",
      "path": "target/petclinic-3.4.0.jar",
      "size_bytes": 67108864,
      "size_human": "64 MB",
      "extension": ".jar"
    }],
    "maven_coords": {
      "group_id": "org.springframework.samples",
      "artifact_id": "spring-petclinic",
      "version": "3.4.0"
    },
    "advice": [{
      "message": "Build OK. Artifact: petclinic-3.4.0.jar (64 MB). Ready to deploy.",
      "severity": "hint"
    }]
  }
}
```

### 4.4 Comparativa: Antes vs Después

| Aspecto | Antes (on-demand) | Después (proactivo) |
|---------|-------------------|---------------------|
| Llamadas del agente | 1 + 4-5 extras | **1 sola** |
| Tokens consumidos | 2.000-10.000 extras | **0 extras** |
| Latencia adicional | 8-25s | **~200ms** (extractors) |
| Conocimiento requerido | Debe conocer herramientas | **Ninguno** |
| Artefactos | Exploración manual | **Extracción automática** |
| Assertions | Llamada explícita | **Auto-evaluados** |
| Advice | Llamada explícita | **Auto-generado** |

---

## 4b. Arnés semántico en lenguaje natural estructurado

### 4b.1 Motivación

El agente no necesita solo datos. Necesita **contexto interpretado**:

- qué ocurrió,
- por qué importa,
- qué evidencias sostienen la conclusión,
- qué artefactos o salidas son útiles,
- qué riesgo o bloqueo existe,
- y cuál es el siguiente paso más razonable.

Una configuración puramente rígida (`regex`, `glob`, `command`, `when`) puede
extraer y evaluar datos, pero no captura bien la **intención semántica** del
arnés. Por ejemplo, puede decir cómo encontrar `target/*.jar`, pero no explica
que ese JAR es probablemente el entregable principal del build Maven ni que el
agente debe recibir su ruta, tamaño y coordenadas para evitar llamadas extra.

Por eso el Enrichment Engine incorpora una capa de **lenguaje natural
estructurado**. Esta capa no sustituye a la ejecución determinista; la guía.

```text
Lenguaje natural estructurado
  → describe intención, utilidad, señales y contexto esperado
  ↓
Extractores / CEL / rules
  → ejecutan de forma determinista y segura
  ↓
Agent Context Composer
  → devuelve contexto compacto y útil al agente
```

### 4b.2 Qué contiene el arnés semántico

Cada enricher puede declarar un bloque `harness` con lenguaje natural
estructurado:

```yaml
harness:
  purpose: >
    When a Maven build runs, help the agent understand whether the build
    succeeded, what artifact was produced, whether tests passed, and what
    the next useful step is.

  activation_intent:
    - The agent is trying to build or package a Java/Maven project.
    - Commands may include mvn package, mvn verify, ./mvnw package, or make build.
    - A pom.xml file in the workspace increases confidence.

  useful_context:
    - Build verdict: success, failure, degraded, or unknown.
    - Generated artifacts with path, name, extension, size, and coordinates.
    - Test counts and failures, if available.
    - First actionable error if build failed.
    - Recommended next step.

  success_semantics: >
    A Maven build is successful when the command exits with code 0, the output
    contains BUILD SUCCESS, and no test failures are detected. If a deployable
    artifact is produced, include it as primary output.

  failure_semantics: >
    A Maven build is failed when the command exits non-zero, output contains
    BUILD FAILURE, tests fail, or no expected artifact is produced after an
    otherwise successful package command.

  agent_summary_template: >
    Maven build {{ build.verdict }}. Artifact {{ artifacts.primary.name }}
    generated at {{ artifacts.primary.path }} ({{ artifacts.primary.size_human }}).
```

Este bloque cumple tres funciones:

1. **Guía de autoría**: ayuda a humanos y agentes a entender por qué existe el
   enricher y qué información debe producir.
2. **Guía de composición**: el `Agent Context Composer` usa estas secciones para
   priorizar qué facts incluir en la respuesta.
3. **Base para mejora futura**: un pipeline de mejora puede comparar experiencias
   reales contra `useful_context` y proponer extractores o rules faltantes.

### 4b.3 Relación con la capa determinista

El arnés semántico no debe ejecutar lógica por sí mismo. La ejecución real debe
seguir siendo determinista:

| Capa | Responsabilidad | Puede fallar la ejecución |
|------|-----------------|---------------------------|
| `harness` | Explica intención, utilidad y semántica | No |
| `activation` | Decide si el enricher aplica | Sí, por confidence baja |
| `extractors` | Extraen datos observables | Sí, con graceful degradation |
| `rules` | Evalúan condiciones CEL | Sí, si config inválida |
| `composer` | Construye `agent_context` | No debe impedir respuesta raw |

Regla de seguridad: **el lenguaje natural nunca ejecuta comandos ni evalúa
condiciones directamente**. Solo orienta la composición y validación del
catálogo. La verdad operacional la determinan extractores, facts y rules.

### 4b.4 Agent Context Composer

El resultado final no debe ser un JSON libre e ilimitado. Debe ser un
`agent_context` compacto, priorizado y trazable:

```json
{
  "agent_context": {
    "summary": "Maven build succeeded. JAR generated at target/app.jar (64 MB).",
    "intent": {
      "tool": "maven",
      "operation": "package",
      "confidence": 0.92,
      "signals": ["command_regex", "pom.xml_exists"]
    },
    "verdict": "success",
    "primary_artifacts": [
      {
        "name": "app.jar",
        "path": "target/app.jar",
        "size_bytes": 67108864,
        "size_human": "64 MB",
        "kind": "java.jar"
      }
    ],
    "tests": { "run": 42, "failed": 0, "errors": 0, "skipped": 0 },
    "next_recommended": [
      {
        "severity": "hint",
        "message": "Sync the generated artifact if you need it on the host."
      }
    ]
  },
  "enrichment_meta": {
    "enricher_id": "maven.package",
    "duration_ms": 184,
    "facts_count": 7,
    "rules_fired": ["build_success", "artifact_detected"]
  }
}
```

El agente debe poder actuar con esta respuesta sin llamar otra vez al LLM ni a
Bastion para descubrir datos obvios.

### 4b.5 Puntos flacos y mitigaciones

| Riesgo | Problema | Mitigación |
|--------|----------|------------|
| Lenguaje natural ambiguo | Puede prometer contexto que no se extrae | Validación `catalog lint`: cada `useful_context` debe estar cubierto por extractor/rule o marcarse como best-effort |
| Exceso de texto | Puede aumentar tokens | `response.max_agent_context_bytes`, summaries compactos y facts raw opcionales |
| Inconsistencia entre semántica y rules | El harness dice una cosa y CEL evalúa otra | Tests de catálogo con fixtures de stdout/stderr y árboles de archivos |
| Seguridad | El harness podría inducir comandos peligrosos | El harness no ejecuta; solo extractores allowlisted ejecutan bajo policy |
| Sobreajuste a una tool | Demasiadas reglas Maven específicas | Separar `harness` semántico, extractores genéricos y facts normalizados |

---

## 5. Arquitectura

### 5.1 Enricher como Orquestador

Un **enricher** es una configuración YAML/TOML que orquesta TODAS las piezas del catálogo en un pipeline proactivo, activado por coincidencia de patrón de comando.

El enricher **no reemplaza** los componentes existentes. Los **referencia** por ID y los orquesta:

```
Enricher (maven.package)
  ├── pre_checks:     [doctor.podman.alive, sandbox.alive]
  ├── assertions:     [maven.build.success]
  ├── extractors:     [regex, glob, command]
  └── advice_scope:   maven
       └── usa advice del catálogo con triggers que coincidan
```

### 5.2 Pipeline de ejecución

```
                  sandbox_run(command)
                         │
                         ▼
              ┌──────────────────────┐
              │  EnricherRegistry    │
              │  match(command)      │
              └──────────┬───────────┘
                         │
              ┌──────────▼───────────┐
              │  Enricher encontrado? │
              └────┬────────────┬────┘
                   Sí           No
                   │            │
                   ▼            ▼
          ┌────────────┐   Ejecución normal
          │ PRE-CHECKS │   (sin enrichment)
          │  Doctors   │
          └─────┬──────┘
                │
          ┌─────▼──────┐
          │  ¿Falló?   │
          └──┬──────┬───┘
            Sí      No
             │      │
             ▼      ▼
    Respuesta    ┌──────────────┐
    con error    │  EJECUTAR    │
    de doctor    │  comando     │
                 └──────┬───────┘
                        │
                        ▼
                 ┌──────────────────────┐
                 │  POST-EXECUTION      │
                 │  ┌────────────────┐  │
                 │  │ 1. Assertions  │  │
                 │  │ 2. Extractors  │  │
                 │  │ 3. Advice      │  │
                 │  └────────────────┘  │
                 └──────────┬───────────┘
                            │
                            ▼
                   Respuesta enriquecida
```

### 5.3 Componentes Rust

```rust
/// Trait principal del enricher post-ejecución.
/// Implementado por cada enricher cargado desde catálogo.
trait PostRunEnricher: Send + Sync {
    /// Devuelve true si el comando coincide con el patrón del enricher.
    fn matches_command(&self, command: &str) -> bool;

    /// Ejecuta el pipeline de enrichment sobre el resultado.
    async fn enrich(&self, ctx: &PostRunContext) -> Vec<Enrichment>;
}

/// Contexto pasado al enricher tras la ejecución del comando.
struct PostRunContext {
    command: String,
    sandbox_id: SandboxId,
    exit_code: i32,
    stdout: String,
    stderr: String,
    sandbox_fs: Arc<dyn SandboxFileSystem>,  // para glob/stat
}

/// Trait para cada bloque extractor (building block).
trait ExtractorBlock: Send + Sync {
    fn extract(&self, ctx: &ExtractionContext) -> Result<Vec<serde_json::Value>, ExtractorError>;
}
```

### 5.4 Registry

```rust
/// Registro de enrichers cargados desde el catálogo.
struct EnricherRegistry {
    enrichers: Vec<Arc<dyn PostRunEnricher>>,
    compiled_patterns: Vec<Regex>,  // cache de regex compilados
}

impl EnricherRegistry {
    /// Encuentra el primer enricher cuyo pattern coincida con el comando.
    fn find_matching(&self, command: &str) -> Option<&Arc<dyn PostRunEnricher>>;

    /// Carga todos los enrichers desde `.bastion/catalog/enrichers/`
    async fn load_from_catalog(path: &Path, format: CatalogFormat) -> Result<Self>;
}
```

### 5.5 Relación con el código existente

Los componentes del catálogo existentes **no se eliminan**. En la primera fase
se orquestan por ID; en fases posteriores evolucionan para admitir condiciones
CEL además de los checks legacy:

- `AssertionRegistry` — se usa por referencia (por ID)
- `DoctorRegistry` — se usa por referencia (por ID)
- `AdviceRegistry` — se usa por referencia (scope/categoría)
- `ExperienceStore` — se sigue usando para el registro de experiencias

El enricher **orquesta** estos componentes, no los reemplaza. La evolución es
aditiva y retrocompatible.

### 5.7 Persistencia SQLite como fuente de consulta

El Enrichment Engine debe tener una regla clara de persistencia:

> Los ficheros de catálogo pueden existir como formato de autoría, revisión y
> versionado humano, pero la fuente de consulta de la aplicación en runtime es
> SQLite.

Esto evita que cada llamada MCP lea, parseé y valide ficheros. También permite
consultas rápidas, auditoría, trazabilidad, métricas y recuperación entre
sesiones.

```text
.bastion/catalog/*.yaml|toml
  → Catalog Watcher / Importer
  → Validación + normalización
  → SQLite catalog tables
  → Runtime queries desde DB
```

#### Responsabilidades de los ficheros

Los ficheros siguen siendo útiles para:

- edición por humanos,
- revisión en Git,
- distribución de packs,
- propuestas generadas por agentes,
- migraciones entre proyectos,
- compatibilidad con flujos declarativos.

Pero no deben ser la fuente primaria de lectura del gateway durante la
ejecución normal.

#### Responsabilidades de SQLite

SQLite almacena:

- catálogos importados y validados,
- versiones y hashes de ficheros,
- facts extraídos,
- harness runs,
- agent contexts,
- reglas disparadas,
- timings,
- fallos de extractores,
- métricas de utilidad,
- propuestas de mejora.

#### Detección de cambios

Todo fichero de catálogo importable debe tener un hash persistido.

```text
catalog_file_hash = sha256(path + content)
```

En arranque, reload explícito, o watcher FS:

1. Escanear `.bastion/catalog/`.
2. Calcular hash de cada fichero válido según `catalog.format`.
3. Comparar contra `catalog_sources.hash`.
4. Si cambió, reimportar.
5. Validar schema, CEL, extractors, budgets y policy.
6. Persistir descriptor normalizado.
7. Marcar versión anterior como superseded, no borrarla.

#### Invariante runtime

El gateway consulta enrichers, assertions, doctors, advice y rules desde SQLite:

```text
runtime → CatalogRepository(SQLite) → descriptors normalizados
```

No desde filesystem:

```text
runtime → filesystem parse each request   # prohibido en modo normal
```

Excepción: comandos administrativos (`catalog_import`, `catalog_validate`,
`catalog_diff`) pueden leer ficheros para sincronizar DB.

### 5.8 Modelo de datos SQLite propuesto

El modelo debe separar **fuentes**, **descriptores normalizados**, **ejecuciones**
y **resultados útiles para agentes**.

#### Tablas de catálogo

```sql
catalog_sources(
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  format TEXT NOT NULL,              -- yaml | toml
  kind TEXT NOT NULL,                -- enricher | assertion | doctor | advice | pack
  hash TEXT NOT NULL,
  version INTEGER NOT NULL,
  status TEXT NOT NULL,              -- active | superseded | invalid
  imported_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  diagnostics_json TEXT
)

catalog_descriptors(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  category TEXT,
  source_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  descriptor_json TEXT NOT NULL,     -- YAML/TOML normalizado a JSON
  normalized_json TEXT NOT NULL,     -- representación lista para runtime
  enabled INTEGER NOT NULL DEFAULT 1,
  trust_level TEXT NOT NULL DEFAULT 'project',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(source_id) REFERENCES catalog_sources(id)
)

enricher_index(
  enricher_id TEXT PRIMARY KEY,
  command_pattern TEXT,
  activation_json TEXT NOT NULL,
  category TEXT,
  priority INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  FOREIGN KEY(enricher_id) REFERENCES catalog_descriptors(id)
)
```

`catalog_descriptors.normalized_json` es la fuente directa de runtime. El parser
puede cambiar, pero el gateway no necesita reparsear YAML/TOML en cada request.

#### Tablas de ejecución del harness

```sql
harness_runs(
  id TEXT PRIMARY KEY,
  trace_id TEXT,
  experience_id TEXT,
  enricher_id TEXT NOT NULL,
  command TEXT NOT NULL,
  intent_json TEXT NOT NULL,
  status TEXT NOT NULL,              -- success | partial | failed | skipped
  started_at TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER,
  budget_json TEXT,
  error_json TEXT,
  FOREIGN KEY(enricher_id) REFERENCES catalog_descriptors(id)
)

facts(
  id TEXT PRIMARY KEY,
  harness_run_id TEXT NOT NULL,
  fact_type TEXT NOT NULL,           -- artifact | test_result | build_status | advice...
  fact_key TEXT,
  source TEXT NOT NULL,              -- extractor:glob:artifacts | rule:build_success
  confidence REAL NOT NULL,
  data_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(harness_run_id) REFERENCES harness_runs(id)
)

rule_firings(
  id TEXT PRIMARY KEY,
  harness_run_id TEXT NOT NULL,
  rule_id TEXT NOT NULL,
  group_name TEXT,
  priority INTEGER,
  condition TEXT NOT NULL,
  result INTEGER NOT NULL,
  output_json TEXT,
  duration_ms INTEGER,
  FOREIGN KEY(harness_run_id) REFERENCES harness_runs(id)
)

agent_contexts(
  id TEXT PRIMARY KEY,
  harness_run_id TEXT NOT NULL,
  summary TEXT NOT NULL,
  verdict TEXT,
  context_json TEXT NOT NULL,
  meta_json TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(harness_run_id) REFERENCES harness_runs(id)
)
```

#### Tablas de utilidad y mejora

```sql
enrichment_utility_events(
  id TEXT PRIMARY KEY,
  harness_run_id TEXT NOT NULL,
  event_type TEXT NOT NULL,          -- next_tool_call | repeated_lookup | used_artifact_path | ignored_advice
  data_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(harness_run_id) REFERENCES harness_runs(id)
)

catalog_proposals(
  id TEXT PRIMARY KEY,
  source_harness_run_id TEXT,
  proposal_kind TEXT NOT NULL,       -- add_extractor | adjust_rule | improve_summary | new_harness
  rationale TEXT NOT NULL,
  diff_json TEXT NOT NULL,
  status TEXT NOT NULL,              -- proposed | validated | approved | rejected | applied
  created_at TEXT NOT NULL
)
```

Estas tablas permiten responder preguntas clave:

- ¿Qué arnés se activó para esta acción?
- ¿Qué facts se extrajeron?
- ¿Qué reglas dispararon el advice?
- ¿Cuánto costó el enrichment?
- ¿El agente usó el contexto devuelto?
- ¿Qué parte del catálogo conviene mejorar?

### 5.9 Crate reusable `enrichment-engine`

El Enrichment Engine debe diseñarse como una librería Rust reusable, no como
una pieza acoplada al gateway de Bastion.

La regla arquitectónica es:

> `enrichment-engine` no conoce MCP, Podman, BastionGateway, SandboxId ni
> ExperienceRecord. Solo conoce operaciones, resultados, facts, catálogos,
> reglas y contexto para agente.

Bastion integra el crate mediante adaptadores.

```text
bastion-gateway
  → BastionEnrichmentAdapter
  → enrichment-engine
      → CatalogRepository
      → ExtractorEngine
      → CEL Rule Engine
      → AgentContextComposer
```

#### Objetivo del crate

El crate debe permitir que otros proyectos obtengan las mismas capacidades:

- inferir intención a partir de una acción previa,
- extraer datos útiles,
- normalizar facts,
- evaluar reglas declarativas,
- componer contexto útil para agentes,
- persistir trazas y resultados,
- mejorar catálogos con evidencia real.

Esto debe funcionar tanto en Bastion como en otros entornos:

- CLIs,
- agentes locales,
- CI/CD,
- runners Docker/Kubernetes,
- herramientas de análisis de repositorios,
- productos que quieran devolver contexto útil a LLMs.

#### Tipos core host-agnostic

```rust
pub struct OperationInvocation {
    pub tool_name: String,
    pub operation: String,
    pub command: Option<String>,
    pub trace_id: Option<String>,
    pub working_dir: Option<String>,
    pub metadata: serde_json::Value,
}

pub struct OperationResult {
    pub status: OperationStatus,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub duration_ms: Option<u64>,
    pub metadata: serde_json::Value,
}

pub struct Fact {
    pub id: String,
    pub fact_type: String,
    pub key: Option<String>,
    pub source: String,
    pub confidence: f64,
    pub data: serde_json::Value,
}

pub struct AgentContext {
    pub summary: String,
    pub verdict: Option<String>,
    pub intent: Option<IntentFact>,
    pub primary_facts: Vec<Fact>,
    pub recommendations: Vec<Recommendation>,
    pub meta: EnrichmentMeta,
}
```

Los adaptadores de cada host convierten sus tipos propios hacia estos tipos
genéricos. En Bastion:

```text
sandbox_run params/result
  → OperationInvocation / OperationResult
  → enrichment-engine
  → AgentContext
  → JSON MCP response
```

#### Traits de integración

El core no ejecuta comandos ni lee ficheros directamente. Usa traits:

```rust
#[async_trait]
pub trait CatalogRepository: Send + Sync {
    async fn find_matching_enrichers(
        &self,
        invocation: &OperationInvocation,
    ) -> Result<Vec<EnricherDescriptor>, EnrichmentError>;

    async fn get_descriptor(
        &self,
        id: &str,
    ) -> Result<Option<CatalogDescriptor>, EnrichmentError>;
}

#[async_trait]
pub trait FactStore: Send + Sync {
    async fn save_harness_run(&self, run: &HarnessRun) -> Result<(), EnrichmentError>;
    async fn save_facts(&self, facts: &[Fact]) -> Result<(), EnrichmentError>;
    async fn save_agent_context(&self, context: &AgentContext) -> Result<(), EnrichmentError>;
}

#[async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn execute(
        &self,
        command: CommandSpec,
        budget: CommandBudget,
    ) -> Result<CommandOutput, EnrichmentError>;
}

#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn glob(&self, pattern: &str, options: GlobOptions) -> Result<Vec<FileEntry>, EnrichmentError>;
    async fn stat(&self, path: &str) -> Result<FileStat, EnrichmentError>;
}

pub trait Policy: Send + Sync {
    fn authorize_extractor(&self, descriptor: &ExtractorDescriptor) -> Result<(), PolicyError>;
    fn authorize_command(&self, command: &CommandSpec) -> Result<(), PolicyError>;
}
```

Bastion implementa estos traits usando su provider/sandbox/SQLite. Otro proyecto
puede implementarlos con filesystem local, Docker, Kubernetes, memoria en
proceso o una base de datos distinta.

#### Features del crate

El crate debe ser modular:

```toml
[features]
default = ["yaml", "cel", "templates"]
yaml = ["serde_yaml"]
toml = ["toml"]
sqlite = ["sqlx/sqlite"]
watch = ["notify"]
cel = ["cel-interpreter"]
templates = ["minijinja"]
```

Reglas:

- `enrichment-engine` core no depende de SQLite salvo feature `sqlite`.
- El parser YAML/TOML es opcional por features.
- CEL es feature, pero Bastion lo activa por defecto.
- Watcher de ficheros es feature `watch`, no obligatorio.
- Adaptadores Bastion viven fuera del crate core.

#### Estructura propuesta

```text
crates/enrichment-engine/
├── src/
│   ├── lib.rs
│   ├── model/
│   │   ├── operation.rs
│   │   ├── fact.rs
│   │   ├── agent_context.rs
│   │   └── descriptor.rs
│   ├── catalog/
│   │   ├── repository.rs
│   │   ├── importer.rs
│   │   └── validator.rs
│   ├── extractors/
│   │   ├── regex.rs
│   │   ├── glob.rs
│   │   ├── command.rs
│   │   └── static_value.rs
│   ├── rules/
│   │   ├── cel.rs
│   │   ├── engine.rs
│   │   └── template.rs
│   ├── runtime/
│   │   ├── intent.rs
│   │   ├── pipeline.rs
│   │   ├── composer.rs
│   │   └── policy.rs
│   └── persistence/
│       ├── sqlite.rs          # feature sqlite
│       └── migrations/
```

En Bastion:

```text
crates/bastion-gateway/src/enrichment_adapter.rs
crates/bastion-infrastructure/src/catalog/enrichment_sqlite.rs
```

#### API pública mínima

El consumidor de la librería debería poder hacer:

```rust
let engine = EnrichmentEngine::builder()
    .catalog_repository(catalog_repo)
    .fact_store(fact_store)
    .command_executor(command_executor)
    .file_system(file_system)
    .policy(policy)
    .build()?;

let output = engine.enrich(invocation, result).await?;

// output.agent_context se inserta en la respuesta de la aplicación host.
```

#### Estrategia de publicación

No se debe publicar como crate externo desde el día uno.

```text
Fase 1: crate interno del workspace
Fase 2: API estabilizada por uso real en Bastion
Fase 3: documentación pública + examples independientes
Fase 4: publicación opcional en crates.io
```

Esto evita sobrediseño prematuro. El crate nace reusable, pero su API se valida
primero con Bastion.

#### Puntos flacos y mitigaciones

| Riesgo | Problema | Mitigación |
|--------|----------|------------|
| API demasiado genérica | Puede volverse abstracta e incómoda | Validarla primero con Bastion y 2 ejemplos externos pequeños |
| Acoplamiento accidental a Bastion | Tipos como `SandboxId` podrían filtrarse | Regla: core solo acepta tipos genéricos (`Operation*`, `Fact`, `AgentContext`) |
| Feature creep | Muchas features desde el inicio | MVP: yaml + cel + regex/glob + in-memory/sqlite opcional |
| Seguridad delegada al host | Cada integrador podría ejecutar comandos peligrosos | Trait `Policy` obligatorio para `command` y `run()` |
| Persistencia opcional inconsistente | Hosts sin SQLite pierden trazabilidad | `FactStore` obligatorio; puede ser in-memory, pero explícito |

### 5.6 Fact Pipeline

Para evitar un `enrichment` JSON caótico, el motor debe normalizar todo lo que
descubre como **Facts**. Un fact es una observación trazable, tipada y con
confianza.

```json
{
  "type": "artifact",
  "id": "target/app.jar",
  "source": "extractor:glob:artifacts",
  "confidence": 1.0,
  "data": {
    "name": "app.jar",
    "path": "target/app.jar",
    "size_bytes": 67108864,
    "extension": ".jar"
  }
}
```

Los facts permiten separar tres responsabilidades:

1. **Extracción**: obtener datos observables del comando y del sandbox.
2. **Razonamiento**: evaluar CEL/rules sobre datos normalizados.
3. **Composición para agente**: devolver solo contexto útil, compacto y
   priorizado.

Pipeline refinado:

```text
Intent Detector
  → Extractors
  → Fact Normalizer
  → CEL Rule Engine
  → Advice Composer
  → Agent Context Composer
```

El `agent_context` no debe construirse directamente desde extractores; debe
construirse desde facts y rules disparadas. Esto permite deduplicar, añadir
provenance, medir confianza, y depurar por qué el agente recibió un contexto.

---

## 6. Motor de Expresiones CEL y Motor de Reglas

### 6.0 Por qué regex y glob no son suficientes

Los extractores `regex` y `glob` pueden extraer datos, pero **no pueden expresar lógica condicional sobre esos datos**:

```yaml
# Esto NO se puede expresar con regex/glob:
# "Si el artefacto pesa más de 100MB, advierta"
# "Si hay tests fallidos Y warnings de deprecación"
# "Si el build tardó más de 60s Y no hay artefacto"
```

Se necesita un **motor de evaluación de expresiones** que permita:
- Comparaciones: `>`, `<`, `>=`, `<=`, `==`, `!=`
- Lógica compuesta: `&&`, `||`, `!`
- Colecciones: `exists()`, `all()`, `filter()`, `map()`
- Funciones de cadena: `contains()`, `startsWith()`, `matches()`
- Cruzar datos entre extractores

### 6.1 CEL (Common Expression Language)

**CEL** es el estándar de la industria para expresiones declarativas, usado por Kubernetes, Google Cloud, Crossplane y OPA.

**Crate Rust**: `cel-interpreter` (v0.10.0) o `cel` (v0.13.0)

**Qué ofrece CEL**:

| Categoría | Operadores / Funciones |
|-----------|------------------------|
| Comparación | `==`, `!=`, `<`, `>`, `<=`, `>=` |
| Lógica | `&&`, `\|\|`, `!`, `in`, `!in` |
| Strings | `contains()`, `startsWith()`, `endsWith()`, `matches()`, `size()` |
| Colecciones | `exists()`, `all()`, `filter()`, `map()`, `size()`, `[index]` |
| Tipo-seguro | `int`, `float`, `string`, `bool`, `bytes`, `duration`, `timestamp`, `list`, `map` |
| No Turing-completo | Sin loops, sin funciones definibles por usuario → seguro para config |

**Ejemplos de expresiones CEL**:

```cel
exit_code == 0
stdout.contains("BUILD SUCCESS")
tests.failed == 0 && tests.skipped == 0
artifacts.exists(a, a.size_bytes > 100_000_000)
artifacts.filter(a, a.extension == ".jar").size() > 0
duration_ms > 60_000 && artifacts.isEmpty()
```

### 6.2 Funciones CEL custom de Bastion

Además de las funciones estándar de CEL, se registran extensiones específicas de Bastion:

```rust
// Funciones disponibles en toda expresión CEL
glob("pattern")              // → list<string> — lista paths en sandbox
file_stat("path")            // → {size, modified, extension, ...} — metadatos
run("command")               // → {exit_code, stdout, stderr} — ejecuta en sandbox
json_parse(text)             // → map — parsea JSON
env("KEY")                  // → string — variable de entorno
human_bytes(1024)           // → "1 KB" — formatea bytes
now()                        // → timestamp — fecha/hora actual
```

**Ejemplos de uso**:

```cel
glob("target/*.jar").size() > 0
file_stat("target/app.jar").size > 100_000_000
run("git status").exit_code == 0
artifacts.filter(a, a.size > 100_000_000).map(a, a.name)
```

### 6.3 Mini Rule Engine: Reglas declarativas en YAML

CEL solo evalúa expresiones. El **mini rule engine** añade:
- **Prioridad**: orden de evaluación (`priority`, mayor = primero)
- **Grupos de exclusión mutua**: solo la primera regla que matchea en un grupo se ejecuta
- **Bloque `then`**: acciones declarativas que modifican el enrichment
- **Templates `{{ }}`**: interpolación de variables CEL en mensajes

**Schema de una regla**:

```yaml
rules:
  - id: string                  # ID único de la regla
    priority: int                # Mayor = primero (default: 0)
    group: string                # Opcional. Grupo de exclusión mutua
    when: string                 # Expresión CEL
    then:                       # Acciones si when == true
      enrichment:                # Modifica el objeto enrichment
        key: value
      advice:
        severity: string         # hint | warning | critical
        message: string          # Template {{ }} con variables CEL
```

**Templates `{{ }}`**:

```yaml
message: "Build OK. {{ artifacts[0].name }} ({{ human_bytes(artifacts[0].size) }}) ready."
message: "{{ tests.failed }} test(s) failed. Review output."
message: "Artifact {{ artifacts.filter(a, a.size > 100_000_000).map(a, a.name) }} exceeds 100MB."
```

### 6.4 Flujo de datos: Extractors → Contexto CEL → Reglas → Enrichment

```
┌──────────────────────────────────────────────────────────────────┐
│  sandbox_run(command="mvn package")                             │
└────────────────────────┬───────────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────────────┐
│  ENRICHMENT PIPELINE                                             │
│                                                                  │
│  1. EXTRACTORS → alimentan el contexto CEL                      │
│     regex tests    → context.tests = {run: 42, failed: 0}        │
│     glob artifacts → context.artifacts = [{name: "app.jar",      │
│                                           size: 67108864}]       │
│     command coords → context.maven = {group: "...", version: "1.0"} │
│                                                                  │
│  2. RULES → evaluadas contra el contexto CEL                    │
│     when: 'exit_code == 0 && tests.failed == 0'                 │
│     when: 'artifacts.exists(a, a.size > 100_000_000)'            │
│     when: 'tests.failed > 0'                                     │
│                                                                  │
│  3. THEN → producen enrichment y advice                         │
│     then: { enrichment.build_health: "healthy",                  │
│             advice: {severity: hint, message: "..."} }          │
└──────────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────────────┐
│  RESPUESTA ENRIQUECIDA                                           │
│  { exit_code: 0, stdout: "...", enrichment: {                   │
│      build_health: "healthy",                                    │
│      artifacts: [{name: "app.jar", size: 67108864}],            │
│      advice: [{severity: "hint", message: "..."}]               │
│    }}
└──────────────────────────────────────────────────────────────────┘
```

### 6.5 Evaluación de alternativas

| Opción | Pros | Contras | Veredicto |
|--------|------|---------|-----------|
| **CEL + mini rule engine (ELEGIDO)** | YAML-native, priority, grupos, CEL expressions, mantenemos nosotros | Lo mantenemos nosotros | ✅ Equilibrio correcto |
| **`rust-rule-engine` (GRL/Drools-like)** | RETE, forward/backward chaining, salience, muy potente | DSL propietario GRL (no YAML), complejidad innecesaria, overkill para nuestra escala | ❌ No cumple requisito YAML, demasiado complejo |
| **CEL puro** | Simple, estándar K8s/GCP, tipo-seguro | Sin prioridad, sin grupos, sin then-blocks, expresiones solas no bastan | ❌ Demasiado simple |
| **Rhai (scripting)** | Muy potente, Rust-native | Demasiado poder para config declarativa, curva aprendizaje alta | ❌ Overkill |
| **JSONPath** | Simple para querying | Sin lógica condicional, sin operadores | ❌ Insuficiente |

**Por qué no RETE / forward chaining**: El caso de uso de Bastion es evaluación puntual sobre un resultado, no razonamiento continuo. RETE brilla con miles de reglas reevaluadas incrementalmente. Nosotros tenemos ~10-20 reglas por ejecución, evaluadas una vez. Forward chaining (reglas disparan otras reglas) tampoco es necesario — el flujo es lineal.

**Por qué no Drools-like (`rust-rule-engine`)**: Usa su propio DSL (GRL), no configurable en YAML/TOML puro. El requisito era 100% declarativo en YAML.

---

## 6b. Tipos de Extractor (Building Blocks)

Cada tipo de extractor es una implementación Rust del trait `ExtractorBlock`. Son los bloques constructivos del pipeline post-ejecución.

**Importante**: Los extractores **alimentan el contexto CEL** — los datos extraídos se convierten en variables disponibles para las reglas mediante expresiones CEL.

### 6.1 Extractor `regex`

Extrae datos de stdout/stderr mediante expresiones regulares con grupos de captura.

**No requiere acceso al sandbox** — opera sobre la salida ya disponible en memoria.

```yaml
- type: regex
  source: stdout              # stdout | stderr
  output_key: build_status    # clave en el objeto enrichment
  pattern: "BUILD (SUCCESS|FAILURE)"
  single: true                # primera coincidencia únicamente
  fields:
    - name: status
      group: 1                # índice del grupo de captura
```

```yaml
- type: regex
  source: stdout
  output_key: tests
  pattern: 'Tests run: (\d+), Failures: (\d+), Errors: (\d+), Skipped: (\d+)'
  fields:
    - name: run
      group: 1
      type: int               # int | string (default: string)
    - name: failed
      group: 2
      type: int
    - name: errors
      group: 3
      type: int
    - name: skipped
      group: 4
      type: int
```

**Cuándo usarlo**: Extraer estados, versiones, contadores, errores — cualquier dato textual predecible en la salida.

### 6.2 Extractor `glob`

Lista archivos en el sandbox que coinciden con un patrón glob y extrae metadatos.

**Requiere acceso al sandbox** — ejecuta `find` y `stat` vía `sandbox_exec`.

```yaml
- type: glob
  pattern: "target/*.jar"
  exclude: ["*-sources.jar", "*-javadoc.jar"]
  output_key: artifacts
  max_results: 5
  fields:
    - name: name
      from: basename
    - name: path
      from: path
    - name: size_bytes
      from: size
      type: int
    - name: size_human
      from: size_human
    - name: extension
      from: extension
```

#### Campos disponibles (`from`)

| `from` | Tipo | Devuelve | Ejemplo |
|--------|------|----------|---------|
| `basename` | string | Nombre del archivo | `"petclinic-3.4.0.jar"` |
| `path` | string | Ruta relativa completa | `"target/petclinic-3.4.0.jar"` |
| `extension` | string | Extensión del archivo | `".jar"` |
| `stem` | string | Nombre sin extensión | `"petclinic-3.4.0"` |
| `parent` | string | Directorio padre | `"target"` |
| `size` | int | Tamaño en bytes | `67108864` |
| `size_human` | string | Tamaño legible | `"64 MB"` |
| `modified` | string | Última modificación ISO 8601 | `"2026-05-05T19:00:00Z"` |
| `permissions` | string | Permisos Unix | `"rw-r--r--"` |
| `is_dir` | bool | Es directorio | `false` |
| `is_executable` | bool | Es ejecutable | `true` |

#### Implementación interna (Rust)

El extractor `glob` ejecuta internamente en el sandbox:

```bash
# 1. Listar archivos que coinciden
find target -name "*.jar" -not -name "*-sources.jar" -not -name "*-javadoc.jar"

# 2. Obtener metadatos de cada uno
stat --format="%n|%s|%Y" target/petclinic-3.4.0.jar
```

Y mapea los resultados:

```rust
// Mapeo interno de stat → campos
// basename  ← path.file_name()
// size      ← stat.st_size
// modified  ← DateTime::from(stat.st_mtime)
// extension ← path.extension()
// ...
```

**Cuándo usarlo**: Descubrir artefactos generados (JARs, WARs, binarios, imágenes Docker), listar recursos producidos por el build.

### 6.3 Extractor `command`

Ejecuta un comando arbitrario en el sandbox y parsea su salida con una estrategia configurable.

**Requiere acceso al sandbox** — ejecuta un comando adicional vía `sandbox_exec`.

```yaml
- type: command
  command: "cat target/maven-archiver/pom.properties 2>/dev/null || echo ''"
  output_key: maven_coords
  parse: regex
  pattern: "groupId=(.+)\\nartifactId=(.+)\\nversion=(.+)"
  on_error: skip              # skip | fail | empty
  single: true
  fields:
    - name: group_id
      group: 1
    - name: artifact_id
      group: 2
    - name: version
      group: 3
```

#### Estrategias de parseo (`parse`)

| `parse` | Cómo parsea | Caso de uso |
|---------|-------------|-------------|
| `regex` | Aplica patrón con grupos de captura | Extraer versión, paths, coordenadas |
| `json` | Parsea la salida como JSON | `package.json`, `cargo metadata`, APIs |
| `lines` | Divide por líneas, aplica regex por línea | Tablas de salida, listas |
| `key_value` | Parsea pares `clave=valor` | `.properties`, variables de entorno |

Ejemplo con `json`:

```yaml
- type: command
  command: "cat package.json"
  output_key: npm_package
  parse: json
  fields:
    - name: name
      path: "$.name"
    - name: version
      path: "$.version"
```

Ejemplo con `key_value`:

```yaml
- type: command
  command: "cat gradle.properties"
  output_key: gradle_props
  parse: key_value
  separator: "="
  fields:
    - name: version
      key: "version"
    - name: group
      key: "group"
```

Ejemplo con `lines`:

```yaml
- type: command
  command: "ls -la target/*.jar"
  output_key: jar_listing
  parse: lines
  pattern: '^(\S+)\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+\S+\s+(\S+)\s+(.+)$'
  fields:
    - name: permissions
      group: 1
    - name: size
      group: 2
    - name: name
      group: 3
```

**Comportamiento ante errores** (`on_error`):

| Valor | Comportamiento |
|-------|---------------|
| `skip` | Si el comando falla (exit_code != 0), se omite silenciosamente — el campo no aparece en la respuesta |
| `fail` | Si el comando falla, se propaga el error y se cancela el enrichment |
| `empty` | Si el comando falla, se incluye el campo con valor `null` |

**Cuándo usarlo**: Extraer metadatos de archivos del proyecto (versiones, coordenadas, configuración), ejecutar herramientas de inspección.

### 6.4 Extractor `static`

Devuelve un valor literal para proporcionar contexto.

**No requiere acceso al sandbox**.

```yaml
- type: static
  output_key: build_tool
  value:
    name: "Apache Maven"
    version: "3.9+"
    config_file: "pom.xml"
```

**Cuándo usarlo**: Proporcionar metadatos estáticos sobre el tipo de proyecto, convenciones de la herramienta de build, o contexto que el agente necesitaría descubrir por sí mismo.

### 6.5 Tabla resumen de extractors

| Tipo | Acceso sandbox | Latencia | Caso principal |
|------|---------------|----------|---------------|
| `regex` | No | ~0ms | Parsear stdout/stderr |
| `glob` | Sí (~100-200ms) | ~200ms | Descubrir artefactos/archivos |
| `command` | Sí (~100-300ms) | ~100-300ms | Inspección personalizada |
| `static` | No | ~0ms | Contexto fijo |

---

## 7. Configuración

### 7.1 Formato del catálogo

**Decisión**: `.bastion/config.toml` (siempre TOML) declara el formato del catálogo:

```toml
[catalog]
format = "yaml"   # "yaml" | "toml"
```

El loader solo acepta archivos con la extensión configurada. Archivos con extensión incorrecta se registran como warnings y se ignoran.

Si `config.toml` no existe o no tiene la sección `[catalog]`, el valor por defecto es `"toml"` (compatibilidad hacia atrás).

### 7.2 Decisión de formato: YAML como preferido

**Razones para preferir YAML**:

1. **Legibilidad**: Estructuras anidadas profundas (arrays de objetos con campos anidados) son mucho más legibles en YAML que en TOML
2. **Sintaxis TOML para arrays anidadas**: `[[extractor.fields]]` con objetos anidados se vuelve verboso y difícil de mantener
3. **Ecosistema**: YAML es el estándar para CI/CD (GitHub Actions) y orquestación (Kubernetes)
4. **Mantenibilidad**: Los enrichers tendrán entre 50-150 líneas; YAML las hace más manejables

**Ejemplo comparativo**:

TOML (verboso):
```toml
[[enricher.extractors]]
type = "regex"
source = "stdout"
output_key = "build_status"
pattern = "BUILD (SUCCESS|FAILURE)"
single = true

[[enricher.extractors.fields]]
name = "status"
group = 1
```

YAML (legible):
```yaml
extractors:
  - type: regex
    source: stdout
    output_key: build_status
    pattern: "BUILD (SUCCESS|FAILURE)"
    single: true
    fields:
      - name: status
        group: 1
```

### 7.3 Migración

Para migrar de TOML a YAML:

1. Cambiar `format = "yaml"` en `.bastion/config.toml`
2. Renombrar archivos `.toml` → `.yaml` en el catálogo
3. Convertir el contenido (automatizable con script)

### 7.4 Estructura de directorios

```
.bastion/
├── config.toml                    # siempre TOML, declara formato
├── catalog/
│   ├── assertions/                # .yaml o .toml según config
│   │   ├── maven.build.success.yaml
│   │   └── container.alive.yaml
│   ├── doctors/
│   │   ├── sandbox.alive.yaml
│   │   └── docker.daemon.yaml
│   ├── advice/
│   │   ├── maven.build.failure.yaml
│   │   └── command.exit_code.nonzero.yaml
│   └── enrichers/                 # NUEVO
│       ├── maven.package.yaml
│       ├── npm.build.yaml
│       ├── cargo.build.yaml
│       └── go.build.yaml
```

### 7.5 Esquema YAML completo de un enricher

```yaml
schema_version: "1.0"

enricher:
  # Identificación
  id: string                    # Obligatorio. ID único del enricher
  command_pattern: string       # Obligatorio. Regex para match del comando
  description: string           # Opcional. Descripción humana
  category: string              # Opcional. Categoría para agrupación

  # Arnés semántico para agentes (lenguaje natural estructurado)
  harness:
    purpose: string              # Qué problema resuelve para el agente
    activation_intent: [string]  # Señales semánticas de cuándo aplica
    useful_context: [string]     # Qué contexto debe intentar devolver
    success_semantics: string    # Qué significa éxito en este dominio
    failure_semantics: string    # Qué significa fallo/degradación
    agent_summary_template: string

  # Activación robusta (no solo regex de comando)
  activation:
    confidence_threshold: float  # Ej: 0.70
    any:
      - command_regex: string
      - file_exists: string
      - command_contains: string
      - env_exists: string

  # Presupuesto de seguridad/rendimiento
  budget:
    max_duration_ms: int         # Tiempo total del enrichment
    max_commands: int            # Máximo command extractors/run()
    max_output_bytes: int        # Salida máxima de extractores command
    max_facts: int               # Límite de facts normalizados

  # Control de respuesta al agente
  response:
    include_agent_context: bool
    include_raw_facts: bool
    max_agent_context_bytes: int
    stdout_policy: string        # "full" | "summary" | "none"

  # PRE-ejecución: checks de infraestructura
  pre_checks:                   # Opcional. Lista de IDs de doctors
    - string                    # doctor ID (ej: "sandbox.alive")

  # POST-ejecución: validaciones
  assertions:                   # Opcional. Lista de IDs de assertions
    - string                    # assertion ID (ej: "maven.build.success")

  # POST-ejecución: extracción de datos
  extractors:                   # Opcional. Lista de extractors
    - type: string              # "regex" | "glob" | "command" | "static"
      # --- Campos comunes ---
      output_key: string        # Clave en el objeto enrichment
      shape: string             # "object" | "array"
      fact_type: string         # Tipo de fact emitido: artifact, test_result, build_status...
      confidence: float         # Confianza por defecto de los facts emitidos

      # --- regex ---
      source: string            # "stdout" | "stderr"
      pattern: string           # Patrón regex con grupos de captura
      single: bool              # Solo primera coincidencia
      fields:
        - name: string
          group: int            # Índice del grupo de captura
          type: string          # "string" | "int" | "float" | "bool"

      # --- glob ---
      pattern: string           # Patrón glob (ej: "target/*.jar")
      exclude: [string]         # Patrones a excluir
      max_results: int          # Máximo de resultados
      fields:
        - name: string
          from: string          # Ver tabla de campos `from`

      # --- command ---
      command: [string]         # Comando argv-style, sin shell por defecto
      allow_shell: bool         # Default false. Solo catálogos trusted
      mode: string              # "read_only" | "inspect" | "trusted"
      parse: string             # "regex" | "json" | "lines" | "key_value"
      pattern: string           # Patrón (para regex/lines)
      on_error: string          # "skip" | "fail" | "empty"
      single: bool
      fields:
        - name: string
          group: int            # Para regex/lines
          path: string          # Para json (JSONPath)
          key: string           # Para key_value

      # --- static ---
      value: object             # Valor literal

  # POST-ejecución: advice contextual
  advice_scope: string          # Opcional. Categoría para filtrar advice

  # POST-ejecución: reglas CEL sobre facts/context
  rules:
    - id: string
      priority: int
      group: string
      when: string              # Expresión CEL
      then:
        facts: object           # Facts derivados
        enrichment: object      # Campos añadidos al enrichment
        advice: object          # Advice inline normalizado
```

---

## 8. Cómo contribuye cada pieza del catálogo

| Pieza | Antes (on-demand) | Después (proactivo con CEL) |
|-------|-------------------|----------------------------|
| **Doctor** | Agente llama `doctor_run("sandbox.alive")` | Pre-check automático si el enricher lo referencia en `pre_checks` |
| **Assertion** | Agente llama `assertion_run("maven.build.success")` | Auto-evaluado post-ejecución si el enricher lo referencia en `assertions`. **Evoluciona a CEL**: checks son expresiones CEL |
| **Pattern** (nuevo) | No existía | `regex` extractor dentro del enricher — alimenta contexto CEL |
| **Artifact** (nuevo) | No existía | `glob` extractor dentro del enricher — alimenta contexto CEL |
| **Advice** | Agente llama `advice_suggest(...)` | Auto-generado desde **rules** (CEL + then-blocks) y filtrado por `advice_scope` del catálogo |
| **Rules** (nuevo) | No existía | Evaluadas contra el contexto CEL, generan enrichment y advice inline |

### 8.1 Evolución del catálogo existente con CEL

Los componentes del catálogo **no desaparecen**, pero evolucionan para usar **CEL conditions** en vez de tipos fijos de checks.

**Assertion — antes**:

```toml
# Formato actual (tipos fijos)
[[assertion.checks]]
type = "exit_code"
expected = 0

[[assertion.checks]]
type = "stdout_contains"
substring = "BUILD SUCCESS"
```

**Assertion — después (con CEL)**:

```yaml
# Nuevo formato con CEL (retrocompatible)
checks:
  - when: 'exit_code == 0'
    message: "Exit code is 0"
  - when: 'stdout.contains("BUILD SUCCESS")'
    message: "Build succeeded"
  - when: 'exit_code == 0 && stdout.contains("BUILD SUCCESS") && tests.failed == 0'
    message: "Full build success"
```

**Doctor — antes**:

```toml
[[doctor.checks]]
type = "aliveness"
```

**Doctor — después (con CEL)**:

```yaml
checks:
  - when: 'run("podman ps").exit_code == 0'
    message: "Podman daemon is running"
  - when: 'file_stat("/var/run/podman.sock").exists == true'
    message: "Podman socket exists"
  - when: 'run("git --version").exit_code == 0'
    message: "Git is available"
```

**Advice — antes**:

```toml
[[advice.triggers]]
type = "assertion_failed"
assertion_id = "maven.build.success"
```

**Advice — después (con CEL)**:

```yaml
# El advice del catálogo sigue funcionando con sus triggers,
# pero ahora las rules del enricher también generan advice inline:
triggers:
  - when: 'assertions.exists(a, a.id == "maven.build.success" && !a.passed)'
    message: "Maven build failed. Review compilation errors."
    severity: warning
```

**Importante**: Las herramientas MCP existentes (`assertion_run`, `advice_suggest`, `doctor_run`) se **mantienen** para uso manual, debugging y casos no cubiertos por enrichers. El formato de los TOML existentes es retrocompatible — se añade soporte para `when` (CEL) alongside `type` (legacy).

---

## 9. Integración en el código existente

### 9.1 Punto de activación

El enrichment se activa exactamente donde `record_experience` se invoca hoy en `server.rs`.

**Código actual** (`server.rs:646-667`):

```rust
Ok(result) => {
    let duration_us = t0.elapsed().as_micros() as u64;
    self.gateway_config.metrics.record_command(duration_us);

    experience = experience
        .with_stdout(&result.stdout)
        .with_stderr(&result.stderr)
        .completed(result.exit_code);
    if result.timed_out {
        experience = experience.timed_out();
    }
    self.record_experience(experience).await;

    serde_json::json!({
        "exit_code": result.exit_code,
        "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
        "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
        "duration_ms": result.duration_ms,
        "timed_out": result.timed_out
    })
    .to_string()
}
```

**Código futuro (con enrichment)**:

```rust
Ok(result) => {
    let duration_us = t0.elapsed().as_micros() as u64;
    self.gateway_config.metrics.record_command(duration_us);

    experience = experience
        .with_stdout(&result.stdout)
        .with_stderr(&result.stderr)
        .completed(result.exit_code);
    if result.timed_out {
        experience = experience.timed_out();
    }
    self.record_experience(experience).await;

    // === NUEVO: Enrichment Engine ===
    let enrichment = self.run_enrichers(
        &params.command,
        &result,
        &sandbox_id
    ).await;

    let mut response = serde_json::json!({
        "exit_code": result.exit_code,
        "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
        "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
        "duration_ms": result.duration_ms,
        "timed_out": result.timed_out
    });

    if !enrichment.is_empty() {
        response["enrichment"] = enrichment;
    }

    response.to_string()
}
```

### 9.2 Método `run_enrichers`

```rust
impl BastionGateway {
    async fn run_enrichers(
        &self,
        command: &str,
        result: &CommandResult,
        sandbox_id: &SandboxId,
    ) -> serde_json::Value {
        let Some(ref registry) = self.enricher_registry else {
            return serde_json::Value::Null;
        };

        let Some(enricher) = registry.find_matching(command) else {
            return serde_json::Value::Null;
        };

        let ctx = PostRunContext {
            command: command.to_string(),
            sandbox_id: sandbox_id.clone(),
            exit_code: result.exit_code,
            stdout: String::from_utf8_lossy(&result.stdout).to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).to_string(),
            sandbox_fs: self.provider.clone(),
        };

        match enricher.enrich(&ctx).await {
            Ok(enrichments) => {
                let mut map = serde_json::Map::new();
                map.insert("enricher_id".into(), serde_json::Value::String(enricher.id().into()));
                for e in enrichments {
                    map.insert(e.key, e.value);
                }
                serde_json::Value::Object(map)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Enrichment failed, returning raw result");
                serde_json::Value::Null
            }
        }
    }
}
```

### 9.3 Flujo de pre-checks

Los pre-checks (doctors) se ejecutan **antes** del comando. Si un doctor falla, se devuelve una respuesta inmediata sin ejecutar el comando:

```rust
// PRE-checks: si hay enricher que coincide, ejecutar doctors
if let Some(ref registry) = self.enricher_registry {
    if let Some(enricher) = registry.find_matching(&params.command) {
        let pre_ctx = PreRunContext {
            sandbox_id: sandbox_id.clone(),
            provider: self.provider.as_ref(),
        };
        match enricher.run_pre_checks(&pre_ctx).await {
            Ok(()) => { /* continuar con ejecución */ }
            Err(failed) => {
                return serde_json::json!({
                    "exit_code": -1,
                    "error": "Pre-check failed",
                    "enrichment": {
                        "enricher_id": enricher.id(),
                        "pre_checks": failed,
                    }
                }).to_string();
            }
        }
    }
}
```

### 9.4 Ubicación en el crate

Los nuevos componentes se organizarían en:

```
crates/
├── bastion-domain/
│   └── src/catalog/
│       ├── experience.rs          # EXISTENTE
│       ├── assertion.rs           # EXISTENTE
│       ├── doctor.rs              # EXISTENTE
│       ├── advice.rs              # EXISTENTE
│       ├── enricher.rs            # NUEVO: traits y tipos
│       └── extractor.rs           # NUEVO: ExtractorBlock trait
│
├── bastion-infrastructure/
│   └── src/catalog/
│       ├── toml_assertion_parser.rs  # EXISTENTE
│       ├── toml_doctor_parser.rs     # EXISTENTE
│       ├── toml_advice_parser.rs     # EXISTENTE
│       ├── enricher_parser.rs        # NUEVO: YAML/TOML parser
│       └── extractors/
│           ├── mod.rs                # NUEVO
│           ├── regex_extractor.rs    # NUEVO
│           ├── glob_extractor.rs     # NUEVO
│           ├── command_extractor.rs  # NUEVO
│           └── static_extractor.rs   # NUEVO
│
└── bastion-gateway/
    └── src/
        └── server.rs                 # MODIFICADO: hook de enrichment
```

---

## 10. Rendimiento

### 10.1 Análisis de overhead

| Operación | Latencia | Notas |
|-----------|----------|-------|
| Pattern matching (regex compilado) | ~0.01ms | Regex pre-compilados en el registry |
| Regex extraction sobre stdout | ~0.1-1ms | Operación en memoria, sin I/O |
| Glob (find + stat en sandbox) | ~100-200ms | 1-2 llamadas `sandbox_exec` |
| Command extractor | ~100-300ms | 1 llamada `sandbox_exec` + parseo |
| Assertion evaluation | ~0.1ms | Evaluación en memoria |
| Advice generation | ~0.1ms | Matching de triggers en memoria |

### 10.2 Zero overhead si no hay match

Si ningún enricher coincide con el comando, el coste es una única evaluación de regex compilado (~0.01ms). El flujo es idéntico al actual.

### 10.3 Comparativa: ahorro neto

| Métrica | Sin enrichment (4-5 llamadas extra) | Con enrichment |
|---------|-------------------------------------|----------------|
| Llamadas LLM | 5-6 | **1** |
| Latencia total | 10-30s | **~200-500ms extra** |
| Tokens | 2.000-10.000 extra | **0 extras** |
| Coste | 4-5x | **1x + ~200ms server** |

### 10.4 Estrategias de optimización

1. **Regex compilados**: Pre-compilar todos los `command_pattern` y `pattern` de extractors al cargar el registry
2. **Lazy extraction**: Los extractors `glob` y `command` solo se ejecutan si los pre-checks pasan y los assertions son relevantes
3. **Timeout**: Cada extractor tiene un timeout configurable (default: 5s). Si un extractor excede el timeout, se omite y se registra un warning
4. **Cache de metadatos**: Para extractores `glob` que se ejecutan frecuentemente, cachear resultados por `sandbox_id + path` con TTL de 30s

### 10.5 Seguridad y budgets

El Enrichment Engine reduce llamadas al LLM, pero puede añadir ejecución
automática dentro del sandbox. Por eso todo enricher debe operar bajo budgets y
políticas explícitas.

```yaml
budget:
  max_duration_ms: 500
  max_commands: 2
  max_output_bytes: 16384
  max_facts: 50

security:
  trust_level: project          # builtin | project | workspace | remote | untrusted
  allow_shell: false
  command_mode: read_only
```

Política recomendada:

| Trust level | regex | glob | command | `run()` CEL |
|-------------|-------|------|---------|-------------|
| `builtin` | sí | sí | sí | sí |
| `project` | sí | sí | sí, sin shell | no por defecto |
| `workspace` | sí | sí | allowlist | no |
| `remote` | sí | limitado | no | no |
| `untrusted` | sí | no | no | no |

Reglas no negociables:

1. Los comandos de extractor deben usar formato argv (`["cat", "file"]`) por
   defecto, no shell string.
2. `allow_shell: true` solo se permite en catálogos `builtin` o explícitamente
   trusted.
3. Todo `command` extractor debe tener timeout y límite de output.
4. Si el enrichment falla, la respuesta raw de `sandbox_run` sigue siendo válida.
5. El `harness` en lenguaje natural nunca ejecuta nada directamente.

### 10.6 Control de tamaño de respuesta

La respuesta enriquecida debe evitar devolver datos que aumenten innecesariamente
tokens. El objetivo es **contexto útil**, no más ruido.

```yaml
response:
  stdout_policy: summary       # full | summary | none
  max_stdout_bytes: 4096
  include_agent_context: true
  include_raw_facts: false
  max_agent_context_bytes: 8192
  max_enrichment_meta_bytes: 2048
```

Por defecto, el agente debe recibir:

- `agent_context.summary`
- `agent_context.verdict`
- artefactos principales,
- tests/resultados clave,
- recomendaciones ordenadas,
- `enrichment_meta` compacto.

Los facts crudos se devuelven solo si el agente o la configuración los pide.

---

## 11. Ejemplos completos

### 11.1 Maven Package

```yaml
# .bastion/catalog/enrichers/maven.package.yaml
schema_version: "1.0"

enricher:
  id: maven.package
  command_pattern: "^mvn\\s+(package|install|verify|compile)"
  description: "Maven build enrichment: artefactos, tests, estado del build"
  category: maven

  harness:
    purpose: >
      Help the agent understand a Maven build in one response: verdict,
      generated artifacts, test result, Maven coordinates, and next step.
    activation_intent:
      - The command builds or packages a Java/Maven project.
      - Commands may be mvn package, mvn verify, mvn install, or ./mvnw package.
      - Presence of pom.xml increases confidence.
    useful_context:
      - Build verdict and confidence.
      - Primary generated JAR/WAR artifact with path, size, and extension.
      - Maven groupId, artifactId, and version when available.
      - Test counters and failure summary.
      - Recommended next step.
    success_semantics: >
      A Maven package run is successful when exit_code is 0, BUILD SUCCESS is
      present, and no test failures are detected. If a deployable artifact is
      generated, expose it as the primary artifact.
    failure_semantics: >
      Failure is indicated by non-zero exit code, BUILD FAILURE, test failures,
      or missing expected artifacts after a package command.
    agent_summary_template: >
      Maven build {{ verdict }}. Primary artifact: {{ primary_artifact.path }}
      ({{ primary_artifact.size_human }}).

  activation:
    confidence_threshold: 0.70
    any:
      - command_regex: "(^|\\s)(mvn|./mvnw)\\s+"
      - command_contains: "mvn package"
      - file_exists: "pom.xml"

  budget:
    max_duration_ms: 500
    max_commands: 2
    max_output_bytes: 16384
    max_facts: 50

  response:
    include_agent_context: true
    include_raw_facts: false
    stdout_policy: summary
    max_agent_context_bytes: 8192

  pre_checks:
    - sandbox.alive

  assertions:
    - maven.build.success

  extractors:
    - type: regex
      source: stdout
      output_key: build_status
      pattern: "BUILD (SUCCESS|FAILURE)"
      single: true
      fields:
        - name: status
          group: 1

    - type: regex
      source: stdout
      output_key: tests
      pattern: 'Tests run: (\d+), Failures: (\d+), Errors: (\d+), Skipped: (\d+)'
      fields:
        - name: run
          group: 1
          type: int
        - name: failed
          group: 2
          type: int
        - name: errors
          group: 3
          type: int
        - name: skipped
          group: 4
          type: int

    - type: glob
      pattern: "target/*.jar"
      exclude: ["*-sources.jar", "*-javadoc.jar"]
      output_key: artifacts
      max_results: 5
      fields:
        - name: name
          from: basename
        - name: path
          from: path
        - name: size_bytes
          from: size
          type: int
        - name: size_human
          from: size_human
        - name: extension
          from: extension

    - type: command
      command: "cat target/maven-archiver/pom.properties 2>/dev/null || echo ''"
      output_key: maven_coords
      parse: regex
      pattern: "groupId=(.+)\\nartifactId=(.+)\\nversion=(.+)"
      on_error: skip
      single: true
      fields:
        - name: group_id
          group: 1
        - name: artifact_id
          group: 2
        - name: version
          group: 3

  # Rules: evaluadas contra el contexto CEL alimentado por los extractors
  rules:
    - id: build_success
      priority: 100
      when: 'exit_code == 0 && build_status.status == "SUCCESS"'
      then:
        enrichment:
          build_health: healthy
        advice:
          severity: hint
          message: "Build OK. {{ artifacts[0].name }} ({{ human_bytes(artifacts[0].size_bytes) }}) ready."

    - id: build_failure
      priority: 95
      group: build_verdict
      when: 'exit_code != 0 || build_status.status == "FAILURE"'
      then:
        enrichment:
          build_health: failed
        advice:
          severity: critical
          message: "Build failed. Review compilation errors above."

    - id: build_success_verdict
      priority: 90
      group: build_verdict
      when: 'exit_code == 0 && build_status.status == "SUCCESS"'
      then:
        enrichment:
          build_health: success

    - id: tests_healthy
      priority: 80
      when: 'tests.exists(t, t.failed == 0) && tests.size() > 0'
      then:
        enrichment:
          tests_status: passed

    - id: tests_failing
      priority: 85
      when: 'tests.exists(t, t.failed > 0)'
      then:
        enrichment:
          tests_status: failed
        advice:
          severity: warning
          message: "{{ tests.map(t, t.failed) }} test(s) failed. Review test output."

    - id: artifact_large
      priority: 70
      when: 'artifacts.exists(a, a.size_bytes > 100_000_000)'
      then:
        advice:
          severity: warning
          message: "Artifact exceeds 100MB. Consider optimizing dependencies or using shading."

    - id: artifact_detected
      priority: 60
      when: 'artifacts.size() > 0'
      then:
        enrichment:
          artifact_count: artifacts.size()
          total_size_bytes: artifacts.map(a, a.size_bytes).sum()
          total_size_human: human_bytes(artifacts.map(a, a.size_bytes).sum())
```

**Respuesta enriquecida generada**:

```json
{
  "exit_code": 0,
  "stdout": "...",
  "stderr": "",
  "duration_ms": 12345,
  "enrichment": {
    "enricher_id": "maven.package",
    "assertions": [
      { "id": "maven.build.success", "passed": true }
    ],
    "build_status": { "status": "SUCCESS" },
    "tests": { "run": 42, "failed": 0, "errors": 0, "skipped": 0 },
    "artifacts": [{
      "name": "petclinic-3.4.0.jar",
      "path": "target/petclinic-3.4.0.jar",
      "size_bytes": 67108864,
      "size_human": "64 MB",
      "extension": ".jar"
    }],
    "maven_coords": {
      "group_id": "org.springframework.samples",
      "artifact_id": "spring-petclinic",
      "version": "3.4.0"
    },
    "advice": [{
      "message": "Build OK. Artifact: petclinic-3.4.0.jar (64 MB). Ready to deploy.",
      "severity": "hint"
    }]
  }
}
```

### 11.2 NPM Build

```yaml
# .bastion/catalog/enrichers/npm.build.yaml
schema_version: "1.0"

enricher:
  id: npm.build
  command_pattern: "^npm\\s+(run\\s+build|run\\s+prod|build)"
  description: "NPM build enrichment: artefactos, bundle size, warnings"
  category: npm

  pre_checks:
    - sandbox.alive

  assertions:
    - command.exit_code.zero

  extractors:
    - type: regex
      source: stderr
      output_key: warnings
      pattern: "WARNING in (.+)"
      fields:
        - name: message
          group: 1

    - type: regex
      source: stdout
      output_key: build_time
      pattern: "compiled successfully in (\\d+) ms"
      single: true
      fields:
        - name: ms
          group: 1
          type: int

    - type: glob
      pattern: "dist/**/*.js"
      exclude: ["*.map", "*.LICENSE"]
      output_key: artifacts
      max_results: 10
      fields:
        - name: name
          from: basename
        - name: path
          from: path
        - name: size_bytes
          from: size
          type: int
        - name: size_human
          from: size_human
        - name: extension
          from: extension

    - type: command
      command: "cat package.json 2>/dev/null || echo '{}'"
      output_key: package_info
      parse: json
      on_error: empty
      fields:
        - name: name
          path: "$.name"
        - name: version
          path: "$.version"

  advice_scope: npm
```

### 11.3 Cargo Build

```yaml
# .bastion/catalog/enrichers/cargo.build.yaml
schema_version: "1.0"

enricher:
  id: cargo.build
  command_pattern: "^cargo\\s+(build|release|check|clippy)"
  description: "Cargo/Rust build enrichment: binarios, warnings, test results"
  category: cargo

  pre_checks:
    - sandbox.alive

  assertions:
    - command.exit_code.zero

  extractors:
    - type: regex
      source: stdout
      output_key: warnings
      pattern: "warning: (.+)"
      fields:
        - name: message
          group: 1

    - type: regex
      source: stdout
      output_key: compilation
      pattern: "Compiling (.+) v(.+)"
      fields:
        - name: crate_name
          group: 1
        - name: version
          group: 2

    - type: regex
      source: stderr
      output_key: errors
      pattern: "error(?:\\[E\\d+\\])?: (.+)"
      fields:
        - name: message
          group: 1

    - type: glob
      pattern: "target/release/*"
      exclude:
        - "*.d"
        - "*.rlib"
        - "*.pdb"
        - "build/**"
        - "deps/**"
        - ".fingerprint/**"
        - "examples/**"
        - "incremental/**"
      output_key: binaries
      max_results: 5
      fields:
        - name: name
          from: basename
        - name: path
          from: path
        - name: size_bytes
          from: size
          type: int
        - name: size_human
          from: size_human
        - name: is_executable
          from: is_executable
          type: bool

    - type: command
      command: "cat Cargo.toml 2>/dev/null || echo ''"
      output_key: cargo_info
      parse: key_value
      separator: "="
      on_error: skip
      fields:
        - name: name
          key: "name"
        - name: version
          key: "version"
        - name: edition
          key: "edition"

  advice_scope: cargo
```

### 11.4 Go Build

```yaml
# .bastion/catalog/enrichers/go.build.yaml
schema_version: "1.0"

enricher:
  id: go.build
  command_pattern: "^go\\s+(build|install|test)"
  description: "Go build enrichment: binarios, test results, module info"
  category: go

  pre_checks:
    - sandbox.alive

  assertions:
    - command.exit_code.zero

  extractors:
    - type: regex
      source: stdout
      output_key: test_results
      pattern: "^(ok|FAIL)\\s+(\\S+)\\s+(\\S+)(?:\\s+(.+))?$"
      fields:
        - name: status
          group: 1
        - name: package
          group: 2
        - name: duration
          group: 3

    - type: command
      command: "cat go.mod 2>/dev/null | head -2 || echo ''"
      output_key: module_info
      parse: regex
      pattern: "module (\\S+)\\ngo (\\S+)"
      on_error: skip
      single: true
      fields:
        - name: module
          group: 1
        - name: go_version
          group: 2

    - type: glob
      pattern: "*.exe"
      output_key: binaries
      max_results: 5
      fields:
        - name: name
          from: basename
        - name: path
          from: path
        - name: size_bytes
          from: size
          type: int
        - name: size_human
          from: size_human
        - name: is_executable
          from: is_executable
          type: bool

  advice_scope: go
```

---

## 12. Roadmap

### Fase 1: Infraestructura de Enrichment (MVP)

**Objetivo**: Primera respuesta enriquecida funcional con catálogo importado a
SQLite y consulta runtime desde DB.

**Entregables**:

- Tipos de dominio: `EnricherDescriptor`, `HarnessDescriptor`, `ExtractionContext`, `Fact`, `AgentContext`
- Crate interno `crates/enrichment-engine` con API host-agnostic
- Adaptador Bastion separado en gateway/infrastructure
- Trait `ExtractorBlock` + implementaciones para `regex` y `glob`
- `EnricherRegistry` respaldado por `CatalogRepository(SQLite)`
- Parser/importer YAML/TOML → descriptors normalizados en SQLite
- Detección de formato de catálogo desde `.bastion/config.toml`
- Hash de ficheros y reimportación si cambian
- Integración en la respuesta de `sandbox_run` en `server.rs`
- Tests unitarios de extractors y registry

**Criterio de aceptación**: Al ejecutar `sandbox_run("mvn package")` con un
enricher `maven.package.yaml` importado a SQLite, la respuesta incluye
`agent_context` con verdict, build_status y artifacts. El runtime no reparsea
el fichero YAML durante la llamada.

### Fase 2: Building Blocks completos

**Objetivo**: Todos los tipos de extractor operativos bajo budgets y seguridad.

**Entregables**:

- Extractor `command` (sandbox exec + parseo)
- Extractor `static`
- Estrategias de parseo: `json`, `key_value`, `lines`
- Timeout configurable por extractor
- Manejo de errores granular (`on_error`: skip/fail/empty)
- `shape: object|array`, `fact_type`, `confidence`
- `command` argv-style, `allow_shell: false` por defecto
- Persistencia de `harness_runs`, `facts`, `rule_firings`, `agent_contexts`

**Criterio de aceptación**: Un enricher puede combinar los 4 tipos de extractor en un solo pipeline.

### Fase 3: Primeros Enrichers de producción

**Objetivo**: Enrichers listos para uso real.

**Entregables**:

- `maven.package.yaml` (enrichment completo de Maven)
- `npm.build.yaml` (enrichment de builds NPM)
- `cargo.build.yaml` (enrichment de builds Cargo/Rust)
- `go.build.yaml` (enrichment de builds Go)
- Pre-checks (doctors) integrados en el pipeline
- Harness semántico completo en cada enricher
- CEL rules y advice automático basado en facts
- `catalog_validate` y `catalog_test` con fixtures

**Criterio de aceptación**: Un agente puede construir un proyecto Java con Maven recibiendo toda la información útil en una sola respuesta, sin llamadas extra.

### Fase 4: Enrichment para otras herramientas

**Objetivo**: Extender el enrichment más allá de `sandbox_run`.

**Entregables**:

- Enrichment para `sandbox_prepare` (detectar tipo de proyecto, sugerir comandos de build)
- Enrichment para `sandbox_sync` (manifiesto de artefactos, checksums)
- Enrichment para `sandbox_run_stream` (enrichment en streaming)
- Soporte para múltiples enrichers por comando (merge de resultados)
- Persistencia de `utility_events` para medir si el agente usó el contexto

**Criterio de aceptación**: El flujo prepare → run → sync ofrece enrichment coherente en cada fase.

### Fase 5: Comunidad y Extensibilidad

**Objetivo**: Ecosistema de enrichers.

**Entregables**:

- Pipeline de mejora: agentes pueden proponer nuevos enrichers basados en su experiencia
- Marketplace/repo compartido de enrichers
- Catalog packs estilo `Agents.md`: convenciones, harness, extractors, advice y fixtures empaquetados
- Extractores personalizados via plugins WASM
- Telemetría: qué enrichers se usan más, qué extractores fallan, ratio de acierto

**Criterio de aceptación**: Un usuario puede instalar un enricher de la comunidad con un solo comando.

### Fase 6: Meta-Harness Optimizer

**Objetivo**: Optimizar la estructura del arnés usando trazas reales, no solo
búsqueda semántica.

Inspiración: Meta-Harness / DSPy. No se ejecuta en el hot path del gateway; es
un proceso offline o explícito.

**Entregables**:

- Métricas de utilidad: llamadas posteriores evitadas, uso de artifact paths,
  alineación con `next_recommended`, retries evitados.
- Evaluador de harness runs persistidos en SQLite.
- Propuestas automáticas: ajustar activation, añadir extractor, modificar rule,
  mejorar summary template.
- `catalog_proposals` persistidas y revisables.
- `catalog_diff`, `catalog_validate`, `catalog_test`, `catalog_apply`.

**Criterio de aceptación**: A partir de 20 trazas Maven reales, Bastion puede
proponer una mejora de enricher validada contra fixtures sin modificar el
catálogo automáticamente.

### Fase 7: Externalización del crate

**Objetivo**: Convertir `enrichment-engine` en librería reusable fuera de
Bastion, si la API se estabiliza.

**Entregables**:

- Ejemplos standalone sin Bastion: filesystem local, CI runner, Docker adapter.
- Documentación pública de API.
- Feature flags revisadas.
- Política de versionado semver.
- Benchmarks y test fixtures independientes.

**Criterio de aceptación**: Un proyecto Rust externo puede usar el crate para
enriquecer el resultado de una operación local sin depender de Bastion ni MCP.

---

## 13. Decisiones de Diseño

| # | Decisión | Opción elegida | Alternativas descartadas | Razón |
|---|----------|---------------|--------------------------|-------|
| D1 | **Modelo de activación** | Pattern matching por regex sobre el comando | Hook por tipo de herramienta, callback genérico | El regex es declarativo, configurable por YAML, y no requiere código Rust nuevo para cada caso |
| D2 | **Formato de configuración** | YAML (configurable a TOML) | Solo TOML, solo YAML, JSON | YAML es más legible para estructuras anidadas profundas. TOML se mantiene como opción para proyectos existentes |
| D3 | **Ubicación de la configuración** | `.bastion/config.toml` para formato del catálogo | Variable de entorno, argumento CLI | Unificado con la configuración existente de Bastion. Siempre TOML para el config raíz |
| D4 | **Relación con catálogo existente** | Referencia por ID, no reemplazo | Migrar todo a enrichers, duplicar lógica | Los componentes existentes funcionan y se usan manualmente. El enricher los orquesta sin duplicar |
| D5 | **Herramientas MCP existentes** | Se mantienen para uso manual | Deprecar, eliminar | Necesarias para debugging, casos no cubiertos, y uso explícito cuando el agente lo quiere |
| D6 | **Comportamiento ante fallo** | Graceful degradation: si un extractor falla, se omite; la respuesta raw siempre se devuelve | Fail-fast, error fatal | El agente nunca debe recibir un error por culpa del enrichment. La respuesta sin enrichment siempre es válida |
| D7 | **Extractores como traits** | `ExtractorBlock` trait con implementaciones por tipo | Enum con match, macros | Permite añadir nuevos tipos de extractor sin modificar código existente (Open/Closed Principle) |
| D8 | **Ejecución de extractors** | Secuencial (por ahora) | Paralelo | El caso común tiene 2-4 extractors. El overhead de paralelización (spawn tasks) no compensa para pocos extractors. Se puede paralelizar en Fase 4 si se necesita |
| D9 | **Pre-checks antes del comando** | Sí, ejecutar doctors referenciados antes | Solo post-ejecución | Si el sandbox está muerto, ejecutar el comando es inútil y desperdicia tiempo. Mejor fallar rápido |
| D10 | **Advice automático** | Advice normalizado desde rules inline + catálogo filtrado por scope | Solo advice embebido, solo advice externo | Rules permiten contexto rico; el catálogo conserva consejos reutilizables. Ambos se deduplican y ordenan |
| D11 | **Timeout de extractors** | Configurable, default 5s | Sin timeout, timeout global | Un extractor `command` colgado no debe bloquear toda la respuesta. Timeout individual permite control granular |
| D12 | **Orden de evaluación** | Intent → Extractors → Facts → CEL Rules/Assertions → Agent Context | Assertions antes de extractors | Los extractors alimentan facts/context; rules y assertions necesitan esos datos |
| D13 | **Motor de expresiones** | CEL (Common Expression Language) | Regex puro, Drools/GRL, Rhai, custom DSL | CEL es estándar K8s/GCP, tipo-seguro, no Turing-completo, y existen crates Rust maduros. Regex puro es insuficiente para lógica condicional |
| D14 | **Motor de reglas** | Mini rule engine propio (CEL + YAML rules) | rust-rule-engine (GRL/Drools-like), Drools, OPA/Rego | DROOLS: usa DSL propietario (GRL), no 100% YAML, overkill. OPA: excesivamente complejo para el caso. Mini rule engine: YAML-native, priority, groups, then-blocks, mantenemos control |
| D15 | **Funciones CEL custom** | Registradas como extensiones CEL en Rust | Funciones embebidas en YAML | Permiten glob(), file_stat(), run(), human_bytes(), etc. Requieren Rust pero se registran una vez |
| D16 | **Rules como fuente de advice** | Rules generan advice directamente + advice_scope para filtrar del catálogo | Solo advice del catálogo, solo rules | Dual: rulesdan advice inline para lógica rica; advice_scope filtra consejos del catálogo existente |
| D17 | **Grupos de exclusión mutua** | Rules en mismo `group`: solo la primera que matchea se ejecuta | Todas las rules matchean, se evaluán todas | Evitaadvice contradictorios. Ej: grupo `build_verdict` solo produce un veredicto |
| D18 | **Persistencia runtime** | SQLite es fuente de consulta de la aplicación | Leer ficheros en cada request | Permite consultas rápidas, auditoría, trazas, métricas y recuperación entre sesiones |
| D19 | **Ficheros de catálogo** | Formato de autoría/versionado sincronizado a SQLite por hash | Solo DB, solo ficheros | Git-friendly para humanos/agentes, pero runtime estable y rápido desde DB |
| D20 | **Arnés semántico** | Lenguaje natural estructurado + ejecución determinista | Solo YAML rígido, solo reglas CEL | Captura propósito/utilidad/intención para agentes, pero no ejecuta lógica insegura |
| D21 | **Fact Pipeline** | Extractores emiten Facts tipados con source/confidence | Enrichment JSON libre | Mejora trazabilidad, deduplicación, depuración y composición compacta para el agente |
| D22 | **Optimización Meta-Harness** | Offline, basada en harness_runs y utility_events | En hot path del gateway | Optimiza estructura sin añadir latencia ni riesgo en ejecución normal |
| D23 | **Forma de implementación** | Crate reusable host-agnostic `enrichment-engine` + adaptador Bastion | Código embebido en bastion-gateway | Reutilizable, testeable, desacoplado de MCP/Podman, publicable si madura |
| D24 | **Publicación del crate** | Interno primero, crates.io después si se estabiliza | Publicar desde el día uno | Evita sobrediseño y permite validar API con Bastion antes |

---

## 14. Apéndice: Esquema TOML completo

Para proyectos que prefieran TOML (formato por defecto si no se configura):

```toml
# .bastion/catalog/enrichers/maven.package.toml
schema_version = "1.0"

[enricher]
id = "maven.package"
command_pattern = "^mvn\\s+(package|install|verify|compile)"
description = "Maven build enrichment: artefactos, tests, estado del build"
category = "maven"

pre_checks = ["sandbox.alive"]
assertions = ["maven.build.success"]
advice_scope = "maven"

# --- Extractor: regex build_status ---

[[enricher.extractors]]
type = "regex"
source = "stdout"
output_key = "build_status"
pattern = "BUILD (SUCCESS|FAILURE)"
single = true

[[enricher.extractors.fields]]
name = "status"
group = 1

# --- Extractor: regex tests ---

[[enricher.extractors]]
type = "regex"
source = "stdout"
output_key = "tests"
pattern = 'Tests run: (\d+), Failures: (\d+), Errors: (\d+), Skipped: (\d+)'

[[enricher.extractors.fields]]
name = "run"
group = 1
type = "int"

[[enricher.extractors.fields]]
name = "failed"
group = 2
type = "int"

[[enricher.extractors.fields]]
name = "errors"
group = 3
type = "int"

[[enricher.extractors.fields]]
name = "skipped"
group = 4
type = "int"

# --- Extractor: glob artifacts ---

[[enricher.extractors]]
type = "glob"
pattern = "target/*.jar"
exclude = ["*-sources.jar", "*-javadoc.jar"]
output_key = "artifacts"
max_results = 5

[[enricher.extractors.fields]]
name = "name"
from = "basename"

[[enricher.extractors.fields]]
name = "path"
from = "path"

[[enricher.extractors.fields]]
name = "size_bytes"
from = "size"
type = "int"

[[enricher.extractors.fields]]
name = "size_human"
from = "size_human"

[[enricher.extractors.fields]]
name = "extension"
from = "extension"

# --- Extractor: command maven_coords ---

[[enricher.extractors]]
type = "command"
command = "cat target/maven-archiver/pom.properties 2>/dev/null || echo ''"
output_key = "maven_coords"
parse = "regex"
pattern = "groupId=(.+)\\nartifactId=(.+)\\nversion=(.+)"
on_error = "skip"
single = true

[[enricher.extractors.fields]]
name = "group_id"
group = 1

[[enricher.extractors.fields]]
name = "artifact_id"
group = 2

[[enricher.extractors.fields]]
name = "version"
group = 3
```

**Comparación de verbosidad**: La versión TOML tiene ~90 líneas vs ~65 líneas en YAML para el mismo enricher. La diferencia crece con más extractors y campos anidados.

---

## 15. Glosario

| Término | Definición |
|---------|-----------|
| **Enricher** | Configuración que orquesta un pipeline de enrichment, activada por coincidencia de patrón de comando |
| **Extractor** | Bloque constructivo que extrae datos de una fuente (stdout, sandbox FS, comando) |
| **Enrichment** | Objeto JSON con los resultados del pipeline, incluido en la respuesta al agente |
| **Catalog** | Conjunto de configuraciones declarativas (assertions, doctors, advice, enrichers) en `.bastion/catalog/` |
| **PostRunContext** | Contexto pasado al enricher tras la ejecución: comando, salida, sandbox ID, acceso a FS |
| **ExtractorBlock** | Trait Rust que implementa un tipo de extractor |
| **Pre-check** | Doctor ejecutado antes del comando para validar pre-condiciones |
| **Advice scope** | Categoría usada para filtrar qué advice son relevantes para un enricher |
| **Command pattern** | Regex que determina qué comandos activan un enricher |
| **Structured Natural Language Harness** | Bloque declarativo en lenguaje natural estructurado que describe propósito, intención, contexto útil y semántica de éxito/fallo |
| **Fact** | Observación tipada, trazable y con confianza emitida por extractores o rules |
| **Fact Pipeline** | Flujo que normaliza datos extraídos en facts, evalúa rules y compone contexto para agente |
| **Agent Context** | Respuesta compacta y accionable para el agente: resumen, verdict, artefactos, tests y próximos pasos |
| **CatalogRepository** | Repositorio SQLite usado por runtime para consultar descriptors normalizados |
| **Catalog Source** | Fichero YAML/TOML versionable que se importa a SQLite mediante hash y validación |
| **Harness Run** | Registro persistido de una ejecución de arnés: intent, facts, rules, agent_context, timings y errores |
| **Utility Event** | Evento que mide si el contexto devuelto fue útil: uso de artifact path, llamada evitada, consejo seguido, retry |
| **Meta-Harness Optimizer** | Capa offline que usa trazas y métricas para proponer mejoras de activation, extractors, rules o summaries |
| **Host application** | Aplicación que usa el crate `enrichment-engine`; Bastion es un host, pero no el único |
| **Host adapter** | Código que traduce tipos específicos del host hacia `OperationInvocation`, `OperationResult`, `FactStore`, etc. |
| **OperationInvocation** | Representación genérica de la acción previa que se quiere enriquecer |
| **OperationResult** | Resultado genérico de la acción previa: estado, stdout/stderr, duración y metadata |
| **Feature flag** | Opción de compilación del crate para activar YAML, TOML, SQLite, watcher, CEL o templates |

---

> *Este documento es la fuente única de verdad para el diseño del Proactive Enrichment Engine. Las decisiones aquí documentadas guían la implementación fase a fase.*
