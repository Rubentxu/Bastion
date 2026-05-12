# Bastion × Pipeliner — Estrategia de Integración y Modelo de Abstracciones

> **Fecha**: 2026-05-12
> **Estado**: Propuesta estratégica — Pendiente de revisión
> **Tipo**: Documento de arquitectura y estrategia de integración
> **Autor**: Investigación de referencia + diseño de abstracciones
> **Relacionado**: `006-bastion-dashboard-architecture.md`

---

## Resumen Ejecutivo

**Tesis central**: Bastion es un orquestador de sandboxes agnóstico al uso. Los pipelines son **un caso de uso** de Bastion, no su propósito principal. Pipeliner (Rustline) se integra como **ciudadano de primer nivel** — una implementación concreta de las abstracciones de orquestación que Bastion define, al mismo nivel que la prueba de concepto ad-hoc, el job batch, o la prueba E2E.

**Principio rector**: _Bastion no sabe que estás ejecutando un pipeline. Solo sabe que pediste N sandboxes con propósito X, plantilla Y, y límites Z._

---

## 1. Modelo Mental: Bastion como Orquestador Agnóstico

### 1.1 El Anti-Pattern a Evitar

```
❌ Bastion → Pipeline Engine → Sandboxes
   (Bastion acoplado a pipelines)
```

Si Bastion se diseña pensando en pipelines, cada sandbox será un "stage de pipeline" por defecto. Esto contamina el modelo de dominio:

- `SandboxPurpose::PipelineStage` implica que el propósito principal son pipelines
- `PipelineDef` en `bastion-domain` sugiere que los pipelines son parte del core
- El dashboard muestra "Pipelines" como tab principal

### 1.2 El Modelo Correcto

```
✅ Bastion (agnóstico) ← SandboxUseCase (trait)
   ├── PipelineUseCase      (implementado por Pipeliner)
   ├── AdHocTestUseCase     (el usuario prueba algo manualmente)
   ├── PoCUseCase           (experimentación libre)
   ├── E2eTestUseCase       (pruebas end-to-end)
   ├── JobUseCase           (jobs batch / cron)
   └── CustomUseCase        (extensible por el usuario)
```

Bastion expone un **trait `SandboxUseCase`** que define cómo un caso de uso consume sandboxes. Pipeliner implementa ese trait. Pero Bastion no sabe ni le importa qué implementación está corriendo.

### 1.3 Implicaciones Arquitectónicas

| Aspecto | Bastion (core) | Pipeliner (plugin) |
|---------|----------------|-------------------|
| Conoce pipelines? | No | Sí |
| Define `SandboxUseCase` trait | Sí | No |
| Implementa `SandboxUseCase` | No | Sí |
| Gestiona lifecycle de sandboxes | Sí | No (delega a Bastion) |
| Ejecuta comandos en sandboxes | Sí | Pide a Bastion que ejecute |
| Almacena estado | `.bastion/db/` | `.bastion/pipelines/` |
| Define triggers | No | Sí |
| Sabe de DAGs/paralelismo | No | Sí |
| Expone al dashboard | UseCase API | Pipeline API (a través de UseCase) |

---

## 2. Abstracciones Propuestas en Bastion

### 2.1 Trait `SandboxUseCase` — El Contract

```rust
/// Un caso de uso que consume sandboxes de Bastion.
///
/// Este es el punto de extensión de Bastion: cualquier workflow
/// que necesite sandboxes implementa este trait.
///
/// Implementaciones: PipelineUseCase, AdHocTestUseCase, PoCUseCase, etc.
#[async_trait]
pub trait SandboxUseCase: Send + Sync + std::fmt::Debug {
    /// Identificador único del tipo de caso de uso.
    fn kind(&self) -> &UseCaseKind;

    /// Nombre legible.
    fn name(&self) -> &str;

    /// Prepara el caso de uso: valida configuración, reserva recursos.
    ///
    /// Retorna un `UseCasePlan` que describe qué sandboxes se necesitan,
    /// en qué orden, con qué dependencias.
    async fn plan(&self, context: &UseCaseContext) -> Result<UseCasePlan, UseCaseError>;

    /// Ejecuta un paso del plan en un sandbox dado.
    ///
    /// Bastion crea el sandbox, llama a `execute_step` para que el
    /// caso de uso diga qué hacer dentro del sandbox.
    async fn execute_step(
        &self,
        sandbox: &Sandbox,
        step: &UseCaseStep,
        executor: &dyn TaskExecutor,
    ) -> Result<StepOutcome, UseCaseError>;

    /// Maneja el resultado de un paso completado.
    ///
    /// Permite al caso de uso decidir: continuar, abortar, reintentar, etc.
    async fn on_step_completed(
        &self,
        result: &StepResult,
        plan: &mut UseCasePlan,
    ) -> StepDecision;

    /// Limpieza al finalizar el caso de uso (éxito o fracaso).
    async fn cleanup(&self, context: &UseCaseContext) -> Result<(), UseCaseError>;
}
```

