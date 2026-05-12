# Bastion Dashboard — Propuesta de Arquitectura v2 (Project-Centric)

> **Fecha**: 2026-05-12
> **Estado**: Propuesta revisada con enfoque project-centric
> **Tipo**: Documento de arquitectura y diseño técnico
> **Revisión**: v2 — Integra feedback de enfoque por proyectos con `.bastion/`

---

## Resumen Ejecutivo

Bastion no es Portainer. **Bastion orquesta sandboxes para proyectos de software** — PoCs, testing e2e, test reales, jobs, pipelines — todo asociado a un proyecto con su repositorio Git y su directorio `.bastion/`.

El dashboard propuesto es **project-centric**: el proyecto es la unidad fundamental, no el sandbox. Cada proyecto tiene:

- Un **repositorio Git local**
- Un **directorio `.bastion/`** con configuración, BBDD, métricas, catálogos
- **Sandboxes** asociados al proyecto para testing, PoCs, pipelines
- **Pipelines** declarativos que ejecutan stages en sandboxes aislados

**Stack tecnológico**: Leptos 0.7+ (WASM) + Tailwind CSS 4.x (Oxide engine) + Axum (backend)

---

## 1. Modelo Fundamental: Project-Centric

### 1.1 El Proyecto como Ciudadano de Primera Clase

```
mi-proyecto/
├── .git/                          # Repositorio Git
├── .bastion/                      # ← Datos de Bastion (como .git para Git)
│   ├── config.toml                # Configuración del proyecto
│   ├── project.toml               # Metadatos del proyecto (nombre, tipo, runtime)
│   ├── providers/                 # Providers del proyecto
│   │   └── podman.toml
│   ├── capabilities/              # Capacidades (node-build, rust-build, etc.)
│   │   └── rust-build.toml
│   ├── catalog/                   # Catálogos: doctors, advice, assertions
│   │   ├── advice/
│   │   ├── doctors/
│   │   └── assertions/
│   ├── pipelines/                 # Pipeline definitions
│   │   └── ci.toml
│   ├── templates/                 # Sandbox templates
│   │   └── rust-ci.toml
│   ├── db/                        # SQLite databases
│   │   ├── sandboxes.db           # Sandbox state
│   │   ├── metrics.db             # MetricsHub data
│   │   └── enrichment.db         # Experience records
│   ├── runtime/                   # Runtime state (PID, logs)
│   │   ├── bastion-gateway.pid
│   │   └── bastion-gateway.log
│   ├── advice.toml                # Advice configuration
│   └── .gitignore                 # Ignore DB files, PID files, logs
├── src/                           # Project source code
├── Cargo.toml
└── README.md
```

### 1.2 Anatomía de un Proyecto

```rust
/// Un proyecto de Bastion — la unidad fundamental del dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Unique project identifier
    pub id: ProjectId,

    /// Human-readable name
    pub name: String,

    /// Absolute path to project root (where .bastion/ lives)
    pub path: PathBuf,

    /// Project type (detected or configured)
    pub kind: ProjectKind,

    /// Git repository info
    pub git: GitInfo,

    /// Associated sandboxes (active + recent)
    pub sandboxes: Vec<SandboxSummary>,

    /// Pipeline definitions
    pub pipelines: Vec<PipelineDef>,

    /// Pool configuration for this project
    pub pool_config: PoolConfig,

    /// Resource limits for this project
    pub resource_limits: ResourceLimits,

    /// Last activity timestamp
    pub last_active: DateTime<Utc>,

    /// Cost attribution
    pub cost: CostSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProjectKind {
    Rust,
    NodeJs,
    Python,
    Java,
    Go,
    Container,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub commit: String,
    pub dirty: bool,
    pub remote_url: Option<String>,
    pub stash_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_sandboxes: usize,
    pub max_cpu_per_sandbox: f32,
    pub max_memory_per_sandbox_mb: usize,
    pub max_sandbox_lifetime: Duration,
    pub auto_sleep_timeout: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub sandbox_hours: f32,
    pub cpu_hours: f32,
    pub estimated_cost_usd: f32,
    pub period: DateRange,
}
```

### 1.3 Sandbox Pertenece a un Proyecto

```rust
/// Un sandbox SIEMPRE pertenece a un proyecto
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sandbox {
    pub id: SandboxId,
    pub project_id: ProjectId,         // ← Requerido

    /// Why was this sandbox created?
    pub purpose: SandboxPurpose,

    /// Which pipeline/job created this? (optional)
    pub pipeline_id: Option<PipelineId>,

    /// Which git commit is this sandbox testing?
    pub git_commit: Option<String>,

    /// Template used
    pub template: String,

    /// Runtime type
    pub runtime: RuntimeType,

    /// Status
    pub status: SandboxStatus,

    /// Resource usage
    pub cpu_usage: f32,
    pub memory_usage_mb: usize,
    pub disk_usage_mb: usize,

    /// Timing
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,

    /// Associated test results
    pub test_results: Vec<TestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxPurpose {
    /// Quick ad-hoc testing
    AdHocTest,

    /// PoC validation
    ProofOfConcept { description: String },

    /// E2E test run
    E2eTest { suite: String },

    /// Real test run (unit, integration)
    RealTest { framework: String },

    /// CI/CD pipeline stage
    PipelineStage { pipeline: String, stage: String },

    /// Job execution (batch, cron)
    Job { job_name: String },
}
```

