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

### 2.1 Insight Clave: Pipeliner ya tiene DSL — no lo dupliquemos

Pipeliner ya define:

| Pipeliner | Función |
|-----------|---------|
| `Pipeline` (name, agent, stages[], environment, parameters, triggers, options, post) | Pipeline completo |
| `Stage` (name, agent, steps[], parallel[], matrix, when, post) | Un stage con su lógica |
| `Step` / `StepType` (Shell, Echo, Retry, Timeout, Stash, Unstash, Input, Dir, Wait) | Pasos atómicos |
| `PipelineBuilder` | Builder pattern para construir pipelines |
| `PipelineExecutor` trait | `execute()`, `validate()`, `dry_run()`, `capabilities()`, `health_check()` |
| `PipelineContext` (env, cwd, pipeline_id, stage_results) | Contexto de ejecución |
| `AgentType` (Any, Label, Docker, Kubernetes, Podman) | Dónde ejecutar |
| `WhenCondition` (Branch, Tag, Environment, Expression, AllOf, AnyOf) | Condiciones |
| `PostCondition` (Always, Success, Failure, Unstable, Changed) | Post-build |
| `Environment` con `${VAR}` expansion | Variables de entorno |
| `Parameters` (boolean, string, choice) | Parámetros |
| `MatrixConfig` (axes, excludes) | Ejecución matrix |

**Pipeliner tiene su propio motor de ejecución.** El `PipelineExecutor` ejecuta `Stage`s con `Step`s en `AgentType`s. Pero Pipeliner ejecuta directamente en Docker/K8s/Podman/local — **no conoce los sandboxes de Bastion**.

**La integración no es un adaptador que traduce tipos. Es más simple:**

> Hacer que Pipeliner delegue la ejecución de Steps a los sandboxes de Bastion, en vez de ejecutar directamente en Docker/K8s.

### 2.2 Dos Capas, No Tres

```
CAPA 1: BASTION (sandbox infrastructure)
  - SandboxLifecycle: create, terminate, is_alive, snapshots
  - TaskExecutor: run_command, run_command_stream, write_file, read_file
  - SandboxProvider: composición de ambos
  - SandboxRegistry: registro de sandboxes activos

CAPA 2: PIPELINER (pipeline orchestration)
  - Pipeline: definición declarativa con stages, environment, parameters
  - PipelineExecutor: motor que decide QUÉ ejecutar y CUÁNDO (DAG, paralelismo, when, post)
  - Stage, Step, WhenCondition, PostCondition: lógica del pipeline
  - PipelineBuilder: construcción programática

PUENTE: BastionPipelineExecutor
  - impl PipelineExecutor for BastionPipelineExecutor
  - Traduce AgentType → Sandbox template
  - Traduce Step::Shell → TaskExecutor::run_command
  - Traduce Environment → sandbox env_vars
  - Bastion crea el sandbox, Pipeliner dice qué hacer dentro
```

**No hay `UseCasePlan`, `UseCaseStep`, ni `UseCaseExecutor`.** Esos son Pipeliner con otro nombre. Usamos Pipeliner directamente.

### 2.3 Trait `SandboxUseCase` — Mínimo, Solo Contract de Acceso

```rust
/// Un caso de uso que consume sandboxes de Bastion.
///
/// Este es el PUNTO DE EXTENSIÓN de Bastion: cualquier workflow
/// que necesite sandboxes implementa este trait.
///
/// El trait es DELIBERADAMENTE mínimo — Bastion solo necesita saber:
/// 1. Quién eres (kind + name)
/// 2. Qué sandboxes vas a necesitar (sandbox_requests)
/// 3. Que te avise cuando termines (on_complete)
///
/// Toda la lógica de planificación (DAG, paralelismo, when, post)
/// la hace el caso de uso internamente, usando su propio motor.
/// Pipeliner usa su PipelineExecutor. Un job batch usa su propio loop.
#[async_trait]
pub trait SandboxUseCase: Send + Sync + std::fmt::Debug {
    /// Identificador del tipo de caso de uso.
    fn kind(&self) -> &UseCaseKind;

    /// Nombre legible de esta instancia.
    fn name(&self) -> &str;

    /// Ejecuta el caso de uso completo.
    ///
    /// Recibe acceso al SandboxProvider de Bastion para crear/destruir
    /// sandboxes según necesite. Toda la lógica interna (DAG, paralelismo,
    /// reintentos) es responsabilidad del caso de uso.
    ///
    /// Pipeliner llama a `provider.create()` por cada Stage,
    /// luego a `executor.run_command()` por cada Step.
    async fn run(
        &self,
        provider: &dyn SandboxProvider,
        context: &UseCaseContext,
    ) -> Result<UseCaseResult, UseCaseError>;

    /// Limpieza al finalizar (éxito o fracaso).
    /// Bastion llama esto después de `run()` para asegurar limpieza.
    async fn cleanup(
        &self,
        provider: &dyn SandboxProvider,
    ) -> Result<(), UseCaseError>;
}
```