### 2.2 Tipos de Soporte

```rust
/// Tipos de casos de uso que Bastion reconoce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UseCaseKind {
    /// Pipeline CI/CD — implementado por Pipeliner
    Pipeline,
    /// Prueba ad-hoc interactiva
    AdHocTest,
    /// Prueba de concepto
    ProofOfConcept,
    /// Pruebas end-to-end
    E2eTest,
    /// Job batch o cron
    BatchJob,
    /// Personalizado por el usuario
    Custom(String),
}

/// Contexto de ejecución de un caso de uso.
pub struct UseCaseContext {
    /// Proyecto al que pertenece este caso de uso.
    pub project_id: ProjectId,
    /// Directorio .bastion/ del proyecto.
    pub bastion_dir: PathBuf,
    /// Variables de entorno del proyecto.
    pub environment: HashMap<String, String>,
    /// Configuración del caso de uso (TOML/YAML cargado).
    pub config: UseCaseConfig,
    /// Git info del proyecto.
    pub git_info: GitInfo,
}

/// Plan de ejecución — qué sandboxes se necesitan y en qué orden.
pub struct UseCasePlan {
    /// Steps a ejecutar (pueden ser secuenciales o paralelos).
    pub steps: Vec<UseCaseStep>,
    /// Dependencias entre steps (DAG).
    pub dependencies: HashMap<String, Vec<String>>,
    /// Steps que pueden correr en paralelo.
    pub parallel_groups: Vec<Vec<String>>,
    /// Políticas de reintentos, timeouts, cleanup.
    pub policy: UseCasePolicy,
}

/// Un paso individual en un caso de uso.
pub struct UseCaseStep {
    /// ID único del paso.
    pub id: String,
    /// Nombre legible.
    pub name: String,
    /// Template de sandbox a usar.
    pub template: String,
    /// Recursos necesarios.
    pub resources: ResourcesSpec,
    /// Variables de entorno para este paso.
    pub environment: HashMap<String, String>,
    /// Timeout en ms.
    pub timeout_ms: u64,
    /// Comando a ejecutar (opcional — el caso de uso puede ejecutar múltiples).
    pub command: Option<CommandSpec>,
}

/// Resultado de un paso.
pub enum StepOutcome {
    /// Paso completado exitosamente.
    Success { output: String, artifacts: Vec<ArtifactRef> },
    /// Paso falló.
    Failure { error: String, exit_code: i32 },
    /// Paso saltado (condición no cumplida).
    Skipped { reason: String },
}

/// Decisión del caso de uso tras completar un paso.
pub enum StepDecision {
    /// Continuar con el siguiente paso.
    Continue,
    /// Abortar todo el caso de uso.
    Abort { reason: String },
    /// Reintentar este paso.
    Retry { max_attempts: usize, delay_ms: u64 },
    /// Saltar al paso indicado.
    JumpTo { step_id: String },
}

/// Políticas del caso de uso.
pub struct UseCasePolicy {
    pub max_lifetime: Duration,
    pub retry_count: usize,
    pub cleanup_on_success: bool,
    pub cleanup_on_failure: bool,
    pub on_failure: FailurePolicy,
}

pub enum FailurePolicy {
    Stop,
    Continue,
    Report,
}
```

### 2.3 Trait `UseCaseExecutor` — El Motor de Ejecución

```rust
/// Motor de ejecución de casos de uso.
///
/// Bastion proporciona una implementación por defecto que:
/// 1. Lee el plan del caso de uso
/// 2. Crea sandboxes según el plan
/// 3. Ejecuta steps secuencialmente o en paralelo según el DAG
/// 4. Notifica al caso de uso de cada resultado
/// 5. Limpia al finalizar
#[async_trait]
pub trait UseCaseExecutor: Send + Sync {
    /// Ejecuta un caso de uso completo.
    async fn execute(
        &self,
        use_case: &dyn SandboxUseCase,
        context: UseCaseContext,
        provider: &dyn SandboxProvider,
    ) -> Result<UseCaseResult, UseCaseError>;
}
```

### 2.4 Registro de Casos de Uso

