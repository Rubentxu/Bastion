//! Bastion Worker Bootstrap
//!
//! Downloads the bastion-worker binary and executes it.
//! Designed for Lambda/FaaS environments where the worker binary
//! is not pre-installed.
//!
//! Environment variables:
//! - BASTION_WORKER_URL: URL to download worker binary (required)
//! - BASTION_WORKER_SHA256: Expected sha256 of binary (optional, verified if set)
//! - BASTION_GATEWAY_ADDR: Gateway address for worker (passed to worker)
//! - BASTION_SANDBOX_ID: Sandbox ID (passed to worker)
//! - BASTION_AUTH_TOKEN: Auth token (passed to worker)

use sha2::Digest;
use std::env;
use std::fs;
use std::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let worker_url = env::var("BASTION_WORKER_URL")
        .expect("BASTION_WORKER_URL is required");
    let expected_sha256 = env::var("BASTION_WORKER_SHA256").ok();
    
    let worker_path = "/tmp/bastion-worker";
    
    // Download worker binary
    println!("Downloading worker from {}", worker_url);
    download_file(&worker_url, worker_path).await?;
    
    // Verify sha256 if provided
    if let Some(expected) = &expected_sha256 {
        let actual = sha256_file(worker_path)?;
        if &actual != expected {
            fs::remove_file(worker_path)?;
            return Err(format!("SHA256 mismatch: expected {}, got {}", expected, actual).into());
        }
        println!("SHA256 verified");
    }
    
    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(worker_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(worker_path, perms)?;
    }
    
    // Execute worker with remaining env vars
    let mut cmd = Command::new(worker_path);
    for (key, value) in env::vars() {
        if key.starts_with("BASTION_") && key != "BASTION_WORKER_URL" && key != "BASTION_WORKER_SHA256" {
            cmd.env(&key, &value);
        }
    }
    // Also pass non-BASTION_ vars that worker needs
    if let Some(gateway) = env::var_os("BASTION_GATEWAY_ADDR") {
        cmd.arg("--gateway-addr").arg(gateway);
    }
    if let Some(sandbox_id) = env::var_os("BASTION_SANDBOX_ID") {
        cmd.arg("--sandbox-id").arg(sandbox_id);
    }
    if let Some(secret) = env::var_os("BASTION_AUTH_TOKEN") {
        cmd.arg("--secret").arg(secret);
    }
    
    println!("Executing worker: {:?}", cmd);
    let status = cmd.status()?;
    
    std::process::exit(status.code().unwrap_or(1));
}

async fn download_file(url: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;
    fs::write(path, &bytes)?;
    println!("Downloaded {} bytes", bytes.len());
    Ok(())
}

fn sha256_file(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{BufReader, Read};
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 { break; }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
