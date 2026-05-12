# Bastion Dashboard — Propuesta de Arquitectura Web

> **Fecha**: 2026-05-12
> **Estado**: Investigación completada, propuesta inicial
> **Tipo**: Documento de arquitectura y diseño técnico

---

## Resumen Ejecutivo

Este documento presenta una propuesta para construir un **dashboard web completo** para Bastion, inspirado en las mejores prácticas de Docker Desktop, Portainer, Lens y Okteto, pero implementado con tecnologías Rust nativas: **Leptos** (frontend WASM) y **Tailwind CSS 4.x** (estilos).

El dashboard propuesto no es solo una interfaz de visualización — es un **centro de control completo** para la orquestación de sandboxes AI, con características de observabilidad, gestión de recursos, lifecycle automation y extensibilidad declarativa.

---

## 1. Investigación de Tecnologías de Referencia

### 1.1 Portainer

**Tipo**: Dashboard web multi-orquestador
**Tech Stack**: Go (backend) + AngularJS/React (frontend)

#### Arquitectura
- **Server**: Servicio central con API REST, autenticación, persistencia y proxy
- **Agent**: Agente ligero desplegado en nodos gestionados
- **Edge Agent**: Agente que invierte conectividad (outbound TLS hacia server)
- Persistencia centralizada; agentes stateless

#### Features Clave
- Gestión de Docker, Swarm, Kubernetes y Azure Container Instances
- UI para contenedores, imágenes, volúmenes, redes, stacks
- RBAC avanzado (Business Edition)
- Edge management para nodos remotos
- Soporte air-gapped

#### Patrones UI/UX
- Navegación por "environments" como unidad primaria
- Tablas densas con acciones por recurso
- Formularios guiados para despliegue
- Separación de roles: usuario estándar, operador, administrador

#### Comunicación API
- API HTTP central → reverse proxy hacia Docker/Kubernetes APIs
- Edge Agent usa túnel TLS outbound (evita abrir puertos)

#### Lección Clave
> El patrón **Edge Agent con conectividad invertida** es esencial para entornos restrictionados. El agente inicia conexión outbound hacia el server, eliminando necesidad de inbound ports.

---

### 1.2 Docker Desktop

**Tipo**: Aplicación desktop multiplataforma
**Tech Stack**: Electron + Go/React (componentes internos)

#### Arquitectura
- VM Linux aislada con Docker Engine
- Contexto Docker dedicado (`desktop-linux`)
- Socket per-user (`~/.docker/desktop/docker.sock`)
- Kubernetes local opcional (cluster interno)
- Extensions SDK para third-party add-ons

#### Features Clave
- Dashboard con navegación por dominios (containers, images, volumes, builds, Kubernetes)
- Resource controls: CPU, memoria, swap, disco, red
- Docker Extensions Marketplace
- Resource Saver: apaga VM cuando está idle
- Synchronized File Shares para monorepos
- Enhanced Container Isolation (ECI) con Sysbox

#### Patrones UI/UX
- Preferencias muy detalladas agrupadas por función
- Acciones rápidas desde listado: logs, terminal, debug, inspect
- Estados visibles en footer/tray
- Marketplace integrado

#### Seguridad (ECI)
- User namespaces para mapear root contenedor a usuarios no privilegiados
- Bloqueo de mounts sensibles (`/var/run/docker.sock`)
- Sysbox intercepta syscalls peligrosos

#### Lección Clave
> **Resource Saver** y **Enhanced Container Isolation** son features que diferencian un tool profesional de un demo. El aislamiento fuerte y la optimización de recursos son requisitos no funcionales críticos.

---

### 1.3 Lens / Mirantis Lens

**Tipo**: IDE Desktop para Kubernetes
**Tech Stack**: Electron + React + TypeScript

#### Arquitectura
- Monorepo con 150+ paquetes (Turborepo)
- UI local que lee `kubeconfig`
- Proxies internos hacia Kubernetes API
- Kubernetes Watch API para estado reactivo
- Lens Extension API para extensibilidad

#### Features Clave
- Navigator: árbol de clusters y workspaces
- Hotbar: acceso rápido a clusters favoritos
- Command Palette: navegación tipo IDE
- Tab Bar: múltiples vistas por cluster
- Dock: terminal, logs, editor de templates
- Terminal integrada con `node-pty`
- Multi-cluster management
- Lens Teamwork con RBAC por espacios

#### Patrones UI/UX (los más sofisticados)
- **IDE-like experience**: Lens se comporta como IDE, no como dashboard administrativo
- Drill Into: foco en sección/cluster
- Contextual Tab Filtering: filtra pestañas según contexto
- Preview vs Fixed tab modes
- Status bar con estado y soporte

#### Comunicación API
- Lectura de `kubeconfig` local
- Watch por recurso y namespace seleccionado
- Un único watch por recurso (no duplicados)
- Probado con 20k namespaces, 50k pods