```rust
/// Registro de casos de uso disponibles.
pub struct UseCaseRegistry {
    factories: HashMap<UseCaseKind, Box<dyn UseCaseFactory>>,
}

impl UseCaseRegistry {
    /// Registra un caso de uso.
    pub fn register<F: UseCaseFactory + 'static>(&mut self, factory: F) {
        self.factories.insert(factory.kind(), Box::new(factory));
    }

    /// Crea un caso de uso a partir de configuración.
    pub fn create(
        &self,
        kind: &UseCaseKind,
        config: &UseCaseConfig,
    ) -> Result<Box<dyn SandboxUseCase>, UseCaseError> {
        self.factories
            .get(kind)
            .ok_or(UseCaseError::UnknownUseCase(kind.clone()))?
            .create(config)
    }
}

/// Factory para crear casos de uso.
pub trait UseCaseFactory: Send + Sync {
    fn kind(&self) -> UseCaseKind;
    fn create(&self, config: &UseCaseConfig) -> Result<Box<dyn SandboxUseCase>, UseCaseError>;
}
```

---

## 3. Pipeliner como Implementación Concreta

### 3.1 Arquitectura de Integración

```
┌─────────────────────────────────────────────────────────────────┐
│                        BASTION CORE                            │
│                                                                 │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ SandboxProvider │ │ UseCaseRegistry │ │ UseCaseExecutor   │  │
│  │ (Podman, etc)  │ │                │ │ (default impl)      │  │
│  └──────┬──────┘  └──────┬───────┘  └──────┬─────────────┘  │
│         │                │                   │                 │
│         │    ┌───────────┼───────────────────┘                 │
│         │    │           │                                     │
└─────────┼────┼───────────┼─────────────────────────────────────┘
          │    │           │
          │    ▼           ▼
          │  ┌─────────────────────────────────────────────────┐
          │  │              BASTION-PIPELINE (crate)            │
          │  │                                                  │
          │  │  ┌──────────────────────────────────────────┐  │
          │  │  │ PipelineUseCaseFactory                    │  │
          │  │  │   kind: Pipeline                          │  │
          │  │  │   create(config) → PipelineUseCase        │  │
          │  │  └──────────────────────────────────────────┘  │
          │  │                                                  │
          │  │  ┌──────────────────────────────────────────┐  │
          │  │  │ PipelineUseCase                            │  │
          │  │  │   impl SandboxUseCase for PipelineUseCase │  │
          │  │  │   - plan() → DAG de steps                 │  │
          │  │  │   - execute_step() → delega a Pipeliner   │  │
          │  │  │   - on_step_completed() → flujo del DAG   │  │
          │  │  └──────────────────────────────────────────┘  │
          │  │                                                  │
          │  │  ┌──────────────────────────────────────────┐  │
          │  │  │ Dependencia: pipeliner (crate externo)    │  │
          │  │  │   Pipeline, Stage, Step, AgentType       │  │
          │  │  │   PipelineBuilder, StageBuilder          │  │
          │  │  │   WhenCondition, PostCondition            │  │
          │  │  │   Environment, Parameters, Matrix        │  │
          │  │  └──────────────────────────────────────────┘  │
          │  └─────────────────────────────────────────────────┘
          │
          ▼
   ┌──────────────┐
   │ Podman       │
   │ (sandbox)    │
   └──────────────┘
```

### 3.2 Flujo de Ejecución de Pipeline

```
1. Usuario: `bastion pipeline run ci`
   ↓
2. Bastion: lee `.bastion/pipelines/ci.toml`
   ↓
3. Bastion: UseCaseRegistry.create(Pipeline, config)
   → PipelineUseCaseFactory.create(config)
   → PipelineUseCase { pipeline_def, pipeliner_pipeline }
   ↓
4. PipelineUseCase.plan(context)
   → Convierte TOML → Pipeline (Pipeliner type)
   → Valida con pipelinerPipeline.validate()`
   → Genera UseCasePlan con DAG de steps
   ↓
5. UseCaseExecutor.execute(pipeline_use_case, context, provider)
   → Para cada step en el plan (respetando DAG y paralelismo):
     a. SandboxProvider.create(step.template, step.resources)
     b. PipelineUseCase.execute_step(sandbox, step, executor)
        → Ejecuta Step::shell(command) dentro del sandbox
     c. PipelineUseCase.on_step_completed(result, plan)
        → Decide: Continue / Abort / Retry
     d. Evalúa WhenCondition para el siguiente step
     e. Ejecuta PostCondition si aplica
   ↓