### 2.4 Tipos de Soporte — Mínimos

```rust
/// Tipos de casos de uso que Bastion reconoce.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UseCaseKind {
    Pipeline,
    AdHocTest,
    ProofOfConcept,
    E2eTest,
    BatchJob,
    Custom(String),
}

/// Contexto que Bastion proporciona al caso de uso.
pub struct UseCaseContext {
    pub project_id: ProjectId,
    pub bastion_dir: PathBuf,
    pub environment: HashMap<String, String>,
    pub git_info: GitInfo,
}

/// Resultado genérico de un caso de uso.
pub struct UseCaseResult {
    pub status: UseCaseStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub sandbox_count: usize,
    pub metadata: HashMap<String, String>,
}

pub enum UseCaseStatus {
    Success,
    Failure { reason: String },
    Partial { completed: usize, total: usize },
}
```

### 2.5 Registro de Casos de Uso

```rust
/// Registro de casos de uso disponibles.
pub struct UseCaseRegistry {
    factories: HashMap<UseCaseKind, Box<dyn UseCaseFactory>>,
}

pub trait UseCaseFactory: Send + Sync {
    fn kind(&self) -> UseCaseKind;
    fn create(&self, config: &serde_json::Value) -> Result<Box<dyn SandboxUseCase>, UseCaseError>;
}

impl UseCaseRegistry {
    pub fn register<F: UseCaseFactory + 'static>(&mut self, factory: F) {
        self.factories.insert(factory.kind(), Box::new(factory));
    }

    pub fn create(
        &self,
        kind: &UseCaseKind,
        config: &serde_json::Value,
    ) -> Result<Box<dyn SandboxUseCase>, UseCaseError> {
        self.factories
            .get(kind)
            .ok_or(UseCaseError::UnknownUseCase(kind.clone()))?
            .create(config)
    }
}
```

### 2.6 ¿Por qué no `UseCasePlan` / `UseCaseStep`?

Porque **Pipeliner ya tiene ese modelo** y es completo:

- `Pipeline` = plan (con stages, dependencies implícitas por when/parallel)
- `Stage` = step (con steps, parallel, matrix, when, post)
- `Step` = atomic unit (con Shell, Retry, Timeout, Stash, etc.)
- `PipelineExecutor` = motor que ejecuta el plan

Recrear esto como `UseCasePlan` / `UseCaseStep` es **re-inventar Pipeliner con otro nombre**. Y cuando otro caso de uso (ej: E2e test suite) necesite DAG, re-inventaría OTRA vez.

La solución: cada caso de uso trae SU propio motor de planificación. Pipeliner trae `PipelineExecutor`. Un E2e test suite trae su propio runner. Bastion solo proporciona **los sandboxes**.

---

## 3. Pipeliner como Implementación Concreta

### 3.1 El Insight Clave: `PipelineExecutor` es la Costura Natural

Después de leer el código de Pipeliner en profundidad, la integración es más simple y natural de lo que parece. El `PipelineExecutor` de Pipeliner es **el punto exacto de extensión**:

