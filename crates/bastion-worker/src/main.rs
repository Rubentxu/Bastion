//! Bastion Sandbox Worker
//!
//! Runs inside each sandbox as a gRPC server.
//! The gateway connects to this worker to execute commands and manage files.

use anyhow::Result;
use clap::Parser;
use crate::sandbox::v1::worker_agent_server::WorkerAgentServer;
use tonic::transport::Server;

mod sandbox;
mod worker;

#[derive(Parser, Debug)]
#[command(name = "bastion-worker", version)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:50051")]
    grpc_addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let args = Args::parse();

    let addr = args.grpc_addr.parse()?;
    let service = WorkerAgentServer::new(worker::WorkerService);

    tracing::info!("Worker Agent starting on {}", addr);

    Server::builder()
        .add_service(service)
        .serve(addr)
        .await?;

    Ok(())
}