6. PipelineUseCase.cleanup(context)
   → Limpia sandboxes, reporta resultados
```

### 3.3 Adaptador Pipeliner → Bastion

```rust
/// Adaptador que convierte tipos de Pipeliner a tipos de Bastion.
///
/// Pipeliner define su propio `Pipeline`, `Stage`, `Step` —
/// Bastion los consume a través de `UseCasePlan` / `UseCaseStep`.
pub struct PipelineAdapter;

impl PipelineAdapter {
    /// Convierte un Pipeline de Pipeliner en un UseCasePlan de Bastion.
    pub fn to_plan(pipeline: &Pipeline) -> Result<UseCasePlan, UseCaseError> {
        let mut steps = Vec::new();
        let mut dependencies = HashMap::new();
        let mut parallel_groups = Vec::new();

        for (i, stage) in pipeline.stages.iter().enumerate() {
            // Cada Stage de Pipeliner → UseCaseStep de Bastion
            let step = UseCaseStep {
                id: stage.name.clone(),
                name: stage.name.clone(),
                template: Self::extract_template(&stage.agent),
                resources: Self::extract_resources(&stage.agent),
                environment: pipeline.environment.vars.clone(),
                timeout_ms: pipeline.options.timeout
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(300_000),
                command: Self::extract_command(stage),
            };
            steps.push(step);

            // WhenCondition → dependencias condicionales
            if let Some(ref when) = stage.when {
                // Las condiciones se evalúan en runtime,
                // pero se registran en el plan como metadatos
            }

            // Parallel branches → parallel_groups
            if !stage.parallel.is_empty() {
                let group: Vec<String> = stage.parallel.iter()
                    .map(|b| b.name.clone())
                    .collect();
                parallel_groups.push(group);
            }
        }

        Ok(UseCasePlan {
            steps,
            dependencies,
            parallel_groups,
            policy: UseCasePolicy {
                max_lifetime: pipeline.options.timeout
                    .unwrap_or(Duration::from_secs(3600)),
                retry_count: pipeline.options.retry.unwrap_or(0),
                cleanup_on_success: true,
                cleanup_on_failure: true,
                on_failure: FailurePolicy::Stop,
            },
        })
    }

    fn extract_template(agent: &AgentType) -> String {
        match agent {
            AgentType::Docker(c) => c.image.clone(),
            AgentType::Podman(c) => c.image.clone(),
            AgentType::Kubernetes(c) => c.image.clone(),
            AgentType::Any => "default".to_string(),
            AgentType::Label(l) => l.clone(),
        }
    }

    fn extract_command(stage: &Stage) -> Option<CommandSpec> {
        stage.steps.first().and_then(|step| {
            match &step.step_type {
                StepType::Shell { command } => Some(CommandSpec::new(command)),
                _ => None,
            }
        })
    }
}
```

---

## 4. Modelos de Datos de GISS — Lecciones para Bastion

### 4.1 ProjectDescriptor (GISS)

**Fuente**: `modules/core/testFixtures/resources/naua/pipeline.yaml`

```yaml
kind: ProjectDescriptor
apiVersion: project.giss.es/v1
spec:
  projectType: Maven

  settings:
    name: naua
    description: Proyecto backend para el servicio
    currentVersion: 1.0.0-140-SNAPSHOT
    developCenter: ot
    codeCapp: fdar
    promotableItem: back

  sourceRepositories:
    - id: defaultSourceRepository
      url: https://gitlab.pro.portal.ss/...
      branch: master
      credentialsId: oc-gs-jenkins-gitlab-auth

  tools:
    - id: defaultMavenTool
      name: maven
      version: 3.6.3
      cache: true
      buildStrategies:
        - buildCommand: -B clean verify -DskipTests
      testStrategies:
        - testCommand: clean test -DargLine="-Xmx1g"
      publishStrategies:
        - publishCommand: deploy -DskipTests -DaltDeploymentRepository=...

  artifactsRepositories:
    - id: defaultNexusRepository
      url: https://nexus-...
      credentialsId: nexus-credentials

  notifications:
    emails: [...]

  scannerTools:
    - id: defaultUtf8Tool
      command: ""
      metadata: { validExtensions: { java: [".java"] } }