```rust
// Pipeliner define este trait:
pub trait PipelineExecutor: Send + Sync {
    fn execute(&self, pipeline: &Pipeline) -> Result<StageResult, PipelineError>;
    fn validate(&self, pipeline: &Pipeline) -> Result<(), ValidationError>;
    fn dry_run(&self, pipeline: &Pipeline) -> Result<StageResult, PipelineError>;
    fn capabilities(&self) -> ExecutorCapabilities;
    fn health_check(&self) -> HealthStatus;
}
```

`LocalExecutor` lo implementa ejecutando `sh -c <command>` directamente en el host. ¿Qué necesitamos? **Un `BastionExecutor` que implemente `PipelineExecutor` pero delegue la ejecución de comandos a los sandboxes de Bastion.**

El `LocalExecutor` hace esto internamente:

```rust
// LocalExecutor::execute_shell() — el punto donde ejecuta comandos
fn execute_shell(&self, command: &str, context: &PipelineContext) -> Result<(), PipelineError> {
    let shell_config = ShellConfig { cwd: context.cwd.clone(), env: context.env.clone(), ... };
    let result = ShellCommand::new(&shell_config).execute(command)?;
    //                         ^^^^ AQUÍ es donde ejecuta en el host
}
```

Nosotros reemplazamos ese punto con: `provider.run_command(sandbox_id, command)`.

### 3.2 Arquitectura: Tres Capas, Sin Duplicación

```
┌─────────────────────────────────────────────────────────────────┐
│                        BASTION CORE                            │
│                                                                 │
│  SandboxLifecycle ── create, terminate, is_alive, snapshots    │
│  TaskExecutor     ── run_command, run_command_stream, files     │
│  SandboxProvider  ── compone Lifecycle + Executor               │
│  SandboxUseCase   ── trait de extensión para workflows         │
│  UseCaseRegistry  ── registro de factories                      │
└──────────────────────────┬──────────────────────────────────────┘
                           │ impl PipelineExecutor
                           │ (delega a SandboxProvider)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    BASTION-PIPELINE (crate)                     │
│                                                                 │
│  BastionPipelineExecutor ── impl PipelineExecutor               │
│    ├── por cada Stage: crea sandbox via SandboxLifecycle        │
│    ├── por cada Step:   ejecuta via TaskExecutor                │
│    ├── AgentType → template mapping                             │
│    ├── Environment → sandbox env_vars                           │
│    ├── WhenCondition → evaluación pre-step                      │
│    ├── PostCondition → evaluación post-step                     │
│    └── cleanup: termina sandboxes al final                      │
│                                                                 │
│  PipelineUseCase ── impl SandboxUseCase                         │
│    └── delega a BastionPipelineExecutor.execute(pipeline)       │
│                                                                 │
│  PipelineUseCaseFactory ── impl UseCaseFactory                  │
│    └── lee .bastion/pipelines/*.toml → Pipeline de Pipeliner    │
│                                                                 │
│  Dependencia: pipeliner crate (Pipeline, Stage, Step, etc.)     │
└─────────────────────────────────────────────────────────────────┘
```

**No hay DAG paralelo. No hay `UseCasePlan`. Pipeliner maneja toda la lógica de planificación. Bastion solo proporciona los sandboxes.**

### 3.3 `BastionPipelineExecutor` — El Puente