#### Lección Clave
> El patrón **IDE-like** con Navigator + Tabs + Dock transforma la gestión de infraestructura de "administrative task" a "development workflow". La experiencia de usuario debe sentirse como usar VS Code, no phpMyAdmin.

---

### 1.4 Okteto

**Tipo**: Plataforma de desarrollo cloud-native
**Tech Stack**: Go (CLI) + React (UI) + Kubernetes (runtime)

#### Arquitectura
- **Open Source**: CLI Go que habla con Kubernetes API
- **Platform**: Helm chart con API, Frontend, BuildKit, Registry, Webhook
- NGINX Ingress para routing y auto-wake
- Validation Webhook para enforcement

#### Features Clave
- Development Containers en Kubernetes
- Code sync instantáneo (evita rebuild/redeploy)
- Preview environments por PR
- Okteto Test (unit/integration/e2e)
- Resource Manager: calcula requests recomendados
- Garbage Collector: escala a cero namespaces inactivos
- Auto-wake mediante ingress
- Secrets manager
- Okteto Insights (métricas)

#### Lifecycle Automation
```yaml
# Okteto Manifest - ejemplo
dev:
  myapp:
    image: okteto/golang:1.21
    command: sleep infinity
    sync:
      - .:/app
    forward:
      - 8080:8080
    autovars:
      - API_URL
```

#### Patrones UI/UX
- Dashboard de administración (resources, namespaces, previews, GC)
- Enfoque "one-click environment"
- CLI + Dashboard como experiencia dual
- Manifiesto declarativo como configuración

#### Lección Clave
> **Resource Manager + Garbage Collector + Auto-wake** forman un trío de automatización que reduce costos drásticamente. No basta con mostrar métricas — hay que actuar automáticamente sobre ellas.

---

### 1.5 Tabla Comparativa

| Plataforma | Arquitectura | UI/UX | API | Extensibilidad | Seguridad |
|------------|--------------|--------|-----|----------------|-----------|
| **Portainer** | Server + Agents | Admin console | REST + proxy | Stacks, templates | Auth/RBAC, TLS |
| **Docker Desktop** | VM + Engine + Extensions | Developer dashboard | CLI/contextos | Extensions SDK | VM isolation, ECI |
| **Lens** | Electron + Kubeconfig | IDE-like | Watch API | Extension API | K8s RBAC, SSO |
| **Okteto** | CLI + Platform Helm | Dev platform | REST + K8s API | Manifests, Helm | IdP, secrets |

---

## 2. Tecnologías Propuestas

### 2.1 Leptos 0.7+ (Frontend WASM)

**Por qué Leptos**:
- 100% Rust — mismo lenguaje que el backend
- WASM compilado — bundle pequeño, rendimiento nativo
- Signals reactivos — modelo mental simple y predecible
- Server Functions — llamada a Rust server-side sin ceremony
- Islands Architecture — interactividad solo donde se necesita
- SSR + CSR + MPA support
- Integración natural con Tailwind CSS

**Características 0.7+**:
```rust
// Component con signals reactivos
#[component]
fn SandboxList() -> impl IntoView {
    let (filter, set_filter) = signal("".to_string());
    let sandboxes = create_resource(filter, fetch_sandboxes);

    view! {
        <input
            class="px-4 py-2 border rounded-lg"
            placeholder="Filter sandboxes..."
            on:input={move |e| set_filter.set(event_target_value(&e))}
        />

        <Transition>
            <Match bound={sandboxes}>
                <Pending>"Loading..."</Pending>
                <Resolved(sandbox_list)>
                    <For each={sandbox_list}>
                        {|sandbox| <SandboxCard sandbox={sandbox} />}
                    </For>
                </Resolved>
                <Errored(e)>"Error: {e}"</Errored>
            </Match>
        </Transition>
    }
}
```

**Server Functions**:
```rust
#[server]
async fn fetch_sandbox_metrics(id: SandboxId) -> Result<Metrics, ServerError> {
    let hub = metrics_hub.read().await;
    hub.get_metrics(id).await.map_err(|e| ServerError::from(e))
}
```

**Integración con Tailwind 4.x**:
```rust
// Tailwind funciona directamente en las clases
view! {
    <div class="bg-slate-900 text-white rounded-xl p-6 shadow-2xl">
        <h1 class="text-2xl font-bold text-primary-400">"Bastion Dashboard"</h1>
    </div>
}
```

### 2.2 Tailwind CSS 4.x (Estilos)

**Novedades en 4.x**:

| Aspecto | v3.x | v4.x |
|---------|------|------|
| Configuración | `tailwind.config.js` | CSS-first (`@theme`) |
| Engine | PostCSS + JavaScript | **Rust (Oxide Engine)** |
| Parser | PostCSS | **Rust custom** |
| Velocidad build | Medium | **2x+ más rápido** |
| Dependencias | Muchas (PostCSS, autoprefixer, etc.) | **Solo Lightning CSS** |
| Tamaño output | Medium | **35% menor** |