```

**Lecciones para Bastion**:

| Concepto GISS | Aplicación en Bastion |
|---------------|----------------------|
| `projectType` → Enum (Maven, Gradle, Naua) | Ya tenemos `ProjectKind` (Rust, NodeJs, Python, Go, Generic) |
| `settings` → Metadatos del proyecto | Enriquecer `project.toml` con `description`, `version`, `developCenter` |
| `sourceRepositories` → Git sources | Ya tenemos integración Git — ampliar con `credentialsId` |
| `tools[]` → Herramientas con strategies | **NUEVO**: `.bastion/tools/` — herramientas disponibles para pipelines |
| `buildStrategies/testStrategies/publishStrategies` | Mapean directamente a Pipeline Stages con propósitos diferentes |
| `artifactsRepositories` → Nexus/registry | **NUEVO**: `.bastion/artifacts.toml` — repositorios de artefactos |
| `notifications` → Alertas | Integrar con sistema de eventos SSE existente |
| `scannerTools` → Análisis | **NUEVO**: scanners como capabilities del proyecto |

### 4.2 PipelineDefinition (GISS)

```yaml
kind: PipelineDefinition
apiVersion: pipeline.giss.es/v1
spec:
  settings:
    debugMode: false
    devopsEnvironment: dev
    skipStages: []
    parameters:
      BRANCH: main

  cache:
    baseDir: /opt/cache
    cacheFolders: [maven, helm/v3/cache]
    exportEnvVar: CACHE_BASE_DIR
    forceClearCache: false

  environmentVars:
    M2_HOME: ${CACHE_BASE_DIR}/maven
    MAVEN_OPTS: >-
      -Xmx512m -Xms256m -Dmaven.repo.local=${CACHE_BASE_DIR}/maven

  toolsManager:
    alternatives:
      javaEngine: java_sdk_11_openjdk
    asdf:
      plugins:
        maven: https://github.com/halcyon/asdf-maven.git
      tools:
        maven: 3.6.3
        helm: 3.12.1
```

**Lecciones para Bastion**:

| Concepto GISS | Aplicación en Bastion |
|---------------|----------------------|
| `settings.parameters` | Mapea a `Parameters` de Pipeliner — ya soportado |
| `cache` → Cache de dependencias entre builds | **NUEVO**: `.bastion/cache.toml` — cache persistente entre runs |
| `environmentVars` con `${VAR}` expansion | Mapea a `Environment` de Pipeliner — ya soportado |
| `toolsManager` → Instalar herramientas antes de stages | **NUEVO**: `ToolManagerAdapter` en Bastion — instalar tools en sandbox |
| `asdf` → Version-managed tools | **NUEVO**: Integrar asdf/vfox como capability |

### 4.3 ReleaseDescriptor (GISS)

```yaml
kind: ReleaseDescriptor
apiVersion: release.giss.es/v1
spec:
  releaseConfigs:
    - id: defaultReleaseConfig
      runtimeTech: java
      runtimeTechVersion: '21'
      configPath: "src/main"
      ocNamespaceSuffix: ot-fdar
      chartVersion: "1.19.0"
      chartName: ot-fdarback_main
      baseImageName: openjdk
```

**Lecciones para Bastion**:

| Concepto GISS | Aplicación en Bastion |
|---------------|----------------------|
| `releaseConfigs` → Configuración de release | **NUEVO**: `.bastion/releases/` — descriptores de release |
| `runtimeTech + runtimeTechVersion` | Mapea a `ProjectKind` + versión específica |
| `chartVersion/chartName` → Helm charts | **NUEVO**: Integración con deployment tools |
| `baseImageName` → Imagen base | Ya tenemos templates — ampliar con imagen base configurable |

### 4.4 Modelo de Datos Propuesto para `.bastion/`

Basado en GISS, pero adaptado al modelo agnóstico de Bastion:

```toml
# .bastion/project.toml — ProjectDescriptor (Bastion flavor)

[project]
name = "bastion"
kind = "rust"
description = "Sandbox orchestration engine"
version = "0.1.0"

[project.git]
remote = "https://github.com/user/bastion"
branch = "main"

# Herramientas disponibles (inspirado en GISS tools[])
[[tools]]
id = "rust-toolchain"
name = "rust"
version = "1.78.0"
cache = true

[[tools.build-strategies]]
id = "cargo-build"
command = "cargo build --workspace"

[[tools.test-strategies]]
id = "cargo-test"
command = "cargo test --workspace"
metadata = { test-type = "unit", framework = "rusttest" }

[[tools.lint-strategies]]
id = "cargo-clippy"
command = "cargo clippy --workspace -- -D warnings"

# Repositorios de artefactos (inspirado en GISS artifactsRepositories)
[[artifacts-repositories]]
id = "crates-io"
url = "https://crates.io"
kind = "cargo"
```

```toml
# .bastion/pipelines/ci.toml — PipelineDefinition (Bastion flavor)
# Usa tipos de Pipeliner directamente