```rust
use pipeliner::executor::{PipelineExecutor, ExecutorCapabilities, HealthStatus, PipelineContext};
use pipeliner::pipeline::{Pipeline, Stage, StageResult, Step, StepType, Validate};
use bastion_domain::provider::{SandboxLifecycle, TaskExecutor};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::execution::command::CommandSpec;
use std::collections::HashMap;
use std::sync::Arc;

/// PipelineExecutor que delega a sandboxes de Bastion.
///
/// Este es el ÚNICO punto de integración entre Pipeliner y Bastion.
/// Pipeliner decide QUÉ ejecutar (DAG, when, post, parallel).
/// Bastion decide DÓNDE ejecutar (sandbox lifecycle, isolation, resources).
pub struct BastionPipelineExecutor<L, E>
where
    L: SandboxLifecycle,
    E: TaskExecutor,
{
    lifecycle: Arc<L>,
    executor: Arc<E>,
    /// Mapping de AgentType image → sandbox template name
    template_map: HashMap<String, String>,
    /// Sandbox IDs activos (para cleanup)
    active_sandboxes: tokio::sync::Mutex<Vec<SandboxId>>,
}

impl<L, E> PipelineExecutor for BastionPipelineExecutor<L, E>
where
    L: SandboxLifecycle + 'static,
    E: TaskExecutor + 'static,
{
    fn execute(&self, pipeline: &Pipeline) -> Result<StageResult, pipeliner::pipeline::PipelineError> {
        // 1. Construir contexto con environment del pipeline
        let mut context = PipelineContext::new();
        for (key, value) in &pipeline.environment.vars {
            context.set_env(key, value);
        }

        // 2. Ejecutar cada stage (Pipeliner controla el orden)
        for stage in &pipeline.stages {
            // 2a. Evaluar WhenCondition
            if let Some(ref when) = stage.when {
                if !self.eval_when(when, &context) {
                    context.record_stage_result(&stage.name, StageResult::Skipped);
                    continue;
                }
            }

            // 2b. Crear sandbox para este stage
            let template = self.resolve_template(&stage.agent);
            let sandbox_id = SandboxId::generate();
            let sandbox = self.lifecycle.create(
                &sandbox_id,
                &template,
                &Default::default(),  // ResourcesSpec from AgentType
                &Default::default(),  // NetworkSpec
                &context.env,         // Environment → sandbox env_vars
                pipeline.options.timeout.map(|d| d.as_millis() as u64).unwrap_or(300_000),
            ).map_err(|e| pipeliner::pipeline::PipelineError::AgentConfig(e.to_string()))?;

            // Track para cleanup
            self.active_sandboxes.lock().await.push(sandbox_id.clone());

            // 2c. Ejecutar steps dentro del sandbox
            let result = self.execute_stage_in_sandbox(stage, &sandbox_id, &context);

            // 2d. Ejecutar PostConditions
            for post in &stage.post {
                let last = context.get_stage_result(&stage.name).copied();
                if post.should_execute(result.clone(), last) {
                    let _ = self.execute_steps_in_sandbox(post.steps(), &sandbox_id, &context);
                }
            }

            // 2e. Record y decidir si continuar
            context.record_stage_result(&stage.name, result.clone());
            if result.is_failure() {
                return Ok(result);
            }
        }

        // 3. Pipeline post-conditions
        for post in &pipeline.post {
            let last = context.stage_results.values().last().copied();
            if let Some(last_result) = last {
                if post.should_execute(last_result, None) {
                    // Ejecutar en último sandbox o en uno nuevo
                }
            }
        }

        // 4. Cleanup: terminar sandboxes
        self.cleanup().await;

        Ok(StageResult::Success)
    }

    fn validate(&self, pipeline: &Pipeline) -> Result<(), pipeliner::pipeline::ValidationError> {
        pipeline.validate()
    }

    fn dry_run(&self, pipeline: &Pipeline) -> Result<StageResult, pipeliner::pipeline::PipelineError> {
        pipeline.validate()?;
        tracing::info!("Dry run: would execute {} stages", pipeline.stages.len());
        for stage in &pipeline.stages {
            tracing::info!("  Stage '{}': {} steps", stage.name, stage.steps.len());
        }
        Ok(StageResult::Success)
    }

    fn capabilities(&self) -> ExecutorCapabilities {
        ExecutorCapabilities {
            can_execute_shell: true,
            can_run_docker: true,     // via sandbox (Podman)
            can_run_kubernetes: false, // futuro
            supports_parallel: true,
            supports_caching: false,
            supports_timeout: true,
            supports_retry: true,
        }
    }

    fn health_check(&self) -> HealthStatus {
        // Check if sandbox provider is alive
        HealthStatus::Healthy  // TODO: actual health check
    }
}

impl<L, E> BastionPipelineExecutor<L, E>
where
    L: SandboxLifecycle,
    E: TaskExecutor,
{
    /// Ejecuta un Stage completo dentro de un sandbox.
    fn execute_stage_in_sandbox(
        &self,
        stage: &Stage,
        sandbox_id: &SandboxId,
        context: &PipelineContext,
    ) -> StageResult {
        // Parallel branches
        if !stage.parallel.is_empty() {
            // TODO: spawn tokio tasks, cada una con su sandbox
        }

        // Matrix execution
        if let Some(ref matrix) = stage.matrix {
            let combinations = matrix.generate_combinations();
            // TODO: spawn tokio tasks con env_override por combination
        }

        // Sequential steps
        match self.execute_steps_in_sandbox(&stage.steps, sandbox_id, context) {
            Ok(()) => StageResult::Success,
            Err(e) => StageResult::Failure, // simplificado
        }
    }

    /// Ejecuta Steps dentro de un sandbox — el corazón de la integración.
    ///
    /// Reemplaza `ShellCommand::execute()` de LocalExecutor
    /// con `TaskExecutor::run_command()` de Bastion.
    fn execute_steps_in_sandbox(
        &self,
        steps: &[Step],
        sandbox_id: &SandboxId,
        context: &PipelineContext,
    ) -> Result<(), pipeliner::pipeline::PipelineError> {
        for step in steps {
            match &step.step_type {
                StepType::Shell { command } => {
                    // AQUÍ está la magia: en vez de sh -c, delegamos al sandbox
                    let cmd = CommandSpec::new(command);
                    let result = self.executor.run_command(sandbox_id, &cmd)
                        .map_err(|e| pipeliner::pipeline::PipelineError::Io(e.to_string()))?;

                    if !result.is_success() {
                        return Err(pipeliner::pipeline::PipelineError::CommandFailed {
                            code: result.exit_code,
                            stderr: result.stderr,
                        });
                    }
                }
                StepType::Echo { message } => {
                    tracing::info!("[echo] {}", message);
                }
                StepType::Retry { count, step } => {
                    let mut last_err = None;
                    for attempt in 0..*count {
                        match self.execute_steps_in_sandbox(&[step.as_ref().clone()], sandbox_id, context) {
                            Ok(()) => { last_err = None; break; }
                            Err(e) => {
                                tracing::warn!("Retry {}/{}", attempt + 1, count);
                                last_err = Some(e);
                            }
                        }
                    }
                    if let Some(e) = last_err { return Err(e); }
                }
                StepType::Timeout { duration, step } => {
                    // TODO: tokio::time::timeout wrapper
                    self.execute_steps_in_sandbox(&[step.as_ref().clone()], sandbox_id, context)?;
                }
                StepType::Stash { name, includes } => {
                    // TODO: leer archivos del sandbox, guardar en .bastion/stash/
                }
                StepType::Unstash { name } => {
                    // TODO: escribir archivos al sandbox desde .bastion/stash/
                }
                StepType::Input { message, default } => {
                    // TODO: emitir evento SSE, esperar respuesta
                }
                StepType::Dir { path, steps } => {
                    // Ejecutar steps en directorio específico del sandbox
                    // Podemos hacer cd con: CommandSpec::new(&format!("cd {} && ...", path))
                    self.execute_steps_in_sandbox(steps, sandbox_id, context)?;
                }
            }
        }
        Ok(())
    }

    /// Traduce AgentType de Pipeliner a template de Bastion.
    fn resolve_template(&self, agent: &Option<pipeliner::pipeline::AgentType>) -> String {
        match agent {
            Some(pipeliner::pipeline::AgentType::Docker(c)) => {
                self.template_map.get(&c.image)
                    .cloned()
                    .unwrap_or_else(|| c.image.clone())
            }
            Some(pipeliner::pipeline::AgentType::Podman(c)) => {
                self.template_map.get(&c.image)
                    .cloned()
                    .unwrap_or_else(|| c.image.clone())
            }
            _ => "default".to_string(),
        }
    }

    /// Cleanup: termina todos los sandboxes creados.
    async fn cleanup(&self) {
        let mut sandboxes = self.active_sandboxes.lock().await;
        for id in sandboxes.drain(..) {
            if let Err(e) = self.lifecycle.terminate(&id).await {
                tracing::warn!("Failed to terminate sandbox {}: {}", id, e);
            }
        }
    }
}
```