### 1.4 Pipeline Asociado a un Proyecto

```toml
# .bastion/pipelines/ci.toml

[pipeline]
name = "ci"
description = "CI pipeline for this project"

[[stages]]
name = "check"
template = "rust-ci"
purpose = "real-test"
commands = ["cargo check --workspace"]
on_failure = "stop"

[[stages]]
name = "test"
template = "rust-ci"
purpose = "real-test"
commands = ["cargo test --workspace"]
depends_on = ["check"]
on_failure = "stop"

[[stages]]
name = "e2e"
template = "debian-bookworm"
purpose = "e2e-test"
commands = ["cargo test -p bastion-gateway --test e2e_test -- --test-threads=1"]
depends_on = ["test"]
on_failure = "report"

[[stages]]
name = "lint"
template = "rust-ci"
purpose = "real-test"
commands = ["cargo clippy --workspace -- -D warnings"]
depends_on = ["check"]
parallel_with = ["test"]
on_failure = "report"

[pipeline.policy]
max_lifetime = "1h"
auto_sleep = "15m"
retry_count = 2
cleanup_on_success = true
```

---

## 2. Research de Reference Technologies

*(Resumen ejecutivo — ver sección 1 del documento original para detalle completo)*

| Plataforma | Lección Clave para Bastion |
|------------|---------------------------|
| **Portainer** | Edge agent con conectividad invertida (outbound TLS). Agentes stateless, server con estado. |
| **Docker Desktop** | Resource Saver (idle → sleep → wake). Enhanced Container Isolation. Extensions SDK. |
| **Lens** | UI tipo IDE: Navigator + Tabs + Dock + Command Palette. Watch API en vez de polling. |
| **Okteto** | Lifecycle automation: GC, sleep/wake, Resource Manager. Manifiestos declarativos. Dev containers. |

**Lo que diferencia a Bastion**: las 4 plataformas gestionan **infraestructura**. Bastion gestiona **proyectos de software**. El sandbox no es un contenedor genérico — es un ambiente efímero al servicio de un proyecto específico.

---

## 3. Arquitectura Project-Centric

### 3.1 Arquitectura General

```
┌──────────────────────────────────────────────────────────────────────┐
│                    Bastion Dashboard (Leptos WASM)                  │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                     PROJECT NAVIGATOR                         │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐                  │  │
│  │  │ Project A│  │ Project B│  │ Project C│  [+ New Project]    │  │
│  │  │ Rust     │  │ Node.js  │  │ Python   │                    │  │
│  │  │ 3 active │  │ 1 active │  │ 0 active │                    │  │
│  │  └──────────┘  └──────────┘  └──────────┘                    │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                     WORKSPACE (Selected Project)               │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐   │  │
│  │  │Sandboxes │ │ Pipelines│ │ Tests    │ │ Metrics      │   │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────────┘   │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                     DOCK (Terminal/Logs/Details)               │  │
│  └────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ REST API + SSE + WebSocket
                                    ▼
┌──────────────────────────────────────────────────────────────────────┐
│                     Bastion Gateway (Axum)                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
│  │ MCP Tools    │  │ REST API v1  │  │  SSE Event Stream       │  │
│  │ (existing)  │  │ /api/v1/*    │  │  /api/v1/events          │  │
│  └──────────────┘  └──────────────┘  └──────────────────────────┘  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
│  │ Project Mgr │  │ .bastion/    │  │  MetricsHub (SQLite)     │  │
│  │ (new)        │  │ config loader│  │  (per-project DB)        │  │
│  └──────────────┘  └──────────────┘  └──────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
         │                    │                    │
    ┌────┘────┐         ┌────┘────┐         ┌─────┘─────┐
    │ Podman  │         │Firecracker│        │   WASM    │
    │ Worker  │         │  Worker   │        │  Worker   │
    └─────────┘         └──────────┘        └───────────┘
```

### 3.2 Project Manager — El Nuevo Componente Central

