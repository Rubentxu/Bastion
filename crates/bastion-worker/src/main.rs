//! Bastion Sandbox Worker
//!
//! Runs inside each sandbox as a gRPC server.
//! The gateway connects to this worker to execute commands and manage files.

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "bastion-worker", version, about = "Bastion Sandbox Worker")]
struct Args {
    /// gRPC listen address
    #[arg(long, default_value = "0.0.0.0:50051")]
    grpc_addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bastion=debug".parse()?),
        )
        .init();

    let args = Args::parse();

    tracing::info!(
        grpc_addr = %args.grpc_addr,
        "Bastion Worker starting..."
    );

    // TODO: Start gRPC server with tonic
    // For PoC: just log and wait
    tracing::info!("Worker ready (stub)");

    // Wait for shutdown signal (using std::future::pending() for PoC stub)
    std::future::pending::<()>().await;
    tracing::info!("Worker shutting down");

    Ok(())
}