### 3.4 `PipelineUseCase` — Implementación del Trait de Bastion

```rust
/// Implementación de SandboxUseCase que usa Pipeliner internamente.
///
/// Bastion ve un SandboxUseCase. Dentro, es un PipelineExecutor.
pub struct PipelineUseCase {
    pipeline: Pipeline,  // tipo de Pipeliner — el DSL completo
    executor: Arc<BastionPipelineExecutor<dyn SandboxLifecycle, dyn TaskExecutor>>,
}

#[async_trait]
impl SandboxUseCase for PipelineUseCase {
    fn kind(&self) -> &UseCaseKind { &UseCaseKind::Pipeline }
    fn name(&self) -> &str {
        self.pipeline.name.as_deref().unwrap_or("unnamed-pipeline")
    }

    async fn run(
        &self,
        provider: &dyn SandboxProvider,
        context: &UseCaseContext,
    ) -> Result<UseCaseResult, UseCaseError> {
        // Delega TODO al PipelineExecutor de Pipeliner
        let result = self.executor.execute(&self.pipeline)
            .map_err(UseCaseError::Pipeline)?;

        Ok(UseCaseResult {
            status: match result {
                StageResult::Success => UseCaseStatus::Success,
                StageResult::Failure => UseCaseStatus::Failure { reason: "Stage failed".into() },
                StageResult::Skipped => UseCaseStatus::Success, // skipped = ok
                StageResult::Unstable => UseCaseStatus::Partial { completed: 0, total: 0 },
            },
            started_at: Utc::now(),  // TODO: track real times
            finished_at: Utc::now(),
            sandbox_count: self.executor.active_sandbox_count().await,
            metadata: HashMap::new(),
        })
    }

    async fn cleanup(
        &self,
        provider: &dyn SandboxProvider,
    ) -> Result<(), UseCaseError> {
        self.executor.cleanup().await;
        Ok(())
    }
}
```