```rust
/// ProjectManager: gestiona la relación entre proyectos y sandboxes
pub struct ProjectManager {
    /// Active projects in memory
    projects: DashMap<ProjectId, Project>,

    /// Project → Sandboxes mapping
    project_sandboxes: DashMap<ProjectId, Vec<SandboxId>>,

    /// .bastion/ config loader
    config_loader: ConfigLoader,

    /// Git integration
    git: GitIntegration,
}

impl ProjectManager {
    /// Open a project from a directory containing .bastion/
    pub async fn open_project(&self, path: &Path) -> Result<Project> {
        let bastion_dir = path.join(".bastion");
        if !bastion_dir.exists() {
            return Err(ProjectError::NotABastionProject(path.to_path_buf()));
        }
        // Load config.toml, project.toml, etc.
        let config = self.config_loader.load(&bastion_dir).await?;
        let git_info = self.git.info(path)?;
        // Discover sandboxes for this project
        let sandboxes = self.discover_sandboxes(&config).await?;
        Ok(Project { path: path.into(), git: git_info, .. })
    }

    /// Initialize .bastion/ in a directory (like git init)
    pub async fn init_project(&self, path: &Path, kind: ProjectKind) -> Result<Project> {
        let bastion_dir = path.join(".bastion");
        // Create directory structure
        self.create_bastion_structure(&bastion_dir, kind).await?;
        // Generate initial config
        self.write_default_configs(&bastion_dir, kind).await?;
        // Add .gitignore
        self.write_gitignore(&bastion_dir).await?;
        self.open_project(path).await
    }

    /// Create a sandbox for a specific project and purpose
    pub async fn create_sandbox(
        &self,
        project_id: &ProjectId,
        purpose: SandboxPurpose,
        template: &str,
    ) -> Result<Sandbox> {
        let project = self.projects.get(project_id)
            .ok_or(ProjectError::ProjectNotFound(project_id.clone()))?;

        // Check resource limits
        self.check_limits(project_id).await?;

        // Create sandbox with project context
        let sandbox = self.provider.create(
            template,
            &project.resource_limits,
        ).await?;

        // Store association
        self.project_sandboxes
            .entry(project_id.clone())
            .or_default()
            .push(sandbox.id.clone());

        // Persist to project's .bastion/db/sandboxes.db
        self.persist_sandbox(project_id, &sandbox).await?;

        Ok(sandbox)
    }
}
```

### 3.3 `.bastion/` como Contract — Integración con Componentes Existentes

El directorio `.bastion/` ya existe y es funcional. El dashboard lo **lee y escribe** como fuente de verdad:

| Componente Existente | `.bastion/` Path | Dashboard Action |
|----------------------|-------------------|-----------------|
| Provider configs | `.bastion/providers/*.toml` | Leer, crear, editar templates |
| Capabilities | `.bastion/capabilities/*.toml` | Leer, crear nuevas capabilities |
| Doctors/Advice | `.bastion/catalog/**/*.toml` | Leer, toggle enable/disable |
| Advice config | `.bastion/advice.toml` | Leer, editar |
| Runtime state | `.bastion/runtime/` | Leer PID, logs |
| SQLite DBs | `.bastion/db/*.db` | Consultar métricas, historial |
| **NUEVO** Pipelines | `.bastion/pipelines/*.toml` | Leer, crear, editar, ejecutar |
| **NUEVO** Project config | `.bastion/project.toml` | Leer, editar |
| **NUEVO** Dashboard state | `.bastion/dashboard.json` | Filtros, vistas preferidas |

---

## 4. Layout del Dashboard (Project-Centric)