**CSS-first config**:
```css
/* main.css */
@import "tailwindcss";

@theme {
    --color-primary: oklch(70% 0.15 250);
    --color-secondary: oklch(60% 0.12 180);
    --color-surface: oklch(15% 0.03 280);
    --color-muted: oklch(50% 0.05 280);
    --radius-lg: 1rem;
    --radius-xl: 1.5rem;

    --font-sans: "Inter", system-ui, sans-serif;
    --font-mono: "JetBrains Mono", monospace;
}

body {
    background: var(--color-surface);
    color: white;
    font-family: var(--font-sans);
}
```

**Dark mode mejorado**:
```css
@custom-variant dark (&:where(.dark, .dark *));

.button {
    background: oklch(60% 0.15 250);
    @dark {
        background: oklch(70% 0.12 250);
    }
}
```

**Componentes con @apply**:
```css
@layer components {
    .card {
        @apply bg-white dark:bg-slate-800 rounded-xl p-6 shadow-lg;
        @apply border border-slate-200 dark:border-slate-700;
    }

    .btn-primary {
        @apply bg-primary-600 hover:bg-primary-700 text-white;
        @apply px-4 py-2 rounded-lg font-medium transition-colors;
    }

    .stat-card {
        @apply card flex flex-col gap-2;
        & .value { @apply text-3xl font-bold text-primary-400; }
        & .label { @apply text-sm text-muted; }
    }
}
```

**Performance en Leptos**:
- Trunk (build tool) compila WASM + CSS en paralelo
- Lightning CSS (Rust) parsea CSS 2x más rápido que PostCSS
- Hot reload para desarrollo
- Tree-shaking automático de clases no usadas

---

## 3. Arquitectura Propuesta para Bastion Dashboard

### 3.1 Arquitectura General

```
┌─────────────────────────────────────────────────────────────────┐
│                     Bastion Dashboard (Leptos WASM)              │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────────┐│
│  │ Navigator   │  │   Workspace  │  │       Dock             ││
│  │ (sidebar)   │  │   (main)     │  │  (terminal/logs/details)││
│  └─────────────┘  └──────────────┘  └─────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ HTTP/SSE/WebSocket
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Bastion Gateway (Axum)                     │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────────────┐│
│  │ MCP Tools    │  │ REST API     │  │  SSE Event Stream       ││
│  │ (existing)   │  │ /api/v1     │  │  /api/v1/events        ││
│  └──────────────┘  └──────────────┘  └─────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
                              │
         ┌────────────────────┼────────────────────┐
         ▼                    ▼                    ▼
┌─────────────────┐  ┌───────────────┐  ┌─────────────────┐
│  Podman Worker  │  │ Firecracker   │  │  MetricsHub     │
│  (sandbox)     │  │ (sandbox)    │  │  (SQLite)       │
└─────────────────┘  └───────────────┘  └─────────────────┘
```

### 3.2 Arquitectura de Componentes

```
bastion-dashboard/
├── crate/
│   ├── bastion-dashboard-core/      # Lógica de UI, components
│   │   ├── components/
│   │   │   ├── layout/             # Navigator, Dock, Workspace
│   │   │   ├── sandbox/            # SandboxCard, SandboxList, SandboxDetail
│   │   │   ├── metrics/            # Charts, Gauges, Stats
│   │   │   ├── terminal/           # XTerm integration
│   │   │   └── common/             # Button, Card, Modal, Table
│   │   ├── pages/
│   │   │   ├── dashboard.rs        # Overview page
│   │   │   ├── sandboxes.rs        # Sandbox list/detail
│   │   │   ├── pools.rs            # Pool management
│   │   │   ├── templates.rs        # Template catalog
│   │   │   ├── settings.rs         # User/Admin settings
│   │   │   └── docs.rs             # Documentation
│   │   ├── state/                  # Global state, signals
│   │   ├── api/                    # Server function clients
│   │   └── i18n/                   # Internationalization
│   │
│   ├── bastion-dashboard-server/   # Axum server, SSR, API
│   │   ├── handlers/
│   │   │   ├── api.rs              # REST API v1
│   │   │   ├── sse.rs              # Server-Sent Events
│   │   │   └── ws.rs               # WebSocket (optional)
│   │   ├── auth/                   # JWT, sessions
│   │   ├── middleware/              # Logging, cors, rate limit
│   │   └── static/                 # WASM + assets
│   │
│   └── bastion-dashboard-cli/       # CLI companion
│       └── main.rs
│
├── frontend/                        # Build output (WASM + CSS)
├── tailwind.config.js              # Tailwind 4.x config
└── package.json
```