### 3.5 Flujo Completo — Cómo se Conecta Todo

```
Usuario: bastion pipeline run ci
    │
    ▼
CLI parsea "pipeline" → UseCaseKind::Pipeline
    │
    ▼
UseCaseRegistry.get(Pipeline) → PipelineUseCaseFactory
    │
    ▼
Factory lee .bastion/pipelines/ci.toml → Pipeline (Pipeliner type)
    │  Pipeline { name: "ci", stages: [check, test, lint], environment: {...}, ... }
    │
    ▼
Factory crea PipelineUseCase { pipeline, executor: BastionPipelineExecutor }
    │  BastionPipelineExecutor { lifecycle: PodmanAdapter, executor: PodmanAdapter }
    │
    ▼
Bastion llama: use_case.run(provider, context)
    │
    ▼
PipelineUseCase delega: executor.execute(pipeline)
    │
    ▼
BastionPipelineExecutor.execute(pipeline):
    │
    │  Para cada Stage en pipeline.stages:
    │    ├── Evalúa WhenCondition? → skip si no aplica
    │    ├── lifecycle.create(template, resources, env) → Sandbox
    │    ├── Para cada Step en stage.steps:
    │    │   └── TaskExecutor.run_command(sandbox_id, command)
    │    │       (en vez de sh -c, ejecuta dentro del sandbox)
    │    ├── Evalúa PostCondition → cleanup si aplica
    │    └── Record result → decide continue/abort
    │
    ▼
Cleanup: lifecycle.terminate() para cada sandbox
    │
    ▼
Result: UseCaseResult { status: Success/Failure, sandbox_count: N, ... }
```

### 3.6 ¿Qué gana Pipeliner con esto?

Pipeliner actualmente solo ejecuta en **local** (`LocalExecutor`). Con `BastionPipelineExecutor`, Pipeliner gana:

| Antes (LocalExecutor) | Después (BastionPipelineExecutor) |
|-----------------------|----------------------------------|
| Ejecuta en el host directamente | Ejecuta en sandboxes aislados |
| Sin aislamiento | Aislamiento completo (Podman/Firecracker/WASM) |
| Sin resource limits | Resource limits por sandbox |
| Sin snapshots | Snapshots para time-travel debugging |
| Sin gestión de lifecycle | Auto-cleanup, sleep/wake, timeout |
| Sin project context | Integrado con `.bastion/` |
| Sin cost attribution | Costos por pipeline/stage/sandbox |