### 4.1 Vista Principal — Project Selector

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▸ Bastion                                           [+ New Project]│
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  📂 Recent Projects                                            [🔍] │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 🦀 Bastion                                    3 sandboxes   │   │
│  │ ~/Proyectos/rust/Bastion                      branch: main  │   │
│  │ Rust · 2 tests running · 1 pool active · CPU 12%          │   │
│  │ Last activity: 2 min ago                                    │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 🐍 DataPipeline                               0 sandboxes   │   │
│  │ ~/Proyectos/python/datapipeline               branch: dev   │   │
│  │ Python · idle · no active sandboxes                         │   │
│  │ Last activity: 3 days ago                                   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ ☕ PetClinic                                   1 sandbox    │   │
│  │ ~/Proyectos/java/spring-petclinic              branch: feat  │   │
│  │ Java · 1 e2e-test running · CPU 45%                         │   │
│  │ Last activity: 30 sec ago                                    │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 📋 Quick Actions                                             │   │
│  │ [New Sandbox] [Run Pipeline] [New Project] [Import Config]   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  💡 1 sandbox sleeping for >24h in Bastion. Consider terminating? │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### 4.2 Vista de Proyecto — Workspace Principal

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▸ Bastion > 🦀 Bastion                                    [⚙️] [🔔]│
├──────────────────────────────────────────────────────────────────────┤
│ OVERVIEW │ SANDBOXES │ PIPELINES │ TESTS │ METRICS │ CONFIG │ DOCS │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────────┐   │
│  │ Active     │ │ Created    │ │ Failed     │ │ Avg Latency   │   │
│  │    3       │ │  1,234     │ │   0.2%     │ │   145ms       │   │
│  │ ▴ 2 from  │ │ today: 42  │ │ ▾ 0.1%     │ │ ▴ -12ms       │   │
│  │   pool     │ │            │ │            │ │                │   │
│  └────────────┘ └────────────┘ └────────────┘ └────────────────┘   │
│                                                                      │
│  🏗️ Active Sandboxes                                           [+]   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 🟢 sb-bast-abc123  │ E2e Test  │ 42m │ CPU 5% │ MEM 120MB │   │
│  │    rust-ci template · branch:main · commit:a1b2c3           │   │
│  │    [Terminal] [Logs] [Details] [Sleep] [Terminate]         │   │
│  ├──────────────────────────────────────────────────────────────┤   │
│  │ 🟢 sb-bast-def456  │ Pool      │ 2h  │ CPU 1% │ MEM 80MB  │   │
│  │    debian:bookworm · idle · ready for use                  │   │
│  │    [Terminal] [Assign] [Details] [Sleep] [Terminate]      │   │
│  ├──────────────────────────────────────────────────────────────┤   │
│  │ 🟡 sb-bast-ghi789  │ AdHoc PoC │ 5m  │ CPU 12% │ MEM 200MB │   │
│  │    rust:latest · testing new tokio patterns                 │   │
│  │    [Terminal] [Logs] [Details] [Sleep] [Terminate]        │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  📋 Recent Pipelines                                           [▶ ▌]  │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ CI Pipeline  #1247 · main · a1b2c3                     ✅   │   │
│  │ ├─ check ── ✅ (12s)                                       │   │
│  │ ├─ test  ── ✅ (3m 42s)                                    │   │
│  │ ├─ lint  ── ✅ (28s)   ║ parallel with test               │   │
│  │ └─ e2e   ── ✅ (1m 12s)                                    │   │
│  │ Total: 4m 34s · Cost: $0.02                                 │   │
│  ├──────────────────────────────────────────────────────────────┤   │
│  │ CI Pipeline  #1246 · main · e4d5f6                     ❌   │   │
│  │ ├─ check ── ✅ (11s)                                       │   │
│  │ └─ test  ── ❌ (2m 15s) · 3 tests failed                 │   │
│  │    sandbox_run: exit code 1                                │   │
│  │ Total: 2m 26s · [View Logs] [Rerun]                        │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  💡 1 sandbox sleeping for >24h. [Terminate] [Wake] [Ignore]      │
└──────────────────────────────────────────────────────────────────────┘
```

### 4.3 Componentes Leptos — Project-Centric

```rust
#[component]
pub fn ProjectCard(project: Project) -> impl IntoView {
    let active_count = project.sandboxes.iter()
        .filter(|s| matches!(s.status, SandboxStatus::Ready | SandboxStatus::Running))
        .count();

    let kind_icon = match project.kind {
        ProjectKind::Rust => "🦀",
        ProjectKind::NodeJs => "🟢",
        ProjectKind::Python => "🐍",
        ProjectKind::Java => "☕",
        ProjectKind::Go => "🔵",
        _ => "📦",
    };

    view! {
        <div class="card hover:border-primary-500 transition-colors cursor-pointer
                    group relative overflow-hidden">
            // Subtle gradient on hover
            <div class="absolute inset-0 bg-gradient-to-r from-primary-500/5 to-transparent
                        opacity-0 group-hover:opacity-100 transition-opacity"></div>

            <div class="relative">
                <div class="flex items-center gap-3">
                    <span class="text-2xl">{kind_icon}</span>
                    <div class="flex-1 min-w-0">
                        <h3 class="font-semibold text-white truncate">{project.name}</h3>
                        <p class="text-sm text-muted truncate">{project.path.display()}</p>
                    </div>
                    <div class="text-right">
                        <span class="text-sm font-medium text-primary-400">
                            {active_count} " sandbox" {if active_count != 1 { "es" } else { "" }}
                        </span>
                        <p class="text-xs text-muted">{format_ago(project.last_active)}</p>
                    </div>
                </div>

                <div class="mt-2 flex gap-2 text-xs text-muted">
                    <span class="px-2 py-0.5 bg-slate-700/50 rounded">
                        {project.git.branch}
                    </span>
                    {if project.git.dirty {
                        view! { <span class="px-2 py-0.5 bg-yellow-500/20 text-yellow-400 rounded">"dirty"</span> }
                    } else {
                        view! { <span class="px-2 py-0.5 bg-green-500/20 text-green-400 rounded">"clean"</span> }
                    }}
                </div>
            </div>
        </div>
    }
}
```

```rust
#[component]
pub fn PipelineVisualization(pipeline: PipelineExecution) -> impl IntoView {
    view! {
        <div class="card">
            <div class="flex items-center justify-between mb-3">
                <div>
                    <h3 class="font-semibold">{pipeline.name}</h3>
                    <p class="text-xs text-muted">
                        "#" {pipeline.number} " · "
                        {pipeline.git_branch} " · "
                        {pipeline.git_commit[..7]}
                    </p>
                </div>
                <PipelineStatusBadge status={pipeline.status} />
            </div>

            // Stage graph
            <div class="flex items-center gap-1 flex-wrap">
                <For each={pipeline.stages}>
                    {stage| view! {
                        <StageNode
                            name={stage.name}
                            status={stage.status}
                            duration={stage.duration}
                            depends_on={stage.depends_on}
                        />
                    }}
                </For>
            </div>

            <div class="mt-3 flex items-center justify-between text-sm text-muted">
                <span>"Total: " {format_duration(pipeline.total_duration)}</span>
                <span>"Cost: $" {format!("{:.2}", pipeline.cost_usd)}</span>
            </div>
        </div>
    }
}
```

---

## 5. API REST — Project-Centric

### 5.1 Endpoints

```
# Projects
GET    /api/v1/projects                              # List projects
POST   /api/v1/projects                              # Open/init project
GET    /api/v1/projects/:id                          # Get project detail
GET    /api/v1/projects/:id/status                  # Project status summary
GET    /api/v1/projects/:id/costs                   # Cost attribution

