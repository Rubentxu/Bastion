//! Bastion MCP Gateway
//!
//! Entry point for the sandbox gateway MCP server.

use anyhow::Result;
use clap::Parser;

mod server;

#[derive(Parser, Debug)]
#[command(name = "bastion-gateway", version, about = "Bastion MCP Gateway")]
struct Args {
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

    let _args = Args::parse();

    tracing::info!("Bastion MCP Gateway starting...");

    // TODO: Load config, initialize providers, start MCP server
    // For PoC: just start the MCP server with a simple echo tool

    let _gateway = server::BastionGateway;
    tracing::info!("MCP Gateway ready (stub)");

    // Block forever - in production this would start the MCP server
    std::future::pending::<()>().await;

    Ok(())
}