[pipeline]
name = "ci"
description = "CI pipeline"

[pipeline.environment]
RUST_BACKTRACE = "1"
CARGO_TERM_PROGRESS_WIDTH = "80"

[pipeline.parameters.boolean]
skip_tests = false
[pipeline.parameters.string]
target_branch = ""

[pipeline.options]
timeout = "30m"
retry = 1
skip_default_checkout = false

[[pipeline.triggers]]
type = "cron"
expression = "H/15 * * * *"

[[stages]]
name = "check"
agent = { type = "podman", image = "rust:1.78" }
steps = [{ type = "shell", command = "cargo check --workspace" }]
post = [{ type = "always", steps = [{ type = "echo", message = "check done" }] }]

[[stages]]
name = "test"
agent = { type = "podman", image = "rust:1.78" }
steps = [{ type = "shell", command = "cargo test --workspace" }]
when = { type = "expression", expression = "!skip_tests" }

[[stages]]
name = "lint"
agent = { type = "podman", image = "rust:1.78" }
steps = [{ type = "shell", command = "cargo clippy --workspace -- -D warnings" }]
parallel = [
  { name = "lint-clippy", stage = { name = "clippy", steps = [...] } },
  { name = "lint-fmt", stage = { name = "fmt", steps = [...] } },
]
post = [{ type = "failure", steps = [{ type = "echo", message = "lint failed" }] }]
```

---

## 5. Pensamiento Lateral — Innovaciones con el Modelo Agnóstico

### 5.1 Sandbox como Recurso Compartible entre Casos de Uso

El modelo agnóstico permite que **un sandbox sirva a múltiples casos de uso** simultáneamente:

```rust
// Un sandbox de pool puede ser reclamado por un pipeline stage
// y luego por un ad-hoc test, sin crear uno nuevo
let sandbox = provider.create("rust-ci", &resources, ...).await?;

// Pipeline stage lo usa
pipeline_use_case.execute_step(&sandbox, &step, &executor).await?;

// Luego un ad-hoc test lo reutiliza (si sigue vivo)
adhoc_use_case.execute_step(&sandbox, &adhoc_step, &executor).await?;
```

### 5.2 Sandbox como "Function-as-a-Service"

Con el modelo de `SandboxUseCase`, los sandboxes se vuelven invocables como funciones:

```bash
# Invocar un caso de uso desde CLI
bastion run --use-case pipeline --config ci.toml
bastion run --use-case e2e-test --suite smoke
bastion run --use-case batch-job --config deploy-job.toml

# O directamente con el caso de uso por defecto (adhoc)
bastion sandbox create --template rust-ci
bastion sandbox exec <id> -- cargo test
```

### 5.3 Casos de Uso Componibles

Los casos de uso pueden componerse — un pipeline puede invocar a otros casos de uso:

```toml
[[stages]]
name = "e2e"
use_case = { kind = "e2e-test", suite = "smoke" }
# En lugar de definir steps inline, delega a otro UseCase
```

### 5.4 Marketplace de Casos de Uso

Con el `UseCaseRegistry`, los casos de uso son registrables dinámicamente:

```bash
# Instalar un nuevo caso de uso (como plugin)
bastion use-case install bastion-ansible-plugin
bastion use-case install bastion-k6-load-test
bastion use-case list
# pipeline, adhoc-test, poc, e2e-test, batch-job, ansible, k6-load-test
```

### 5.5 Time-Travel Debugging con Sandboxes

Los snapshots de sandbox permiten "volver atrás en el tiempo":

```bash
# Snapshot automático antes de cada step de pipeline
bastion pipeline run ci --snapshot-each-step

# Al fallar, inspeccionar el sandbox en el punto exacto del fallo
bastion sandbox inspect <id> --at-step test
```

### 5.6 Sandbox Templates como "Capabilidades" Componibles

Inspirado en las `capabilities` de Bastion y las `strategies` de GISS:

```toml
# .bastion/capabilities/rust-build.toml
[capability]
name = "rust-build"
description = "Rust build environment"

[capability.tools]
rust = "1.78.0"
cargo-nextest = "latest"

[capability.env]
RUST_BACKTRACE = "1"
CARGO_INCREMENTAL = "1"

[capability.strategies.build]
command = "cargo build --workspace"
timeout = "10m"

[capability.strategies.test]
command = "cargo nextest run --workspace"
timeout = "20m"

