//! Bastion MCP Gateway
//!
//! Entry point for the sandbox gateway MCP server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use bastion_domain::execution::command::{CommandResult, CommandSpec};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::capabilities::ProviderCapabilities;
use bastion_domain::provider::executor::{CommandStream, TaskExecutor};
use bastion_domain::provider::lifecycle::SandboxLifecycle;
use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::provider::router::CommandRouter;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::sandbox::snapshot::SnapshotInfo;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxFilter};
use bastion_domain::secret::SecretResolver;
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;
use bastion_infrastructure::catalog::sqlite_experience_store::SqliteExperienceStore;
use bastion_infrastructure::catalog::toml_advice_parser::{AdviceConfigStore, AdviceRegistry};
use bastion_infrastructure::catalog::toml_assertion_parser::AssertionRegistry;
use bastion_infrastructure::catalog::toml_doctor_parser::DoctorRegistry;
use bastion_infrastructure::metrics::{GatewayMetrics, MetricsHub};
use bastion_infrastructure::persistence::SqliteSandboxRepository;
use bastion_infrastructure::pool::{PoolConfig, SandboxPoolManager};
use bastion_infrastructure::provider::{PodmanProvider, ProviderFactory, ProviderRegistry};
use bastion_infrastructure::secret::EnvSecretResolver;
use bastion_infrastructure::template::CapabilityRegistry;
use enrichment_engine::traits::RunRecorder;

use rmcp::{ServiceExt, service::RoleServer};

// HTTP transport imports
use hyper::server::conn::http1;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

mod advice_tools;
mod auth;
mod auto_tls;
mod catalog_tools;
mod doctor_tools;
mod enrichment_tools;
mod metrics_tools;
mod metrics_tools_types;
mod orientation_tools;
mod orientation_tools_types;
mod pipeline_events;
mod registry;
mod sandbox;
mod server;

use registry::{RegistryService, WorkerRegistryServer};
use tonic::codec::CompressionEncoding;

/// CLI project kind enum that maps to bastion_domain::project::ProjectKind.
#[derive(clap::ValueEnum, Clone, Debug)]
enum CliProjectKind {
    Rust,
    Nodejs,
    Python,
    Go,
    Generic,
}

impl From<CliProjectKind> for bastion_domain::project::ProjectKind {
    fn from(kind: CliProjectKind) -> Self {
        match kind {
            CliProjectKind::Rust => bastion_domain::project::ProjectKind::Rust,
            CliProjectKind::Nodejs => bastion_domain::project::ProjectKind::NodeJs,
            CliProjectKind::Python => bastion_domain::project::ProjectKind::Python,
            CliProjectKind::Go => bastion_domain::project::ProjectKind::Go,
            CliProjectKind::Generic => bastion_domain::project::ProjectKind::Generic,
        }
    }
}