# Sandboxes (scoped to project)
GET    /api/v1/projects/:id/sandboxes                # List project sandboxes
POST   /api/v1/projects/:id/sandboxes                # Create sandbox for project
GET    /api/v1/projects/:id/sandboxes/:sid           # Get sandbox detail
DELETE /api/v1/projects/:id/sandboxes/:sid           # Terminate sandbox
POST   /api/v1/projects/:id/sandboxes/:sid/sleep     # Sleep sandbox
POST   /api/v1/projects/:id/sandboxes/:sid/wake      # Wake sandbox
GET    /api/v1/projects/:id/sandboxes/:sid/terminal   # Terminal WS
GET    /api/v1/projects/:id/sandboxes/:sid/logs      # Logs SSE

# Pipelines (scoped to project)
GET    /api/v1/projects/:id/pipelines                # List pipeline definitions
POST   /api/v1/projects/:id/pipelines                # Create pipeline def
GET    /api/v1/projects/:id/pipelines/:pid            # Get pipeline definition
POST   /api/v1/projects/:id/pipelines/:pid/run        # Run a pipeline
GET    /api/v1/projects/:id/pipelines/:pid/runs       # List pipeline runs
GET    /api/v1/projects/:id/pipelines/:pid/runs/:rid  # Get run detail

# Tests (scoped to project)
GET    /api/v1/projects/:id/tests                    # List test definitions
POST   /api/v1/projects/:id/tests/run                # Run tests

# Metrics (scoped to project)
GET    /api/v1/projects/:id/metrics                  # Project metrics
GET    /api/v1/projects/:id/metrics/historical        # Historical metrics

# Config (scoped to project, reads/writes .bastion/)
GET    /api/v1/projects/:id/config                   # Read .bastion/config.toml
PUT    /api/v1/projects/:id/config                   # Update config
GET    /api/v1/projects/:id/providers                # List provider configs
GET    /api/v1/projects/:id/capabilities              # List capability configs

# Events (SSE)
GET    /api/v1/events                                 # All events
GET    /api/v1/projects/:id/events                    # Project-scoped events
```

### 5.2 Server Functions (Leptos)

```rust
#[server]
pub async fn list_projects() -> Result<Vec<Project>, ServerError> {
    let state = use_axum_state()?;
    state.project_manager.list_projects().await.map_err(Into::into)
}

#[server]
pub async fn open_project(path: String) -> Result<Project, ServerError> {
    let state = use_axum_state()?;
    state.project_manager.open_project(Path::new(&path)).await.map_err(Into::into)
}

#[server]
pub async fn create_sandbox_for_project(
    project_id: ProjectId,
    purpose: SandboxPurpose,
    template: String,
) -> Result<Sandbox, ServerError> {
    let state = use_axum_state()?;
    state.project_manager
        .create_sandbox(&project_id, purpose, &template)
        .await
        .map_err(Into::into)
}

#[server]
pub async fn run_pipeline(
    project_id: ProjectId,
    pipeline_name: String,
) -> Result<PipelineRun, ServerError> {
    let state = use_axum_state()?;
    state.pipeline_executor
        .run(&project_id, &pipeline_name)
        .await
        .map_err(Into::into)
}