### 3.3 Modelo de Datos

```rust
// Entidades core del dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardState {
    pub sandboxes: Vec<SandboxSummary>,
    pub pools: Vec<PoolStatus>,
    pub metrics: DashboardMetrics,
    pub templates: Vec<TemplateInfo>,
    pub user: UserInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSummary {
    pub id: SandboxId,
    pub name: String,
    pub status: SandboxStatus,
    pub runtime: RuntimeType,
    pub template: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cpu_usage: f32,
    pub memory_usage: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxStatus {
    Creating,
    Starting,
    Ready,
    Sleeping,
    Stopping,
    Failed { error: String },
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStatus {
    pub name: String,
    pub runtime: RuntimeType,
    pub active: usize,
    pub idle: usize,
    pub total: usize,
    pub min_idle: usize,
    pub max_idle: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardMetrics {
    pub sandboxes_created_total: u64,
    pub sandboxes_active: u64,
    pub commands_executed_total: u64,
    pub error_rate: f32,
    pub avg_command_duration_ms: f64,
}
```

---

## 4. Patrones UI/UX Propuestos

### 4.1 Layout Principal (Patrón Lens)

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▸ Bastion    │ Dashboard │ Sandboxes │ Pools │ Templates │ Settings │
│              │                              [Search...] [User ▾]   │
├─────────────┬────────────────────────────────────────────────────────┤
│ NAVIGATOR   │ WORKSPACE                                              │
│             │                                                        │
│ ▼ Clusters  │ ┌──────────────────────────────────────────────────┐  │
│   └ prod    │ │ Metric Cards Row                                   │  │
│   └ staging │ │ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ │  │
│             │ │ │ Active  │ │ Created │ │ Errors  │ │ Latency │ │  │
│ ▼ Sandboxes │ │ │   12    │ │  1,234  │ │   0.2%  │ │  145ms  │ │  │
│   └ running │ │ └─────────┘ └─────────┘ └─────────┘ └─────────┘ │  │
│   └ stopped │ └──────────────────────────────────────────────────┘  │
│             │                                                        │
│ ▼ Templates │ ┌──────────────────────────────────────────────────┐  │
│   └ debian  │ │ Sandbox List / Grid                               │  │
│   └ rust    │ │ ┌──────────────────────────────────────────────┐ │  │
│             │ │ │ 🔴 sandbox-abc123  │ Running │ 2h │ CPU: 5%   │ │  │
│ ▼ History   │ │ │ 🟡 sandbox-def456  │ Idle    │ 30m│ CPU: 1%   │ │  │
│             │ │ │ 🟢 sandbox-ghi789  │ Ready   │ 5m │ CPU: 12%  │ │  │
│             │ │ └──────────────────────────────────────────────┘ │  │
│             │ └──────────────────────────────────────────────────┘  │
├─────────────┴────────────────────────────────────────────────────────┤
│ DOCK (collapsible)                                                  │
│ ┌──────────────────────────────────────────────────────────────────┐│
│ │ [Terminal] [Logs] [Details] [Metrics]                            ││
│ │ ┌──────────────────────────────────────────────────────────────┐ ││
│ │ │ $ bastion run --sandbox sandbox-abc123                      │ ││
│ │ │ > Compiling sandbox-core v0.1.0...                           │ ││
│ │ │ > Finished dev [unoptimized] in 12.3s                       │ ││
│ │ └──────────────────────────────────────────────────────────────┘ ││
│ └──────────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────┘
```

### 4.2 Componentes Clave

#### MetricCard
```rust
#[component]
pub fn MetricCard(
    title: &'static str,
    value: impl IntoView,
    trend: Option<f32>,
    icon: &'static str,
) -> impl IntoView {
    view! {
        <div class="stat-card">
            <div class="flex items-center justify-between">
                <span class="text-muted text-sm">{title}</span>
                <span class="text-2xl">{icon}</span>
            </div>
            <div class="value">{value}</div>
            {match trend {
                Some(t) if t > 0.0 => view! { <span class="text-green-400">+"{}%"</span> },
                Some(t) if t < 0.0 => view! { <span class="text-red-400">"{}%"</span> },
                _ => view! { <span class="text-muted">"-"</span> },
            }}
        </div>
    }
}
```

#### SandboxCard
```rust
#[component]
pub fn SandboxCard(sandbox: SandboxSummary) -> impl IntoView {
    let status_color = match sandbox.status {
        SandboxStatus::Ready => "bg-green-500",
        SandboxStatus::Running => "bg-green-500",
        SandboxStatus::Sleeping => "bg-yellow-500",
        SandboxStatus::Creating | SandboxStatus::Starting => "bg-blue-500 animate-pulse",
        SandboxStatus::Failed { .. } | SandboxStatus::Stopping => "bg-red-500",
        SandboxStatus::Terminated => "bg-slate-500",
    };

    view! {
        <div class="card hover:border-primary-500 transition-colors cursor-pointer">
            <div class="flex items-center gap-3">
                <div class=format!("w-3 h-3 rounded-full {}", status_color)></div>
                <div class="flex-1">
                    <div class="font-mono text-sm">{sandbox.id.to_string()}</div>
                    <div class="text-muted text-xs">{sandbox.template}</div>
                </div>
                <div class="text-right">
                    <div class="text-sm">{format_duration(sandbox.created_at)}</div>
                    <div class="text-xs text-muted">"CPU: {}%"</div>
                </div>
            </div>

            <div class="mt-4 flex gap-2">
                <button class="btn-primary flex-1">"Terminal"</button>
                <button class="btn-secondary flex-1">"Logs"</button>
                <button class="btn-secondary">"⋮"</button>
            </div>
        </div>
    }
}
```

#### Terminal Component (xterm.js integration)
```rust
#[component]
pub fn Terminal(sandbox_id: SandboxId) -> impl IntoView {
    let terminal_ref = NodeRef::<web_sys::HtmlElement>::default();

   Effect::once(move || {
        if let Some(el) = terminal_ref.get() {
            let term = xterm::Terminal::new();
            term.open(el);
            // Connect to backend stream
            spawn_local(async move {
                let stream = connect_terminal(sandbox_id).await;
                stream.for_each(|chunk| {
                    term.write(chunk);
                }).await;
            });
        }
    });

    view! {
        <div ref={terminal_ref} class="h-full w-full bg-black text-white"></div>
    }
}
```

### 4.3 Estados y Animaciones

```css
/* Loading state */
.skeleton {
    @apply animate-pulse bg-slate-700 rounded;
}