/// CLI commands for the bastion gateway.
#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Initialize a new Bastion project in the current directory.
    Init {
        /// Project kind (determines default templates and pipelines).
        #[arg(long, value_enum, default_value = "generic")]
        kind: CliProjectKind,
        /// Project name (defaults to directory name).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Parser, Debug)]
#[command(name = "bastion-gateway", version, about = "Bastion MCP Gateway")]
struct Args {
    /// Path to Podman socket
    #[arg(long, default_value = "/run/user/1000/podman/podman.sock")]
    socket: String,

    /// Default image to use for sandboxes
    #[arg(long, default_value = "debian:bookworm-slim")]
    image: String,

    /// Path to bastion-worker binary (for container injection)
    #[arg(long, default_value = "target/debug/bastion-worker")]
    worker_binary: String,

    /// Path to configuration file
    #[arg(short, long, default_value = "config/sandbox-gateway.toml")]
    config: String,

    /// Enable sandbox pooling (default: enabled for high-performance pipelines)
    #[arg(long, default_value_t = true)]
    pool_enabled: bool,

    /// Minimum idle sandboxes per template (when pooling enabled)
    #[arg(long, default_value_t = 2)]
    pool_min_idle: usize,

    /// Maximum idle sandboxes per template (when pooling enabled)
    #[arg(long, default_value_t = 5)]
    pool_max_idle: usize,

    /// Maximum total pooled sandboxes
    #[arg(long, default_value_t = 50)]
    pool_max_total: usize,

    /// Pool idle timeout in milliseconds
    #[arg(long, default_value_t = 600_000)]
    pool_idle_timeout_ms: u64,

    /// Pool refill interval in milliseconds
    #[arg(long, default_value_t = 5_000)]
    pool_refill_interval_ms: u64,

    /// gRPC registry server address
    #[arg(long, default_value = "127.0.0.1:50052")]
    registry_addr: String,

    /// Disable mTLS for registry (for development/testing with plain HTTP).
    /// When set, workers can connect without client certificates.
    #[arg(long, default_value_t = false)]
    registry_no_tls: bool,

    /// HTTP server port for MCP protocol
    #[arg(long, default_value_t = 8080)]
    http_port: u16,

    /// Path to bastion config directory (default: .bastion in current dir or ~/.bastion)
    #[arg(long)]
    config_dir: Option<PathBuf>,

    /// Enable hot-reload of TOML configs (watch for file changes)
    #[arg(long, default_value_t = false)]
    watch_config: bool,

    /// Path to the SQLite database for sandbox persistence (default: ~/.bastion/sandboxes.db)
    #[arg(long)]
    db_path: Option<PathBuf>,

    /// Allow LocalProvider (DANGEROUS: runs commands directly on host filesystem)
    #[arg(long, default_value_t = false)]
    dangerous_allow_local: bool,

    /// Run retention cleanup on enrichment database at startup (deletes old rows based on retention policy)
    #[arg(long, default_value_t = false)]
    enrichment_retention_cleanup: bool,

    /// Test mode: skip slow initialization (no Podman, no pool, no TOML loading, in-memory DB).
    /// Useful for e2e tests that only need MCP protocol testing without real infrastructure.
    #[arg(long, default_value_t = false)]
    test_mode: bool,

    /// Subcommand to run.
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Run HTTP transport server using StreamableHttpService
async fn run_http_transport<S>(gateway: S, port: u16) -> Result<()>
where
    S: ServiceExt<RoleServer> + Clone + Send + 'static,
{
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("HTTP server listening on {}", addr);

    // Configure session with 30-minute keep-alive for long-running pipelines (Maven builds, etc.)
    // Default is 5 minutes (300s) which kills long operations.
    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config.keep_alive = Some(std::time::Duration::from_secs(1800));
    let session_manager = Arc::new(session_manager);
    let config = StreamableHttpServerConfig::default();
    let mcp_service =
        StreamableHttpService::new(move || Ok(gateway.clone()), session_manager, config);
    // Wrap with TowerToHyperService for hyper compatibility
    let service = TowerToHyperService::new(mcp_service);

    loop {
        let (stream, _) = listener.accept().await?;
        let mut service = service.clone();
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, &mut service)
                .await
            {
                tracing::warn!("HTTP serve error: {}", e);
            }
        });
    }
}