#[server]
pub async fn get_project_metrics(
    project_id: ProjectId,
    range: DateRange,
) -> Result<ProjectMetrics, ServerError> {
    let state = use_axum_state()?;
    state.metrics_hub
        .get_project_metrics(&project_id, range)
        .await
        .map_err(Into::into)
}
```

---

## 6. Eventos en Tiempo Real — Project-Scoped

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProjectEvent {
    // Project lifecycle
    ProjectOpened { id: ProjectId, name: String, kind: ProjectKind },
    ProjectClosed { id: ProjectId },

    // Sandbox lifecycle (scoped to project!)
    SandboxCreated { project_id: ProjectId, sandbox_id: SandboxId, purpose: SandboxPurpose },
    SandboxReady { project_id: ProjectId, sandbox_id: SandboxId },
    SandboxSleeping { project_id: ProjectId, sandbox_id: SandboxId },
    SandboxTerminated { project_id: ProjectId, sandbox_id: SandboxId, reason: TerminateReason },
    SandboxFailed { project_id: ProjectId, sandbox_id: SandboxId, error: String },

    // Resource updates
    SandboxMetricsUpdated { project_id: ProjectId, sandbox_id: SandboxId, cpu: f32, memory_mb: u32 },

    // Pipeline events
    PipelineStarted { project_id: ProjectId, pipeline_id: PipelineId, run_id: RunId },
    PipelineStageCompleted { project_id: ProjectId, run_id: RunId, stage: String, result: StageResult },
    PipelineCompleted { project_id: ProjectId, pipeline_id: PipelineId, run_id: RunId, result: PipelineResult },

    // Pool events
    PoolScaleUp { project_id: ProjectId, runtime: RuntimeType, new_size: usize },
    PoolScaleDown { project_id: ProjectId, runtime: RuntimeType, new_size: usize },

    // Cost events
    CostThresholdReached { project_id: ProjectId, current_usd: f32, threshold_usd: f32 },

    // Recommendations
    IdleSandboxNotice { project_id: ProjectId, sandbox_id: SandboxId, idle_minutes: u32 },
    CostOptimizationTip { project_id: ProjectId, tip: String, potential_savings_usd: f32 },
}
```

---

## 7. Integración con Componentes Existentes

### 7.1 MetricsHub → Project-Scoped

```rust
// Current: MetricsHub is global
// Proposed: MetricsHub becomes project-scoped

pub struct ProjectMetricsHub {
    /// Per-project SQLite connections
    hubs: DashMap<ProjectId, Arc<tokio::sync::Mutex<MetricsHub>>>,

    /// Base directory for project databases
    base_path: PathBuf,  // defaults to project_path/.bastion/db/
}

impl ProjectMetricsHub {
    pub async fn get_hub(&self, project_id: &ProjectId) -> Result<Arc<tokio::sync::Mutex<MetricsHub>>> {
        if let Some(hub) = self.hubs.get(project_id) {
            return Ok(Arc::clone(hub.value()));
        }

        // Lazy init: open project's .bastion/db/metrics.db
        let project = self.project_manager.get(project_id).await?;
        let db_path = project.path.join(".bastion/db/metrics.db");

        let hub = MetricsHub::new(&db_path).await?;
        let arc = Arc::new(tokio::sync::Mutex::new(hub));
        self.hubs.insert(project_id.clone(), Arc::clone(&arc));

        Ok(arc)
    }
}
```

### 7.2 HeartbeatBridge → Project-Scoped

```rust
// Resource data flows from Worker → RegistryService → Dashboard per project
pub struct ProjectResourceTracker {
    /// Per-project resource snapshots
    resources: DashMap<ProjectId, DashMap<SandboxId, SandboxResources>>,
}

impl ProjectResourceTracker {
    pub fn update(&self, project_id: &ProjectId, sandbox_id: &SandboxId, resources: SandboxResources) {
        self.resources
            .entry(project_id.clone())
            .or_default()
            .insert(sandbox_id.clone(), resources);
    }

    pub fn get_project_total(&self, project_id: &ProjectId) -> ProjectResourceSummary {
        let resources = self.resources.get(project_id);
        let mut summary = ProjectResourceSummary::default();
        if let Some(res) = resources {
            for (_, r) in res.iter() {
                summary.total_cpu += r.cpu_usage;
                summary.total_memory_mb += r.memory_usage_mb;
                summary.sandbox_count += 1;
            }
        }
        summary
    }
}
```

### 7.3 Enrichment Engine → Project-Scoped

El enrichment engine ya usa `.bastion/catalog/` — el dashboard lo integra:

```rust
#[server]
pub async fn get_project_catalog(project_id: ProjectId) -> Result<ProjectCatalog, ServerError> {
    let project = get_project(&project_id).await?;
    let catalog_dir = project.path.join(".bastion/catalog");

    Ok(ProjectCatalog {
        advice: load_toml_dir(&catalog_dir.join("advice")).await?,
        doctors: load_toml_dir(&catalog_dir.join("doctors")).await?,
        assertions: load_toml_dir(&catalog_dir.join("assertions")).await?,
    })
}

#[server]
pub async fn toggle_advice(
    project_id: ProjectId,
    advice_id: String,
    enabled: bool,
) -> Result<(), ServerError> {
    let project = get_project(&project_id).await?;
    let advice_path = project.path.join(".bastion/advice.toml");

    // Read, modify, write advice config
    let mut config: AdviceConfig = read_toml(&advice_path).await?;
    if enabled {
        config.advice.disabled.retain(|id| id != &advice_id);
    } else {
        config.advice.disabled.push(advice_id);
    }
    write_toml(&advice_path, &config).await?;

    Ok(())
}
```

---

## 8. Pensamiento Lateral — Innovaciones Project-Centric

### 8.1 `bastion init` — Como `git init`

```bash
# Inicializar un proyecto de Bastion (como git init)
cd ~/Proyectos/rust/Bastion
bastion init --kind rust

# Output:
# ✔ Created .bastion/ directory structure
# ✔ Detected Rust project (Cargo.toml)
# ✔ Generated .bastion/project.toml
# ✔ Generated .bastion/providers/podman.toml
# ✔ Generated .bastion/capabilities/rust-build.toml
# ✔ Generated .bastion/pipelines/ci.toml
# ✔ Generated .bastion/.gitignore
# ✔ Project "Bastion" initialized at /home/user/Proyectos/rust/Bastion
```

### 8.2 `bastion dashboard` — Servir la UI

```bash
# Abrir el dashboard para un proyecto
cd ~/Proyectos/rust/Bastion
bastion dashboard

# Output:
# ✔ Starting Bastion Dashboard for "Bastion"
# ✔ Gateway running on :50051
# ✔ Dashboard available at http://localhost:3000
# ✔ Connected to .bastion/ at /home/user/Proyectos/rust/Bastion/.bastion
# ✔ 3 sandboxes active
```

### 8.3 Git Hooks Integration

```bash
# .bastion/hooks/pre-commit
bastion sandbox run --purpose real-test --template rust-ci -- cargo test --workspace

# .bastion/hooks/pre-push
bastion pipeline run ci

# .bastion/hooks/post-merge
bastion sandbox cleanup --stale
```

### 8.4 Cost Attribution por Branch

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchCost {
    pub branch: String,
    pub commit: String,
    pub sandbox_hours: f32,
    pub cpu_hours: f32,
    pub estimated_cost_usd: f32,
    pub period: DateRange,
}

#[server]
pub async fn get_branch_costs(
    project_id: ProjectId,
    branch: String,
    range: DateRange,
) -> Result<Vec<BranchCost>, ServerError> {
    // Each sandbox knows which git branch/commit it was created for
    // Aggregate cost by branch
}
```

### 8.5 Estado del Proyecto en `.bastion/`

El dashboard lee y escribe el estado del proyecto en `.bastion/`:

```json
// .bastion/dashboard.json (gitignored)
{
  "last_opened": "2026-05-12T10:30:00Z",
  "preferred_view": "sandboxes",
  "filters": {
    "status": ["running", "sleeping"],
    "purpose": null
  },
  "columns": ["id", "purpose", "template", "status", "cpu", "age"],
  "pinned_sandboxes": ["sb-bast-abc123"],
  "collapsed_sections": []
}
```

---

## 9. Seguridad — Project-Scoped

### 9.1 Modelo RBAC por Proyecto

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ProjectRole {
    /// Solo lectura: ver sandboxes, métricas, logs
    Viewer,

    /// Puede crear/destruir sandboxes, ejecutar pipelines
    Operator,

    /// Puede editar config, templates, policies
    Maintainer,

    /// Control total: managing users, policies, costs
    Admin,
}

#[derive(Debug, Clone)]
pub struct ProjectPermission {
    pub project_id: ProjectId,
    pub user_id: UserId,
    pub role: ProjectRole,
}
```

### 9.2 Aislamiento entre Proyectos

```
Proyecto A (.bastion/)          Proyecto B (.bastion/)
├── db/                         ├── db/
│   ├── sandboxes.db            │   ├── sandboxes.db        ← BBDD separada
│   ├── metrics.db              │   ├── metrics.db
│   └── enrichment.db           │   └── enrichment.db
├── providers/                  ├── providers/
│   └── podman.toml             │   └── podman.toml
├── pipelines/                  ├── pipelines/
│   └── ci.toml                 │   └── ci.toml
└── dashboard.json              └── dashboard.json

           │                                │
           ▼                                ▼
    Gateway (shared)                Gateway (shared)
    ├── Pool A (3 sandboxes)       ├── Pool B (2 sandboxes)
    └── MetricsHub A               └── MetricsHub B
```