/* Status transitions */
.status-badge {
    @apply px-2 py-1 rounded-full text-xs font-medium transition-colors;
    &.creating { @apply bg-blue-500/20 text-blue-400; }
    &.ready { @apply bg-green-500/20 text-green-400; }
    &.sleeping { @apply bg-yellow-500/20 text-yellow-400; }
    &.failed { @apply bg-red-500/20 text-red-400; }
}

/* Hover effects */
.hover-lift {
    @apply transition-transform hover:-translate-y-0.5;
}

/* Focus states */
.focus-ring {
    @apply focus:outline-none focus:ring-2 focus:ring-primary-500 focus:ring-offset-2 focus:ring-offset-slate-900;
}
```

---

## 5. Modelo de Extensibilidad

### 5.1 Extensiones Declarativas

Inspirado en Okteto manifests y Docker Extensions:

```yaml
# bastion-extension.yaml
name: bastion-prometheus
version: 1.0.0
description: Prometheus metrics integration

runtime: podman

extension:
  views:
    - path: /metrics
      component: PrometheusDashboard
      icon: chart-bar

  hooks:
    - event: sandbox_created
      action: notify_prometheus

  resources:
    memory: 128Mi
    cpu: 0.25

dependencies:
  - prometheus-operator
```

### 5.2 Plugin System

```rust
// Plugin trait
#[async_trait]
pub trait DashboardPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> semver::Version;

    // Lifecycle
    async fn on_load(&self, registry: &mut PluginRegistry) -> Result<()>;
    async fn on_unload(&self) -> Result<()>;

    // Extension points
    fn views(&self) -> Vec<ViewRegistration>;
    fn actions(&self) -> Vec<ActionRegistration>;
    fn middleware(&self) -> Vec<MiddlewareFn>;
}

// Registration
impl PluginRegistry {
    pub fn register<P: DashboardPlugin + 'static>(&mut self, plugin: P) {
        let name = plugin.name().to_string();
        self.plugins.insert(name, Box::new(plugin));
    }
}
```

### 5.3 Templates como extensibilidad

```yaml
# templates/nginx-template.yaml
name: nginx-development
description: Nginx for web development
image: nginx:alpine

resources:
  cpu: "0.5"
  memory: "256Mi"
 ephemeral_storage: "1Gi"

network:
  ports:
    - 80:8080
    - 443:8443

files:
  - path: /etc/nginx/nginx.conf
    content: |
      worker_processes 1;
      events { worker_connections 1024; }

startup:
  command: ["/docker-entrypoint.sh", "nginx", "-g", "daemon off;"]

policies:
  max_lifetime: 24h
  auto_sleep: 30m
  allow_privileged: false