/// Run SSE server for pipeline events on a dedicated port.
/// Streams pipeline events to connected SSE clients (dashboard).
///
/// Uses channel-based streaming - each event is sent immediately as it arrives.
async fn run_pipeline_events_sse(pipeline_events: Arc<pipeline_events::PipelineEventStore>, port: u16) -> Result<()> {
    use hyper_util::rt::TokioIo;
    use http_body_util::{StreamBody, Empty, Either};
    use http_body::Frame;
    use tokio_stream::{StreamExt, wrappers::BroadcastStream};

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Pipeline events SSE server listening on {}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let pipeline_events = pipeline_events.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                let pipeline_events = pipeline_events.clone();
                async move {
                    // Check if this is a valid SSE request
                    let is_valid_path = req.method() == hyper::Method::GET && req.uri().path() == "/api/v1/pipeline-events";

                    // Stream-based SSE: convert broadcast receiver to StreamBody
                    let rx = pipeline_events.subscribe();

                    // Create a stream that yields SSE data frames
                    let stream = BroadcastStream::new(rx).filter_map(|result| {
                        match result {
                            Ok(event) => {
                                let sse = pipeline_events::event_to_sse(event);
                                if sse.is_empty() {
                                    None
                                } else {
                                    Some(Ok::<_, std::convert::Infallible>(Frame::data(hyper::body::Bytes::from(sse))))
                                }
                            }
                            Err(_) => None, // BroadcastStream wraps RecvError internally
                        }
                    });

                    let body: Either<Empty<hyper::body::Bytes>, StreamBody<_>> = if is_valid_path {
                        Either::Right(StreamBody::new(stream))
                    } else {
                        Either::Left(Empty::new())
                    };

                    let response = hyper::Response::builder()
                        .status(if is_valid_path { 200 } else { 404 })
                        .header("Content-Type", if is_valid_path { "text/event-stream" } else { "text/plain" })
                        .header("Cache-Control", "no-cache")
                        .header("Connection", "keep-alive")
                        .header("Access-Control-Allow-Origin", "*")
                        .header("X-Accel-Buffering", "no")
                        .body(body)
                        .unwrap();

                    Ok::<_, std::convert::Infallible>(response)
                }
            });

            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::warn!("SSE serve error: {}", e);
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs to stderr to keep stdout clean for MCP JSON-RPC protocol
    // Use default env filter but always write to stderr
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("bastion=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_level(true)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args = Args::parse();

    // Set DANGEROUS_ALLOW_LOCAL if flag is present
    if args.dangerous_allow_local {
        // SAFETY: This modifies process-global state but is intentional
        // when the user explicitly enables the dangerous local provider.
        unsafe { std::env::set_var("DANGEROUS_ALLOW_LOCAL", "1") };
        tracing::warn!(
            "DANGEROUS: LocalProvider is enabled. Commands will run on host filesystem!"
        );
    }

    tracing::info!("Bastion MCP Gateway starting...");

    // Install rustls crypto provider BEFORE any TLS code runs
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Create ~/.bastion directory structure and initialize security
    let bastion_home = dirs::home_dir()
        .map(|h| h.join(".bastion"))
        .unwrap_or_else(|| PathBuf::from(".bastion"));
    std::fs::create_dir_all(&bastion_home).context("Failed to create ~/.bastion directory")?;

    let jwt_manager = auth::JwtManager::init_or_load(&bastion_home)?;
    auto_tls::init_or_load(bastion_home.clone()).await?;

    // Extract HTTP port before async blocks (args will be moved)
    let _http_port = args.http_port;

    // Determine sandbox DB path
    let db_path = args.db_path.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".bastion").join("sandboxes.db"))
            .unwrap_or_else(|| PathBuf::from(".bastion/sandboxes.db"))
    });

    // Initialize SQLite repository (keep concrete type for sync_from_provider)
    let sqlite_repo = SqliteSandboxRepository::new(&db_path)
        .map_err(|e| anyhow::anyhow!("Failed to initialize SQLite repository: {}", e))?;

    // Initialize secret resolver (reads from environment variables)
    let secret_resolver: Arc<dyn SecretResolver> = Arc::new(EnvSecretResolver::new());

    // Determine config directory path
    let bastion_config_dir = args.config_dir.clone().unwrap_or_else(|| {
        // Check current dir first, then home dir
        let local = PathBuf::from(".bastion");
        if local.exists() {
            local
        } else {
            dirs::home_dir()
                .map(|h| h.join(".bastion"))
                .unwrap_or_else(|| PathBuf::from(".bastion"))
        }
    });

    // Create provider registry and register Podman (backward compat)
    let registry = ProviderRegistry::new(ProviderFactory::new("podman"));

    // Load provider configs from .bastion/providers/ if directory exists
    let providers_dir = bastion_config_dir.join("providers");
    match registry.load_from_dir(&providers_dir) {
        Ok(count) => {
            if count > 0 {
                tracing::info!(count, path = %providers_dir.display(), "Loaded TOML provider configs");
            } else {
                tracing::info!(
                    "No TOML provider configs found in {}, using hardcoded defaults",
                    providers_dir.display()
                );
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, path = %providers_dir.display(), "Failed to load provider configs, using hardcoded defaults");
        }
    }

    // Create the RegistryService (gRPC) with JWT + auto_tls + auth config support
    use pipeline_events::PipelineEventStore;
    let pipeline_events = Arc::new(PipelineEventStore::new());
    let grpc_registry: Arc<RegistryService> = Arc::new(RegistryService::new(
        jwt_manager.clone(),
        Arc::new(auto_tls::get_auto_tls().clone()),
        server::AuthConfig::default(),
        pipeline_events.clone(),
    ));

    // Start watchdog to detect dead workers (10s heartbeat, 30s watchdog timeout)
    grpc_registry.start_watchdog(10000);

    // Create PodmanProvider WITHOUT doing network I/O yet (ping is deferred).
    // This is fast — no socket connection involved.
    let podman: Arc<dyn SandboxProvider> = if args.test_mode {
        // In test mode, skip Podman entirely — use NullProvider directly
        tracing::warn!("TEST MODE: using NullProvider (no real sandbox operations)");
        Arc::new(NullProvider::new("test mode".to_string())) as Arc<dyn SandboxProvider>
    } else {
        let podman_result = PodmanProvider::new(
            &args.socket,
            &args.image,
            PathBuf::from(&args.worker_binary),
        );
        match podman_result {
            Ok(mut p) => {
                p.set_command_router(grpc_registry.clone() as Arc<dyn CommandRouter>);

                // Quick ping to verify Podman is reachable
                match p.ping().await {
                    Ok(pong) => tracing::info!(pong = %pong, "Connected to Podman"),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to ping Podman, containers may not be reachable")
                    }
                }
                tracing::info!("Podman provider initialized");
                Arc::new(p)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    socket = %args.socket,
                    "Podman not available — sandbox operations will fail."
                );
                Arc::new(NullProvider::new(e.to_string())) as Arc<dyn SandboxProvider>
            }
        }
    };
    registry.register("podman", podman.clone());

    // Sync repository with provider state BEFORE wrapping in Arc (sync_from_provider needs concrete type)
    // Skip in test mode — NullProvider returns no sandboxes
    if !args.test_mode {
        if let Err(e) = sqlite_repo.sync_from_provider(podman.as_ref()).await {
            tracing::warn!(error = %e, "Failed to sync sandboxes from provider — continuing anyway");
        } else {
            tracing::info!("Sandbox repository synced with provider");
        }
    }

    // Wrap sqlite_repo as Arc<dyn SandboxRepository> for gateway use
    let repository: Arc<dyn SandboxRepository> = Arc::new(sqlite_repo);

    // Optionally create pool manager — pool fill is deferred to background
    // In test mode, skip pool entirely (NullProvider doesn't support pool operations)
    let pool_manager: Option<Arc<SandboxPoolManager>> = if args.test_mode {
        tracing::warn!("TEST MODE: pool disabled");
        None
    } else if args.pool_enabled {
        let pool_config = PoolConfig {
            min_idle: args.pool_min_idle,
            max_idle: args.pool_max_idle,
            max_total: args.pool_max_total,
            idle_timeout_ms: args.pool_idle_timeout_ms,
            refill_interval_ms: args.pool_refill_interval_ms,
        };

        let manager = Arc::new(SandboxPoolManager::new(
            podman.clone(),
            repository.clone(),
            pool_config,
        ));
        manager.register_template(&args.image);

        // Defer pool fill to background so MCP serve starts quickly
        let pool_for_start = Arc::clone(&manager);
        tokio::spawn(async move {
            if let Err(e) = pool_for_start.start().await {
                tracing::warn!(error = %e, "Pool manager failed to start");
            } else {
                tracing::info!("Sandbox pool initialized");
            }
        });
        Some(manager)
    } else {
        None
    };

    let pool_enabled = pool_manager.is_some();
    if pool_enabled {
        tracing::info!("Sandbox pooling enabled");
    } else {
        tracing::info!("Sandbox pooling disabled");
    }

    // Start background expiration enforcer: terminates expired sandboxes every 60s
    {
        let repo_for_expiry = repository.clone();
        let provider_for_expiry = podman.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                match repo_for_expiry.find_expired().await {
                    Ok(expired) => {
                        if !expired.is_empty() {
                            tracing::info!(
                                count = expired.len(),
                                "Found expired sandboxes, terminating..."
                            );
                            for sandbox in expired {
                                tracing::info!(sandbox_id = %sandbox.id, "Terminating expired sandbox");
                                match provider_for_expiry.terminate(&sandbox.id).await {
                                    Ok(()) => {
                                        if let Err(e) = repo_for_expiry.update(&Sandbox {
                                            status: bastion_domain::sandbox::value_objects::SandboxStatus::Stopped,
                                            ..sandbox
                                        }).await {
                                            tracing::warn!(error = %e, "Failed to update expired sandbox status");
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Failed to terminate expired sandbox");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to query expired sandboxes");
                    }
                }
            }
        });
    }

    // Create gateway metrics
    let metrics = GatewayMetrics::default();

    // Create MetricsHub for historical metrics and heartbeat data (Phase 4)
    // Wrapped in Arc<tokio::sync::Mutex<>> — tokio Mutex guards are Send,
    // allowing us to hold the lock across .await points in tool handlers
    let metrics_db_path = bastion_home.join("metrics.db");
    let metrics_hub = Arc::new(tokio::sync::Mutex::new(
        MetricsHub::new(Arc::new(metrics.clone()), Some(metrics_db_path))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize MetricsHub: {}", e))?,
    ));

    // Wire HeartbeatBridge from MetricsHub to RegistryService for worker resource tracking
    // This enables per-sandbox CPU/memory monitoring via worker heartbeat data
    {
        let hub = metrics_hub.lock().await;
        let heartbeat_bridge = hub.heartbeat_bridge();
        grpc_registry.set_heartbeat_bridge(heartbeat_bridge);
    }

    // Clone pool_manager for potential cleanup after server exits
    let pool_manager_cleanup = pool_manager.clone();

    // Create capability registry and load TOML configs from .bastion/capabilities/
    let capability_registry = CapabilityRegistry::new();
    let capabilities_dir = bastion_config_dir.join("capabilities");
    if capabilities_dir.exists() {
        match capability_registry.load_from_dir(&capabilities_dir) {
            Ok(count) => {
                if count > 0 {
                    tracing::info!(count, path = %capabilities_dir.display(), "Loaded TOML capability configs");
                } else {
                    tracing::info!(
                        "No capability TOMLs found in {}, using hardcoded resolvers",
                        capabilities_dir.display()
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %capabilities_dir.display(), "Failed to load capability configs, using hardcoded resolvers");
            }
        }
    } else {
        tracing::info!(
            "No capabilities directory at {}, using hardcoded resolvers",
            capabilities_dir.display()
        );
    }

    // Create experience store (SQLite-backed)
    let experience_db_path = bastion_home.join("experiences.db");
    let experience_store = match SqliteExperienceStore::new(&experience_db_path) {
        Ok(store) => {
            tracing::info!(path = %experience_db_path.display(), "Experience store initialized");
            Some(Arc::new(store)
                as Arc<
                    dyn bastion_domain::catalog::experience::ExperienceStore,
                >)
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to initialize experience store, catalog features disabled");
            None
        }
    };

    // Create assertion registry and load TOML files
    let assertion_registry = Arc::new({
        let registry = AssertionRegistry::new();
        let assertions_dir = bastion_config_dir.join("catalog").join("assertions");
        if assertions_dir.exists() {
            match registry.load_from_dir(&assertions_dir) {
                Ok(count) => {
                    tracing::info!(count, path = %assertions_dir.display(), "Loaded assertion files");
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %assertions_dir.display(), "Failed to load assertions");
                }
            }
        } else {
            tracing::info!(
                "No assertions directory at {}, using empty registry",
                assertions_dir.display()
            );
        }
        registry
    });

    // Create doctor registry and load TOML files
    let doctor_registry = Arc::new({
        let registry = DoctorRegistry::new();
        let doctors_dir = bastion_config_dir.join("catalog").join("doctors");
        if doctors_dir.exists() {
            match registry.load_from_dir(&doctors_dir) {
                Ok(count) => {
                    tracing::info!(count, path = %doctors_dir.display(), "Loaded doctor files");
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %doctors_dir.display(), "Failed to load doctors");
                }
            }
        } else {
            tracing::info!(
                "No doctors directory at {}, using empty registry",
                doctors_dir.display()
            );
        }
        registry
    });

    // Create advice registry and load TOML files
    let advice_registry = Arc::new({
        let registry = AdviceRegistry::new();
        let advice_dir = bastion_config_dir.join("catalog").join("advice");
        if advice_dir.exists() {
            match registry.load_from_dir(&advice_dir) {
                Ok(count) => {
                    tracing::info!(count, path = %advice_dir.display(), "Loaded advice files");
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %advice_dir.display(), "Failed to load advice");
                }
            }
        } else {
            tracing::info!(
                "No advice directory at {}, using empty registry",
                advice_dir.display()
            );
        }
        registry
    });

    // Create advice config store (`.bastion/advice.toml`)
    let advice_config = Arc::new({
        let config_path = bastion_config_dir.join("advice.toml");
        AdviceConfigStore::new(config_path)
    });

    // Initialize enrichment catalog: SQLite-backed repository with built-in + file-based enrichers
    let enrichment_catalog_db_path = bastion_home.join("enrichment_catalog.db");
    let (enrichment_adapter, enrichment_config) =
        match bastion_infrastructure::enrichment::SqliteCatalogRepository::new(
            &enrichment_catalog_db_path,
        ) {
            Ok(catalog_repo) => {
                let catalog_repo = Arc::new(catalog_repo);
                let importer =
                    bastion_infrastructure::enrichment::YamlCatalogImporter::new(&catalog_repo);

                // Load built-in enrichers from YAML catalog (maven.yaml) via YamlCatalogImporter
                let enrichers_src_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("enrichment-engine")
                    .join("src")
                    .join("enrichers");
                let maven_yaml_path = enrichers_src_dir.join("maven.yaml");
                match importer.import_file(&maven_yaml_path).await {
                    Ok(()) => {
                        tracing::info!(path = %maven_yaml_path.display(), "Loaded built-in enricher from YAML catalog");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, path = %maven_yaml_path.display(), "Failed to load built-in enricher from YAML catalog — continuing startup in degraded mode");
                    }
                }

                // Import from catalog directory (if present)
                let enrichers_dir = bastion_config_dir.join("catalog").join("enrichers");
                if enrichers_dir.exists() {
                    match importer.import_dir(&enrichers_dir).await {
                        Ok(count) if count > 0 => {
                            tracing::info!(count, path = %enrichers_dir.display(), "Imported enricher descriptors from catalog");
                        }
                        Ok(_) => {
                            tracing::debug!(
                                "No enricher descriptor files found in {}",
                                enrichers_dir.display()
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, path = %enrichers_dir.display(), "Failed to import enricher descriptors");
                        }
                    }
                } else {
                    tracing::debug!(
                        "No enrichers catalog directory at {}, using built-in enrichers only",
                        enrichers_dir.display()
                    );
                }

                // Create the BastionEnrichmentAdapter wrapping the catalog repository
                let enrichment_cfg = bastion_infrastructure::enrichment::EnrichmentConfig {
                    enabled: true,
                    catalog_dir: enrichers_dir.clone(),
                    retention: bastion_infrastructure::enrichment::RetentionConfig::default(),
                    semaphore: bastion_infrastructure::enrichment::SemaphoreConfig::default(),
                };
                let adapter = bastion_infrastructure::enrichment::BastionEnrichmentAdapter::new(
                    catalog_repo,
                    registry.default().clone(),
                    enrichment_cfg.clone(),
                );

                // Initialize the enrichment runs recorder (persistence for Meta-Harness optimization)
                let enrichment_runs_db_path = bastion_home.join("data").join("enrichment_runs.db");
                let (adapter, _enrichment_log) =
                    match bastion_infrastructure::enrichment::SqliteRunRecorder::new(
                        &enrichment_runs_db_path,
                    ) {
                        Ok(recorder) => {
                            // Run retention cleanup if requested via CLI flag (before wrapping in Arc)
                            if args.enrichment_retention_cleanup {
                                match recorder.cleanup().await {
                                    Ok(deleted) if deleted > 0 => {
                                        tracing::info!(
                                            rows_deleted = deleted,
                                            "Retention cleanup completed at startup"
                                        );
                                    }
                                    Ok(_) => {
                                        tracing::debug!(
                                            "Retention cleanup completed, no rows deleted"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Retention cleanup failed at startup, continuing");
                                    }
                                }
                            }

                            let concrete_recorder = Arc::new(recorder);
                            let run_recorder: Arc<dyn enrichment_engine::traits::RunRecorder> =
                                concrete_recorder.clone();
                            let adapter_arc = Arc::new(adapter);

                            // Create optimizer repository from the same recorder
                            let optimizer_repo = Arc::new(
                                bastion_infrastructure::enrichment::SqliteOptimizerRepository::new(
                                    concrete_recorder,
                                ),
                            );
                            let adapter_with_recorder_and_optimizer = bastion_infrastructure::enrichment::BastionEnrichmentAdapter::with_recorder(adapter_arc, run_recorder)
                        .with_optimizer_repo(optimizer_repo);
                            tracing::info!(
                                "Enrichment catalog initialized at {}, runs recorded at {}",
                                enrichment_catalog_db_path.display(),
                                enrichment_runs_db_path.display()
                            );
                            // adapter_with_recorder_and_optimizer is Arc<BastionEnrichmentAdapter>, and BastionGateway expects Arc<Option<BastionEnrichmentAdapter>>
                            let inner = Arc::try_unwrap(adapter_with_recorder_and_optimizer)
                                .unwrap_or_else(|arc| (*arc).clone());
                            (inner, Some(enrichment_runs_db_path.clone()))
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to initialize enrichment run recorder, continuing without recording");
                            tracing::info!(
                                "Enrichment catalog initialized at {}",
                                enrichment_catalog_db_path.display()
                            );
                            (adapter, None)
                        }
                    };
                (Arc::new(Some(adapter)), enrichment_cfg)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize enrichment catalog, enrichment disabled");
                (
                    Arc::new(None),
                    bastion_infrastructure::enrichment::EnrichmentConfig::default(),
                )
            }
        };

    // Create gateway — ready to serve MCP immediately
    let default_provider = registry.default().clone();
    let providers_map = registry.into_providers();
    let gateway_config = server::GatewayConfig {
        pool_manager,
        metrics,
        metrics_hub: Some(metrics_hub.clone()),
        auto_tls: Arc::new(auto_tls::get_auto_tls().clone()),
        auth: server::AuthConfig::default(),
    };
    let catalog_config = server::CatalogConfig {
        experience_store,
        assertion_registry: Some(assertion_registry),
        doctor_registry: Some(doctor_registry.clone()),
        advice_registry: Some(advice_registry),
        advice_config: Some(advice_config),
    };
    // Create doctor context for pre-flight readiness checks in sandbox_create
    let doctor_context = Some(Arc::new(server::DoctorContext {
        doctor_registry: doctor_registry,
    }));
    let gateway = server::BastionGateway::new(
        default_provider,
        providers_map,
        repository.clone(),
        secret_resolver.clone(),
        gateway_config,
        capability_registry,
        catalog_config,
        doctor_context,
        enrichment_adapter,
        enrichment_config,
    );

    // Start the Worker Registry gRPC server with AutoTLS (mandatory mTLS unless --registry-no-tls)
    let registry_addr: std::net::SocketAddr = args
        .registry_addr
        .parse()
        .expect("Invalid registry address");

    let registry_for_grpc = Arc::clone(&grpc_registry);
    let registry_no_tls = args.registry_no_tls;
    let registry_handle = tokio::spawn(async move {
        let svc = WorkerRegistryServer::new((*registry_for_grpc).clone())
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip);

        let mut builder = tonic::transport::Server::builder()
            .http2_adaptive_window(Some(true))
            .initial_stream_window_size(1024 * 1024)
            .initial_connection_window_size(4 * 1024 * 1024)
            .max_frame_size(4 * 1024 * 1024);

        if registry_no_tls {
            tracing::info!("Starting registry WITHOUT TLS (plaintext - DEV MODE)");
        } else {
            let tls_config = auto_tls::get_auto_tls()
                .server_config()
                .expect("Failed to get AutoTLS server config");
            tracing::info!("Starting registry with AutoTLS (mTLS) + gzip compression");
            builder = builder.tls_config(tls_config).expect("TLS config failed");
        }

        builder
            .add_service(svc)
            .serve(registry_addr)
            .await
            .expect("Worker registry server failed");
    });
    tracing::info!("Worker registry listening on {}", registry_addr);

    // SSE server for pipeline events on port 8081
    let pipeline_events_sse_port = 8081;
    let pipeline_events_for_sse = pipeline_events.clone();
    let sse_handle = tokio::spawn(async move {
        run_pipeline_events_sse(pipeline_events_for_sse, pipeline_events_sse_port).await
    });
    tracing::info!("Pipeline events SSE server starting on port {}", pipeline_events_sse_port);

    // HTTP transport for MCP protocol
    tracing::info!("MCP Gateway ready — HTTP transport on port {}", args.http_port);
    if args.registry_no_tls {
        tracing::info!("Worker Registry — gRPC on port 50052 (plaintext)");
    } else {
        tracing::info!("Worker Registry — gRPC on port 50052 (mTLS)");
    }
    tracing::info!("Pipeline Events — SSE on port 8081");

    // Run all services: HTTP MCP, Worker Registry gRPC, and SSE in parallel
    let http_future = run_http_transport(gateway, args.http_port);

    tokio::select! {
        result = http_future => {
            if let Err(e) = result {
                tracing::error!("HTTP transport error: {}", e);
            }
        }
        result = registry_handle => {
            if let Err(e) = result {
                tracing::error!("Worker registry error: {}", e);
            }
        }
        result = sse_handle => {
            if let Err(e) = result {
                tracing::error!("Pipeline events SSE error: {}", e);
            }
        }
    }

    // Cleanup pool manager if enabled
    if let Some(pm) = pool_manager_cleanup {
        let _ = pm.stop().await;
    }

    Ok(())
}

