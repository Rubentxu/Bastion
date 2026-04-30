//! Bastion MCP Gateway
//!
//! Entry point for the sandbox gateway MCP server.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use bastion_domain::provider::router::CommandRouter;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::persistence::InMemorySandboxRepository;
use bastion_infrastructure::pool::{PoolConfig, SandboxPoolManager};
use bastion_infrastructure::provider::{PodmanProvider, ProviderFactory};

use rmcp::ServiceExt;

mod registry;
mod sandbox;
mod server;
mod tls;

use registry::{RegistryService, WorkerRegistryServer};
use tonic::codec::CompressionEncoding;

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

    /// Enable sandbox pooling
    #[arg(long, default_value_t = false)]
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

    /// TLS certificate path (optional, enables TLS for registry)
    #[arg(long)]
    tls_cert: Option<String>,

    /// TLS private key path (optional, enables TLS for registry)
    #[arg(long)]
    tls_key: Option<String>,
}

impl Args {
    fn tls_config(&self) -> Option<tls::TlsConfig> {
        match (&self.tls_cert, &self.tls_key) {
            (Some(cert), Some(key)) => Some(tls::TlsConfig {
                cert_path: cert.clone(),
                key_path: key.clone(),
            }),
            _ => None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bastion=debug".parse()?),
        )
        .json()
        .init();

    let args = Args::parse();

    tracing::info!("Bastion MCP Gateway starting...");

    // Initialize repository
    let repository: Arc<dyn SandboxRepository> = Arc::new(InMemorySandboxRepository::new());

    // Create provider factory and register Podman
    let mut factory = ProviderFactory::new("podman");

    // First create the RegistryService (before PodmanProvider so we can wire it)
    let registry: Arc<RegistryService> = Arc::new(RegistryService::new());

    // Start watchdog to detect dead workers (10s heartbeat, 30s watchdog timeout)
    registry.start_watchdog(10000);

    let mut podman =
        PodmanProvider::new(&args.socket, &args.image, PathBuf::from(&args.worker_binary))
            .expect("Failed to connect to Podman");

    // Wire the command router so PodmanProvider can route commands through the registry
    podman.set_command_router(registry.clone() as Arc<dyn CommandRouter>);

    // Verify connection to Podman
    match podman.ping().await {
        Ok(pong) => tracing::info!(pong = %pong, "Connected to Podman"),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to ping Podman, containers may not be reachable")
        }
    }

    let podman = Arc::new(podman) as Arc<dyn SandboxProvider>;
    factory.register("podman", podman.clone());

    // Optionally create pool manager
    let pool_manager: Option<Arc<SandboxPoolManager>> = if args.pool_enabled {
        let pool_config = PoolConfig {
            min_idle: args.pool_min_idle,
            max_idle: args.pool_max_idle,
            max_total: args.pool_max_total,
            idle_timeout_ms: args.pool_idle_timeout_ms,
            refill_interval_ms: args.pool_refill_interval_ms,
        };

        let manager = SandboxPoolManager::new(podman.clone(), repository.clone(), pool_config);

        // Register the default template with the pool
        manager.register_template(&args.image);

        manager.start().await?;
        Some(Arc::new(manager))
    } else {
        None
    };

    let pool_enabled = pool_manager.is_some();
    if pool_enabled {
        tracing::info!("Sandbox pooling enabled");
    } else {
        tracing::info!("Sandbox pooling disabled");
    }

    // Clone pool_manager for potential cleanup after server exits
    let pool_manager_cleanup = pool_manager.clone();

    // Create gateway metrics
    let metrics = GatewayMetrics::default();

    // Create gateway and start MCP server
    let gateway =
        server::BastionGateway::new(podman.clone(), repository.clone(), pool_manager, metrics);

    // Start the Worker Registry gRPC server (with optional TLS)
    let registry_addr: std::net::SocketAddr = args.registry_addr.parse()
        .expect("Invalid registry address");

    let registry_for_grpc = Arc::clone(&registry);
    let registry_handle = tokio::spawn(async move {
        // Create service with gzip compression enabled
        let svc = WorkerRegistryServer::new((*registry_for_grpc).clone())
            .accept_compressed(CompressionEncoding::Gzip)
            .send_compressed(CompressionEncoding::Gzip);

        if let Some(ref tls_cfg) = args.tls_config()
            && tls_cfg.files_exist()
        {
            match tls_cfg.load_identity() {
                Ok(identity) => {
                    tracing::info!("Starting registry with TLS + gzip compression");
                    let tls_config = tonic::transport::ServerTlsConfig::new()
                        .identity(identity);
                    tonic::transport::Server::builder()
                        .http2_adaptive_window(Some(true))
                        .initial_stream_window_size(1024 * 1024)  // 1MB
                        .initial_connection_window_size(4 * 1024 * 1024) // 4MB
                        .max_frame_size(4 * 1024 * 1024)  // 4MB
                        .tls_config(tls_config)
                        .expect("TLS config failed")
                        .add_service(svc)
                        .serve(registry_addr)
                        .await
                        .expect("TLS registry server failed");
                    return;
                }
                Err(e) => {
                    tracing::error!("Failed to load TLS identity: {}, falling back to plaintext", e);
                }
            }
        }

        // Plaintext fallback with HTTP/2 tuning
        tracing::info!("Starting registry without TLS (plaintext) + gzip compression");
        tonic::transport::Server::builder()
            .http2_adaptive_window(Some(true))
            .initial_stream_window_size(1024 * 1024)  // 1MB
            .initial_connection_window_size(4 * 1024 * 1024) // 4MB
            .max_frame_size(4 * 1024 * 1024)  // 4MB
            .add_service(svc)
            .serve(registry_addr)
            .await
            .expect("Worker registry server failed");
    });
    tracing::info!("Worker registry listening on {}", registry_addr);

    tracing::info!("MCP Gateway ready — serving on stdio");

    // Run MCP server and registry in parallel
    let mcp_future = async {
        let service = gateway
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start MCP server: {e}"))?;
        let _ = service.waiting().await;
        Ok::<(), anyhow::Error>(())
    };

    // Wait for either to finish
    tokio::select! {
        result = mcp_future => {
            if let Err(e) = result {
                tracing::error!("MCP server error: {}", e);
            }
        }
        result = registry_handle => {
            if let Err(e) = result {
                tracing::error!("Registry server error: {}", e);
            }
        }
    }

    // Cleanup pool manager if enabled
    if let Some(pm) = pool_manager_cleanup {
        let _ = pm.stop().await;
    }

    Ok(())
}