Y Pipeliner no pierde nada — sigue funcionando standalone con `LocalExecutor` para uso simple. `BastionPipelineExecutor` es una implementación alternativa del mismo trait.

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

- [ ] Crear `bastion-domain/src/usecase/` con trait `SandboxUseCase` (3 métodos: kind, run, cleanup)
- [ ] Crear tipos mínimos: `UseCaseKind`, `UseCaseContext`, `UseCaseResult`, `UseCaseStatus`
- [ ] Crear `UseCaseRegistry` + `UseCaseFactory`
- [ ] Mover `PipelineDef` y `PipelineStage` fuera de `bastion-domain` → eliminar de domain core
- [ ] Actualizar `Project` aggregate: `pipelines: Vec<PipelineDef>` → `use_cases: Vec<UseCaseConfig>`

### Fase 1: BastionPipelineExecutor (2 semanas)

- [ ] Crear crate `bastion-pipeline` (depende de `bastion-domain` + `pipeliner`)
- [ ] Implementar `BastionPipelineExecutor` — `impl PipelineExecutor`
- [ ] Mapeo `AgentType` → sandbox template
- [ ] Implementar `execute_steps_in_sandbox()` — reemplaza `ShellCommand::execute()` con `TaskExecutor::run_command()`
- [ ] Implementar WhenCondition evaluation
- [ ] Implementar PostCondition execution
- [ ] Implementar cleanup de sandboxes
- [ ] Tests: `BastionPipelineExecutor` con mock provider

### Fase 2: PipelineUseCase (1 semana)

- [ ] Implementar `PipelineUseCase` — `impl SandboxUseCase` que delega a `BastionPipelineExecutor`
- [ ] Implementar `PipelineUseCaseFactory` — lee `.bastion/pipelines/*.toml` → `Pipeline` de Pipeliner
- [ ] TOML parser: TOML → `Pipeline` (usando serde de Pipeliner)
- [ ] Registrar `PipelineUseCaseFactory` en `UseCaseRegistry`
- [ ] Tests: flujo completo TOML → Pipeline → SandboxUseCase → Sandboxes

### Fase 3: StepTypes Avanzados (1 semana)

- [ ] `Stash/Unstash` → leer/escribir archivos via `TaskExecutor::read_file/write_file`
- [ ] `Retry` → loop con delay en `execute_steps_in_sandbox`
- [ ] `Timeout` → `tokio::time::timeout` wrapper
- [ ] `Input` → emitir SSE event, esperar respuesta
- [ ] `Dir` → prefijo de directorio en CommandSpec
- [ ] Parallel branches → spawn tokio tasks con sandboxes paralelos
- [ ] Matrix → generar combinaciones, cada una con su sandbox

### Fase 4: GISS-Inspired Data Models (1 semana)

- [ ] Diseñar `project.toml` schema con tools, strategies, artifacts-repositories
- [ ] Diseñar `cache.toml` schema (cache persistente entre runs)
- [ ] Diseñar `releases/` schema (ReleaseDescriptor adaptado)
- [ ] Integrar con Pipeliner: capabilities como Stage templates

### Fase 5: CLI + Dashboard (2 semanas)

- [ ] CLI: `bastion run --use-case pipeline --config ci.toml`
- [ ] CLI: `bastion use-case list`
- [ ] Dashboard: UseCase API endpoints
- [ ] Dashboard: Pipeline visualization (via Pipeliner stage results)
- [ ] Dashboard: Cost attribution por use case

### Fase 6: Extensibilidad (ongoing)

- [ ] Implementar `AdHocTestUseCase` (sandbox + command, el más simple)
- [ ] Implementar `BatchJobUseCase` (sandbox + command + cleanup)
- [ ] Documentar cómo crear un `SandboxUseCase` custom
- [ ] Plugin system: cargar casos de uso desde crates externos

---

## 7. Cambios en el Modelo de Dominio Existente

### 7.1 Eliminar de `bastion-domain` (core)