```

---

## 6. Pensamiento Lateral — Diferenciadores

### 6.1 AI-Native Dashboard

**Innovación**: El dashboard que administra sandboxes PARA AGENTES AI, podría ser usado POR AGENTES AI.

```rust
// MCP tool que el dashboard expone
#[server]
pub async fn dashboard_query(query: String) -> Result<DashboardQueryResponse, ServerError> {
    // El dashboard tiene acceso completo al estado
    // Un agente AI podría consultar: "¿qué sandboxes están inactivos hace más de 1 hora?"
    // Y pedir recomendaciones de cleanup

    let state = dashboard_state.read().await;
    let recommendations = generate_recommendations(&state, &query).await;

    Ok(DashboardQueryResponse {
        answer: recommendations.narrative,
        affected_sandboxes: recommendations.ids,
        suggested_actions: recommendations.actions,
        confidence: recommendations.confidence,
    })
}
```

**UI**: Los agentes podrían usar el dashboard via MCP, no solo humanos.

### 6.2 Cost Attribution Multi-Tenant

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAttribution {
    pub project_id: ProjectId,
    pub user_id: UserId,
    pub sandbox_hours: f32,
    pub compute_cost_usd: f32,
    pub storage_cost_usd: f32,
    pub egress_cost_usd: f32,
    pub total_cost_usd: f32,
}

#[server]
pub async fn get_project_costs(
    project_id: ProjectId,
    range: DateRange,
) -> Result<Vec<CostAttribution>, ServerError> {
    // Aggregación por proyecto, usuario, sandbox
    // Integración futura con cloud billing APIs
}
```

### 6.3 GitOps Integration

```yaml
# bastion-gitops.yaml en repositorio
apiVersion: bastion.rs/v1
kind: SandboxPolicy
metadata:
  name: production-policy
spec:
  match:
    branch: main
    paths:
      - "services/**"
  sandbox:
    template: rust-production
    resources:
      cpu: "4"
      memory: "8Gi"
    policies:
      max_lifetime: 2h
      auto_sleep: 15m
      require_approval: true
```

El dashboard lee políticas GitOps y aplica automáticamente.

### 6.4 Sandbox Pipelines Visual

Inspirado en CI/CD pipelines, pero para ejecución distribuida:

```rust
// Representación visual de un pipeline
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub id: StageId,
    pub name: String,
    pub sandbox_id: Option<SandboxId>,
    pub status: StageStatus,
    pub parallel_tasks: Vec<Task>,
    pub depends_on: Vec<StageId>,
}

#[derive(Debug, Clone)]
pub struct PipelineExecution {
    pub id: ExecutionId,
    pub stages: Vec<PipelineStage>,
    pub total_duration_ms: u64,
    pub created_at: DateTime<Utc>,
}
```

**UI**: Visualización tipo pipeline de GitHub Actions, pero ejecutando en sandboxes distribuidos.

### 6.5 Collaborative Debugging

```rust
// Sesión compartida de debugging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    pub id: SessionId,
    pub sandbox_id: SandboxId,
    pub participants: Vec<UserId>,
    pub cursor_positions: HashMap<UserId, CursorPosition>,
    pub terminal_input: Vec<SharedTerminalEvent>,
    pub created_at: DateTime<Utc>,
}

#[server]
pub async fn create_debug_session(
    sandbox_id: SandboxId,
) -> Result<DebugSession, ServerError> {
    // Crea sesión colaborativa
    // Participantes ven terminal compartido
    // Cursor colors identifican cada usuario
}
```

---

## 7. API y Comunicación

### 7.1 REST API v1

```
GET    /api/v1/sandboxes                    # List sandboxes
GET    /api/v1/sandboxes/:id               # Get sandbox detail
POST   /api/v1/sandboxes                   # Create sandbox
DELETE /api/v1/sandboxes/:id               # Terminate sandbox
POST   /api/v1/sandboxes/:id/sleep         # Sleep sandbox
POST   /api/v1/sandboxes/:id/wake          # Wake sandbox

GET    /api/v1/pools                        # List pools
GET    /api/v1/pools/:name/stats            # Pool statistics

GET    /api/v1/metrics                     # Dashboard metrics
GET    /api/v1/metrics/historical           # Historical data

GET    /api/v1/templates                    # List templates
POST   /api/v1/templates                    # Register template

GET    /api/v1/users/me                    # Current user
GET    /api/v1/users/:id/costs             # User cost attribution

WS     /api/v1/events                      # Real-time events
SSE    /api/v1/stream/:resource            # Resource streaming
```

### 7.2 Server-Sent Events

```rust
// Server: emisión de eventos
async fn sse_handler(
    axum::extract::Path(resource): Path<String>,
    axum::extract::State(state): State<Arc<AppState>>,
    stream: Sse,
) -> impl axum::response::IntoResponse {
    let event_stream = state.subscribe(resource).map(|event| {
        Event::default()
            .event(event.kind.as_str())
            .data(serde_json::to_string(&event.payload).unwrap())
    });

    stream::channel(32, event_stream)
        .map(Ok::<_, Infallible>)
}

// Client: suscripción en Leptos
#[server]
pub async fn subscribe_to_events(
    resource: String,
) -> Result<SseConsumer, ServerError> {
    let url = format!("/api/v1/stream/{}", resource);
    Ok(SseConsumer::new(&url))
}
```

