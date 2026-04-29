//! Bastion MCP Gateway
//!
//! Entry point for the sandbox gateway MCP server.

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_infrastructure::persistence::InMemorySandboxRepository;
use bastion_infrastructure::provider::PodmanProvider;

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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bastion=debug".parse()?)
        )
        .json()
        .init();

    let args = Args::parse();

    tracing::info!("Bastion MCP Gateway starting...");

    // Initialize provider
    let podman = PodmanProvider::new(&args.socket, &args.image)
        .expect("Failed to connect to Podman");

    // Verify connection to Podman
    match podman.ping().await {
        Ok(pong) => tracing::info!(pong = %pong, "Connected to Podman"),
        Err(e) => tracing::warn!(error = %e, "Failed to ping Podman, containers may not be reachable"),
    }

    let provider: Arc<dyn SandboxProvider> = Arc::new(podman);

    // Initialize repository
    let repository: Arc<dyn SandboxRepository> = Arc::new(InMemorySandboxRepository::new());

    // Create gateway and start MCP server
    let gateway = server::BastionGateway::new(provider, repository);

    tracing::info!("MCP Gateway ready — serving on stdio");

    let service = gateway
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start MCP server: {e}"))?;
    service.waiting().await?;

    Ok(())
}