[capability.strategies.lint]
command = "cargo clippy --workspace -- -D warnings"
timeout = "5m"
```

Un pipeline referencia capabilities en lugar de definir steps manualmente:

```toml
[[stages]]
name = "build"
capability = "rust-build"    # ← usa la strategy.build de la capability
strategy = "build"           # ← qué strategy usar de la capability
```

### 5.7 Cost Attribution por Caso de Uso

Cada caso de uso puede tener su propio modelo de costos:

```rust
pub trait SandboxUseCase {
    // ... existing methods ...

    /// Calcula el costo de este caso de uso.
    fn cost_model(&self) -> CostModel {
        CostModel::default() // Sobreescribir por caso de uso
    }
}

pub struct CostModel {
    pub cpu_hour_rate: f32,
    pub memory_gb_hour_rate: f32,
    pub sandbox_hour_rate: f32,
    pub minimum_charge: f32,
}
```

El dashboard muestra costos agregados por caso de uso:

```
Proyecto: Bastion
├── Pipeline CI:    $0.45 (312 runs, avg 1.2s)
├── E2E Tests:      $0.12 (45 runs, avg 5.3s)
├── AdHoc Testing:  $0.03 (7 sessions, avg 12m)
└── Batch Jobs:     $0.08 (2 runs, avg 45m)
```

---

## 6. Plan de Implementación

### Fase 0: Preparación (1 semana)

- [ ] Crear `bastion-domain/src/usecase/` module con traits `SandboxUseCase`, `UseCaseExecutor`, `UseCaseRegistry`
- [ ] Crear tipos de soporte: `UseCasePlan`, `UseCaseStep`, `StepOutcome`, `StepDecision`, `UseCasePolicy`
- [ ] Mover `PipelineDef` y `PipelineStage` fuera de `bastion-domain/src/project/types.rs` → eliminar de domain core
- [ ] Actualizar `Project` aggregate: `pipelines: Vec<PipelineDef>` → `use_cases: Vec<UseCaseConfig>`

### Fase 1: Casos de Uso Core (2 semanas)

- [ ] Implementar `AdHocTestUseCase` — el caso más simple (un sandbox, un comando)
- [ ] Implementar `PoCUseCase` — sandbox con lifetime extendido, sin comando fijo
- [ ] Implementar `BatchJobUseCase` — sandbox con comando batch, cleanup automático
- [ ] Implementar `DefaultUseCaseExecutor` — ejecutor genérico que respeta DAG
- [ ] Tests: cada caso de uso con mock provider

### Fase 2: Integración Pipeliner (3 semanas)

- [ ] Crear crate `bastion-pipeline` (depende de `bastion-domain` + `pipeliner`)
- [ ] Implementar `PipelineUseCaseFactory`
- [ ] Implementar `PipelineUseCase` — adaptador entre Pipeliner types y Bastion traits
- [ ] Implementar `PipelineAdapter::to_plan()` — convertir Pipeline → UseCasePlan
- [ ] Implementar `PipelineAdapter::from_toml()` — leer `.bastion/pipelines/*.toml`
- [ ] Soporte para: environment, parameters, when conditions, post conditions
- [ ] Soporte para: parallel branches, matrix execution (via DAG en UseCasePlan)

### Fase 3: GISS-Inspired Data Models (1 semana)

- [ ] Diseñar `project.toml` schema con GISS-inspired fields (tools, strategies, artifacts-repositories)
- [ ] Diseñar `cache.toml` schema (cache persistente entre runs)
- [ ] Diseñar `releases/` schema (ReleaseDescriptor adaptado)
- [ ] Parser TOML → tipos Rust en `bastion-domain`

### Fase 4: CLI + Dashboard (2 semanas)

- [ ] CLI: `bastion run --use-case <kind> --config <path>`
- [ ] CLI: `bastion use-case list` — mostrar casos de uso registrados
- [ ] Dashboard: UseCase API endpoints (`/api/v1/projects/:id/use-cases/`)
- [ ] Dashboard: Pipeline visualization (via UseCase events)
- [ ] Dashboard: UseCase-agnostic metrics (costos por tipo de uso)

### Fase 5: Extensibilidad (ongoing)

- [ ] Documentar cómo crear un `SandboxUseCase` custom
- [ ] Plugin system: cargar casos de uso desde crates externos
- [ ] Marketplace conceptual: registry de use-cases compartibles

---

## 7. Cambios en el Modelo de Dominio Existente

### 7.1 Eliminar de `bastion-domain` (core)

```rust
// ELIMINAR de project/types.rs:
pub struct PipelineDef { ... }
pub struct PipelineStage { ... }

// MOVER a bastion-pipeline crate
```

### 7.2 Añadir a `bastion-domain` (core)

```rust
// NUEVO module: usecase/
pub trait SandboxUseCase { ... }
pub trait UseCaseExecutor { ... }
pub trait UseCaseFactory { ... }
pub struct UseCaseRegistry { ... }
pub struct UseCasePlan { ... }
pub struct UseCaseStep { ... }
pub enum UseCaseKind { ... }
pub enum StepOutcome { ... }
pub enum StepDecision { ... }
pub struct UseCasePolicy { ... }
```

### 7.3 Modificar en `bastion-domain`

```rust
// project/aggregate.rs - Project struct
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectKind,
    pub git: GitInfo,
    pub sandboxes: Vec<SandboxSummary>,
-   pub pipelines: Vec<PipelineDef>,           // ELIMINAR
+   pub use_cases: Vec<UseCaseConfig>,          // NUEVO — configs de casos de uso
    pub pool_config: PoolConfig,
    pub resource_limits: ResourceLimits,
    pub last_active: DateTime<Utc>,
    pub cost: CostSummary,
}

// SandboxPurpose — simplificar
pub enum SandboxPurpose {
    AdHocTest,
    ProofOfConcept,
    E2eTest,
    RealTest,
-   PipelineStage,           // Ya no es un propósito — es un caso de uso
    Job,
+   UseCase(String),         // Caso de uso genérico (pipeline, ansible, k6, etc.)
}
```

---

## 8. Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|-------------|---------|------------|
| Over-engineering: demasiadas abstracciones para 2 casos de uso | Media | Alto | Empezar con solo 2 casos de uso (adhoc + pipeline), añadir más según necesidad |
| Pipeliner API cambia frecuentemente | Media | Medio | El adaptador aísla cambios; Pipeliner es nuestro proyecto, controlamos breaking changes |
| Performance overhead del trait dispatch | Baja | Bajo | Benchmark; usar generics donde sea posible, `dyn` solo en registry |
| TOML schema demasiado complejo (como GISS) | Media | Medio | Empezar con schema mínimo, ampliar iterativamente |
| Dashboard no sabe cómo visualizar UseCases genéricos | Media | Medio | Cada UseCase expone su propio `UseCaseView` con layout específico |

---

## 9. Decisión Estratégica: Por qué Este Modelo

### 9.1 ¿Por qué no Pipeliner directamente en el core?

Porque **Bastion no es un motor de pipelines**. Bastion es un **orquestador de sandboxes**. Si acoplamos Pipeliner al core:

- Cada cambio en Pipeliner rompe Bastion
- No podemos añadir casos de uso que no sean pipelines
- El modelo mental se contamina: "sandbox = pipeline stage"
- Otros usuarios de Bastion (que no quieren pipelines) tienen overhead innecesario

### 9.2 ¿Por qué abstracciones y no implementación directa?

Porque **las abstracciones correctamente diseñadas son más baratas que el acoplamiento**:

- El trait `SandboxUseCase` tiene ~5 métodos, todos opcionales con defaults
- `UseCasePlan` es un DAG genérico — sirve para pipelines, tests, jobs
- El coste de la abstracción es ~200 LOC de traits
- El beneficio es: Pipeliner es intercambiable, añadir casos de uso es trivial

### 9.3 ¿Por qué GISS models como inspiración y no como estándar?

Porque GISS está diseñado para **Jenkins + Groovy + OpenShift** — un contexto muy específico. Pero los **patrones de modelado** son universales:

- `ProjectDescriptor` → cómo describir un proyecto de software
- `PipelineDefinition` → cómo configurar un pipeline declarativo
- `ReleaseDescriptor` → cómo gestionar releases

Tomamos los patrones, no la implementación.

---

## 10. Conclusión

**Bastion orquesta sandboxes. Los pipelines usan sandboxes. Son capas diferentes.**

La integración de Pipeliner como ciudadano de primer nivel se logra mediante:

1. **Abstracción `SandboxUseCase`** — Bastion define QUÉ necesita de un caso de uso
2. **`PipelineUseCase`** — Pipeliner implementa CÓMO un pipeline usa sandboxes
3. **`UseCaseRegistry`** — Bastion descubre casos de uso dinámicamente
4. **Modelo de datos GISS-inspired** — Configuración declarativa en `.bastion/`

Este modelo mantiene a Bastion agnóstico, permite que Pipeliner evolucione independientemente, y abre la puerta a una ecología de casos de uso que van más allá de los pipelines.

---

*Documento de estrategia — 2026-05-12*
*Proyectos: Bastion + Pipeliner + GISS Framework*