### 7.3 Tipos de Eventos

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DashboardEvent {
    SandboxCreated { id: SandboxId },
    SandboxReady { id: SandboxId },
    SandboxSleeping { id: SandboxId },
    SandboxTerminated { id: SandboxId },
    SandboxMetricsUpdated { id: SandboxId, cpu: f32, memory: f32 },
    PoolScaleUp { name: String, new_size: usize },
    PoolScaleDown { name: String, new_size: usize },
    CommandCompleted { sandbox_id: SandboxId, command_id: CommandId, exit_code: i32 },
    Error { component: String, message: String },
}
```

---

## 8. Seguridad

### 8.1 Modelo de Seguridad en Capas

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: Network                                            │
│ - mTLS entre agentes y gateway                              │
│ - TLS 1.3 para UI                                          │
│ - WebSocket secure (wss://)                                 │
│ - Firewall rules para flujos específicos                     │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Layer 2: Authentication                                       │
│ - JWT para API y dashboard                                   │
│ - API keys para CI/automation                               │
│ - OAuth2/OIDC para SSO (futuro)                             │
│ - Session management con refresh tokens                      │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Layer 3: Authorization (RBAC)                               │
│ - Roles: viewer, operator, admin                            │
│ - viewer: solo lectura                                      │
│ - operator: puede crear/destruir sandboxes propios           │
│ - admin: acceso total, gestión de usuarios, políticas        │
│ - Attribute-based: por proyecto, namespace, sandbox type    │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Layer 4: Sandbox Isolation                                   │
│ - gVisor/Firecracker como runtime options                   │
│ - No privileged containers                                  │
│ - Read-only rootfs por defecto                              │
│ - Seccomp profiles restrictivos                             │
│ - Capabilities mínimas (CAP_NET_RAW, etc.)                   │
│ - Network policies (egress allowlist)                       │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Layer 5: Audit & Compliance                                 │
│ - Every action logged with timestamp, user, action, result  │
│ - Audit log inmutable                                       │
│ - Integration con SIEM (Splunk, Elastic)                    │
│ - Retention policies configurables                           │
└─────────────────────────────────────────────────────────────┘
```

### 8.2 RBAC Implementation

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

#[derive(Debug, Clone)]
pub struct Permission {
    pub action: Action,
    pub resource_scope: ResourceScope,
}

pub enum Action {
    SandboxCreate,
    SandboxRead,
    SandboxUpdate,
    SandboxDelete,
    SandboxExecute,
    PoolManage,
    TemplateManage,
    UserManage,
    PolicyManage,
}

#[derive(Debug, Clone)]
pub enum ResourceScope {
    All,
    Project(ProjectId),
    Sandbox(SandboxId),
    Own,  // Only own resources
}