/// Fallback provider that errors on all sandbox operations.
/// Used when no container runtime is available, allowing the gateway
/// to still serve health checks and tool listings.
struct NullProvider {
    reason: String,
}

impl NullProvider {
    fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl std::fmt::Debug for NullProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NullProvider")
            .field("reason", &self.reason)
            .finish()
    }
}

// ── SandboxLifecycle ─────────────────────────────────────────────

#[async_trait::async_trait]
impl SandboxLifecycle for NullProvider {
    async fn create(
        &self,
        _id: &SandboxId,
        _template: &str,
        _resources: &ResourcesSpec,
        _network: &NetworkSpec,
        _env_vars: &HashMap<String, String>,
        _timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        Err(DomainError::ProviderUnavailable(format!(
            "No provider available: {}",
            self.reason
        )))
    }

    async fn terminate(&self, _id: &SandboxId) -> Result<(), DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
        Ok(false)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    fn name(&self) -> &str {
        "null"
    }

    async fn list_sandboxes(&self, _filter: &SandboxFilter) -> Result<Vec<Sandbox>, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn set_timeout(&self, _id: &SandboxId, _timeout_ms: u64) -> Result<(), DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn create_snapshot(
        &self,
        _id: &SandboxId,
        _name: &str,
    ) -> Result<SnapshotInfo, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }
}

// ── TaskExecutor ─────────────────────────────────────────────────

#[async_trait::async_trait]
impl TaskExecutor for NullProvider {
    async fn run_command(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandResult, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn run_command_stream(
        &self,
        _id: &SandboxId,
        _command: &CommandSpec,
    ) -> Result<CommandStream, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn write_file(
        &self,
        _id: &SandboxId,
        _path: &str,
        _content: &[u8],
    ) -> Result<(), DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn read_file(&self, _id: &SandboxId, _path: &str) -> Result<Vec<u8>, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }

    async fn list_files(&self, _id: &SandboxId, _dir: &str) -> Result<Vec<FileEntry>, DomainError> {
        Err(DomainError::ProviderUnavailable(self.reason.clone()))
    }
}