Cada proyecto tiene:
- Su propia BBDD SQLite en `.bastion/db/`
- Sus propias configuraciones en `.bastion/providers/`, `capabilities/`
- Sus propios pipelines en `.bastion/pipelines/`
- Sus propias métricas en `.bastion/db/metrics.db`
- Su propia configuración de dashboard en `.bastion/dashboard.json`

El Gateway es compartido pero los datos están **aislados por proyecto**.

---

## 10. Stack Tecnológico Final (Actualizado)

| Componente | Tecnología | Versión | Rol |
|------------|------------|---------|-----|
| Frontend | Leptos | 0.7+ | UI reactiva WASM |
| CSS | Tailwind CSS | 4.x | Estilos con Oxide engine |
| CSS Engine | Lightning CSS | bundled | Build rápido |
| WASM Bundler | Trunk / cargo-leptos | 0.3+ | Build tool |
| Backend HTTP | Axum | 0.7+ | REST API + SSE |
| MCP | rmcp | 1.5+ | Protocolo MCP existente |
| Serialization | serde | 1.0 | JSON/TOML |
| Auth | jsonwebtoken | 9.x | JWT |
| DB | SQLite (rusqlite) | bundled | Per-project |
| Terminal | xterm.js | 5.x | Emulador terminal |
| Charts | uPlot | - | Gráficos rápidos |
| Icons | Lucide | - | Iconos |
| Git | git2 | 0.19+ | Integración Git |
| Project Config | toml | 0.8 | Config TOML |

---

## 11. Roadmap de Implementación (Actualizado)

### Fase 1: Project Core (2 semanas)

- [ ] `bastion-dashboard-core` crate con tipos Project, SandboxPurpose, PipelineDef
- [ ] `ProjectManager` con `open_project()`, `init_project()`
- [ ] `ProjectConfigLoader` que lee `.bastion/`
- [ ] Server function: `list_projects()`, `open_project()`
- [ ] UI: Project selector con Leptos + Tailwind 4.x
- [ ] Setup Tailwind 4.x con Oxide engine

### Fase 2: Sandbox Management (2 semanas)

- [ ] Sandbox list/detail scoped a proyecto
- [ ] Crear sandbox con purpose (adhoc, poc, e2e-test, real-test, pipeline)
- [ ] Asociar sandbox a git branch/commit
- [ ] Acciones: terminal, logs, sleep, wake, terminate
- [ ] SSE para actualizaciones en tiempo real

### Fase 3: Pipeline Visualization (1 semana)

- [ ] Parser de `.bastion/pipelines/*.toml`
- [ ] Ejecución de pipelines via MCP tools
- [ ] Visualización tipo CI/CD (stage graph)
- [ ] Cost attribution por pipeline

### Fase 4: Observabilidad por Proyecto (1 semana)

- [ ] MetricsHub project-scoped (`.bastion/db/metrics.db`)
- [ ] Dashboard overview con KPIs por proyecto
- [ ] Gráficos de métricas (uPlot)
- [ ] Cost summary por proyecto/branch

### Fase 5: Config Editor (1 semana)

- [ ] Editor visual de `.bastion/providers/*.toml`
- [ ] Toggle de advice/doctors/assertions
- [ ] Pipeline editor visual
- [ ] Template management

### Fase 6: Multi-Project + Seguridad (2 semanas)

- [ ] Autenticación JWT
- [ ] RBAC por proyecto
- [ ] Audit log por proyecto
- [ ] Multi-project switching
- [ ] Cost attribution multi-tenant

---

## 12. Conclusión

El insight clave es que **Bastion no gestiona infraestructura — gestiona proyectos de software**. Cada sandbox existe para un propósito dentro de un proyecto: PoC, testing, e2e, pipelines. El directorio `.bastion/` (como `.git/`) es el contracto entre el proyecto y la orquestación.

**Diferenciadores versus Portainer/Lens/Docker Desktop:**

| Plataforma | Unidad Fundamental | Bastion |
|------------|---------------------|---------|
| Portainer | Environment (Docker/K8s cluster) | **Proyecto** (git repo + `.bastion/`) |
| Docker Desktop | Local Docker Engine | **Proyecto** con BBDD propia |
| Lens | Kubernetes Cluster | **Proyecto** con pipelines y tests |
| Okteto | Namespace/Environment | **Proyecto** con cost attribution |

El dashboard project-centric permite:
1. **Cost attribution** natural por proyecto/branch/commit
2. **Pipelines declarativos** en `.bastion/pipelines/`
3. **Tests organizados** por propósito (adhoc, PoC, e2e, real)
4. **Git hooks** integrados con el flujo de desarrollo
5. **Auto-cleanup** por proyecto (sandbox sleeping >24h → sugerir terminate)
6. **Sandbox purposes** que dan contexto al por qué existe cada sandbox

---

*Documento creado como parte de la investigación para el Dashboard de Bastion*
*2026-05-12 — v2 Project-Centric*