#[async_trait]
impl Authorization for AppState {
    async fn authorize(
        &self,
        user: &User,
        action: Action,
        resource: Resource,
    ) -> Result<bool, AuthError> {
        let role = self.get_user_role(user).await?;
        let permissions = ROLE_PERMISSIONS.get(&role)?;

        Ok(permissions.iter().any(|p| {
            p.action == action && p.resource_scope.matches(&resource)
        }))
    }
}
```

---

## 9. Rendimiento y Optimización

### 9.1 Patrones de Rendimiento

**Lazy Loading de Vistas**:
```rust
#[component]
pub fn DashboardPage() -> impl IntoView {
    // Solo carga HeavyChart cuando el usuario hace scroll
    let show_charts = create_signal(false);

    view! {
        <div>
            <MetricCards />

            <Show when={show_charts.get()}>
                <HeavyCharts lazy_loaded=true />
            </Show>

            <button on:click=|_| show_charts.set(true)>
                "Load Charts"
            </button>
        </div>
    }
}
```

**Virtual Scrolling para Listas Grandes**:
```rust
#[component]
pub fn VirtualSandboxList(
    sandboxes: Vec<SandboxSummary>,
) -> impl IntoView {
    // Virtualize huge lists (10k+ items)
    view! {
        <VirtualList
            items={sandboxes}
            item_height={72}
            overscan={5}
            renderer=|sandbox| view! { <SandboxCard {sandbox} /> }
        />
    }
}
```

**Memoización de Queries**:
```rust
#[component]
pub fn SandboxMetrics(id: SandboxId) -> impl IntoView {
    // create_resource memoriza el resultado
    // No re-fetch si id no cambia
    let metrics = create_resource(
        move || id,
        |id| async move { fetch_metrics(id).await },
    );

    view! {
        <Suspense fallback={() => view! { <Skeleton /> }>
            <ErrorBoundary fallback=|e| view! { <Error error={e} /> }>
                <MetricsChart data={metrics} />
            </ErrorBoundary>
        </Suspense>
    }
}
```

### 9.2 Caching Strategy

```
┌─────────────────────────────────────────────────────────────┐
│ Client Cache (Leptos State)                                 │
│ - Signals con valores recent                                │
│ - invalidar en eventos SSE                                 │
│ - TTL configurable por tipo de dato                        │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Edge Cache (futuro: Redis)                                  │
│ - /api/v1/templates (1h TTL)                                │
│ - /api/v1/metrics/historical (5m TTL)                       │
│ - /api/v1/pools/stats (10s TTL)                             │
└─────────────────────────────────────────────────────────────┘
                           │
┌─────────────────────────────────────────────────────────────┐
│ Database (SQLite -> PostgreSQL)                            │
│ - MetricsHub SQLite actual                                  │
│ - Sharding por tenant/project (futuro)                     │
└─────────────────────────────────────────────────────────────┘
```

---

## 10. Roadmap de Implementación

### Fase 1: Dashboard Core (2 semanas)

- [ ] Setup proyecto Leptos + Tailwind 4.x
- [ ] Layout base: Navigator + Workspace + Dock
- [ ] Sandbox list con estado y acciones básicas
- [ ] Conexión a gateway via REST API
- [ ] SSE para actualizaciones en tiempo real
- [ ] Metric cards estáticos

### Fase 2: Interactividad (2 semanas)

- [ ] Crear/terminar sandboxes desde UI
- [ ] Terminal integrado (xterm.js)
- [ ] Vista de logs
- [ ] Sandbox detail con métricas
- [ ] Pool management UI

### Fase 3: Observabilidad (1 semana)

- [ ] Gráficos de métricas (Chart.js o similar)
- [ ] Historial de comandos
- [ ] Dashboard overview con KPIs
- [ ] Filtros y búsqueda

### Fase 4: Templates y Policies (1 semana)

- [ ] Catálogo de templates
- [ ] Editor visual de policies
- [ ] Import/export de configuraciones

### Fase 5: Multi-tenant y Seguridad (2 semanas)

- [ ] Sistema de autenticación
- [ ] RBAC
- [ ] Audit log
- [ ] Cost attribution

### Fase 6: Extensibilidad (2 semanas)

- [ ] Plugin system
- [ ] Extension registry
- [ ] Custom dashboards

---

## 11. Stack Tecnológico Final

| Componente | Tecnología | Versión |
|------------|------------|---------|
| Frontend Framework | Leptos | 0.7+ |
| WASM Bundler | Trunk / cargo-leptos | 0.3+ |
| CSS Framework | Tailwind CSS | 4.x |
| CSS Engine | Lightning CSS (Rust) | bundled |
| Backend HTTP | Axum | 0.7+ |
| Serialization | serde | 1.0 |
| Auth | jsonwebtoken | 9.x |
| Database (metrics) | SQLite (MetricsHub) | - |
| Terminal | xterm.js | 5.x |
| Charts | Chart.js o uPlot | - |
| Icons | Lucide | - |
| Build Tool | Cargo + Trunk | - |

---

## 12. Conclusiones y Próximos Pasos

### Conclusiones

1. **Leptos + Tailwind 4.x** es la combinación tecnológica correcta:
   - 100% Rust end-to-end
   - WASM para rendimiento nativo
   - Tailwind 4.x con engine Rust (Oxide) para builds rápidas
   - Signals reactivos para UX fluida

2. **Patrones probados** de las 4 plataformas de referencia:
   - Lens: IDE-like UX con Navigator + Tabs + Dock
   - Portainer: Edge agents con conectividad invertida
   - Docker Desktop: Resource optimization + isolation
   - Okteto: Lifecycle automation + cost management

3. **Diferenciadores propuesta**:
   - AI-native: dashboard expose MCP tools para agentes
   - Cost attribution multi-tenant
   - GitOps integration
   - Pipeline visualization
   - Collaborative debugging

### Próximos Pasos

1. **Validar propuesta** con stakeholders
2. **Crear SDD** (Spec-Driven Development) para el dashboard
3. **Prototipo rápido** con Leptos + Tailwind 4.x
4. **Iterar** basándose en feedback

---

## Referencias

- Portainer Architecture: https://docs.portainer.io/start/architecture
- Lens IDE: https://github.com/lensapp/lens
- Okteto Platform: https://www.okteto.com/docs/
- Docker Desktop: https://docs.docker.com/desktop/
- Leptos Framework: https://leptos.dev/
- Tailwind CSS 4.0: https://tailwindcss.com/blog/tailwindcss-v4-alpha
- Oxide Engine: https://tailwindcss.com/blog/tailwindcss-v4-alpha#a-rust-powered-engine

---

*Documento creado como parte de la investigación para el Dashboard de Bastion*
*2026-05-12*
