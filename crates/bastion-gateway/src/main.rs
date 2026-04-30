//! Bastion MCP Gateway
//!
//! Entry point for the sandbox gateway MCP server.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::persistence::InMemorySandboxRepository;
use bastion_infrastructure::pool::{PoolConfig, SandboxPoolManager};
use bastion_infrastructure::provider::{PodmanProvider, ProviderFactory};

use rmcp::ServiceExt;

mod server;

#[derive(Parser, Debug)]
#[command(name = "bastion-gateway", version, about = "Bastion MCP Gateway")]
struct Args {
    /// Path to Podman socket
    #[arg(long, default_value = "/run/user/1000/podman/podman.sock")]
    socket: String,

    /// Default image to use for sandboxes
    #[arg(long, default_value = "debian:bookworm-slim")]
    image: String,

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

    let podman =
        PodmanProvider::new(&args.socket, &args.image).expect("Failed to connect to Podman");

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

    tracing::info!("MCP Gateway ready — serving on stdio");

    let service = gateway
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start MCP server: {e}"))?;

    // Wait for shutdown
    service.waiting().await?;

    // Cleanup pool manager if enabled
    if let Some(pm) = pool_manager_cleanup {
        let _ = pm.stop().await;
    }

    Ok(())
}