```rust
// ELIMINAR de project/types.rs:
pub struct PipelineDef { ... }
pub struct PipelineStage { ... }
// → Se mueven a bastion-pipeline crate (usa tipos de Pipeliner directamente)
```

### 7.2 Añadir a `bastion-domain` (core) — Mínimo

```rust
// NUEVO module: usecase/ (~150 LOC total)
pub trait SandboxUseCase { fn kind(), name(), run(), cleanup() }  // ~30 LOC
pub enum UseCaseKind { Pipeline, AdHocTest, ProofOfConcept, E2eTest, BatchJob, Custom(String) }
pub struct UseCaseContext { project_id, bastion_dir, environment, git_info }
pub struct UseCaseResult { status, started_at, finished_at, sandbox_count, metadata }
pub enum UseCaseStatus { Success, Failure, Partial }
pub trait UseCaseFactory { fn kind(), create() }
pub struct UseCaseRegistry { factories }
```

### 7.3 Añadir a `bastion-pipeline` (crate nuevo) — Toda la lógica

```rust
// BastionPipelineExecutor — impl PipelineExecutor (~200 LOC)
// PipelineUseCase — impl SandboxUseCase (~50 LOC)
// PipelineUseCaseFactory — impl UseCaseFactory (~80 LOC)
// TOML parser — TOML → Pipeline (~100 LOC)
```

### 7.4 Modificar en `bastion-domain`

```rust
// project/aggregate.rs - Project struct
pub struct Project {
    ...
-   pub pipelines: Vec<PipelineDef>,           // ELIMINAR
+   pub use_cases: Vec<UseCaseConfig>,          // NUEVO — configs genéricas
    ...
}

// SandboxPurpose — simplificar
pub enum SandboxPurpose {
    AdHocTest,
    ProofOfConcept,
    E2eTest,
    RealTest,
-   PipelineStage,           // Ya no es un propósito especial
    Job,
+   UseCase(String),         // Caso de uso genérico
}
```

### 7.5 Dependencias entre Crates

```
bastion-domain          (core, sin dependencias externas)
    ↑
bastion-infrastructure  (implementaciones concretas de providers)
    ↑
bastion-pipeline        (depende de bastion-domain + pipeliner)
    ↑
bastion-gateway         (CLI + MCP + HTTP server)
```

`bastion-domain` NO depende de `pipeliner`. Solo `bastion-pipeline` conoce Pipeliner.

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

**Bastion orquesta sandboxes. Pipeliner orquesta pipelines. Son capas complementarias, no competitivas.**

La integración tiene **un único punto de costura**: `impl PipelineExecutor for BastionPipelineExecutor`.

- Pipeliner decide **QUÉ** ejecutar (DAG, stages, when, post, parallel, matrix)
- Bastion decide **DÓNDE** ejecutar (sandbox lifecycle, isolation, resources, cleanup)
- El `BastionPipelineExecutor` traduce entre ambos mundos: `ShellCommand::execute()` → `TaskExecutor::run_command()`

No hay `UseCasePlan`, `UseCaseStep`, ni DAG paralelo. Pipeliner tiene su DSL completo y lo usamos directamente.

El trait `SandboxUseCase` es la abstracción mínima de Bastion para extensibilidad — pero Pipeliner no lo "ve". Pipeliner solo ve `PipelineExecutor`, que es SU trait.

**Principio**: cada proyecto usa su propio trait como interfaz natural:
- Pipeliner → `PipelineExecutor` (su API de ejecución)
- Bastion → `SandboxUseCase` (su API de extensión)
- `bastion-pipeline` → el puente que conecta ambos

```
Pipeliner          bastion-pipeline         Bastion
─────────          ────────────────         ───────
Pipeline           BastionPipeline          SandboxLifecycle
Stage               Executor                 TaskExecutor
Step                   │                      SandboxProvider
PipelineExecutor ──────┘
  (impl)               │
                       └─── SandboxUseCase (impl PipelineUseCase)
                            (Bastion solo ve esto)
```

---

*Documento de estrategia v2 — 2026-05-12*
*Proyectos: Bastion + Pipeliner + GISS Framework*
