//! Bastion Sandbox Worker
//!
//! Runs inside each sandbox as a gRPC CLIENT.
//! Connects OUTBOUND to the Gateway (JNLP-inspired pattern).

use anyhow::Result;
use clap::Parser;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;

mod sandbox;

use sandbox::v2::worker_registry_client::WorkerRegistryClient;
use sandbox::v2::*;

#[derive(Parser, Debug)]
#[command(name = "bastion-worker", version)]
struct Args {
    /// Gateway address to connect to (outbound)
    #[arg(long, default_value = "http://127.0.0.1:50052")]
    gateway_addr: String,

    /// Sandbox ID (set by the provider at container creation)
    #[arg(long, default_value = "unknown")]
    sandbox_id: String,

    /// Pre-shared secret for authentication
    #[arg(long, default_value = "")]
    secret: String,

    /// Working directory inside the sandbox
    #[arg(long, default_value = "/workspace")]
    workdir: String,
}

/// Track active commands for health reporting
static ACTIVE_COMMANDS: AtomicU32 = AtomicU32::new(0);

/// Maximum concurrent commands per worker
const MAX_CONCURRENT_COMMANDS: usize = 4;

fn active_command_count() -> i32 {
    ACTIVE_COMMANDS.load(Ordering::Relaxed) as i32
}

#[derive(Debug)]
enum ExitReason {
    Shutdown,
    StreamEnded,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bastion_worker=debug".parse()?),
        )
        .init();

    let args = Args::parse();

    tracing::info!(
        sandbox_id = %args.sandbox_id,
        gateway = %args.gateway_addr,
        "Bastion Worker starting"
    );

    let mut attempt: u32 = 0;
    let base_delay = std::time::Duration::from_secs(1);
    let max_delay = std::time::Duration::from_secs(60);

    loop {
        match run_worker_session(&args).await {
            Ok(ExitReason::Shutdown) => {
                tracing::info!("Graceful shutdown, exiting");
                return Ok(());
            }
            Ok(ExitReason::StreamEnded) => {
                tracing::info!("Stream ended cleanly, reconnecting immediately");
                attempt = 0;
                continue;
            }
            Err(e) => {
                let delay = next_backoff(attempt, base_delay, max_delay);
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    retry_in_ms = delay.as_millis(),
                    "Worker session failed, reconnecting"
                );
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn next_backoff(attempt: u32, base: std::time::Duration, max: std::time::Duration) -> std::time::Duration {
    let exp_ms = base.as_millis() as u64 * 2u64.saturating_pow(attempt.min(6));
    // Add jitter: random 0-500ms based on current time nanos
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64 % 500;
    let total = std::time::Duration::from_millis(exp_ms + jitter);
    total.min(max)
}

async fn run_worker_session(args: &Args) -> Result<ExitReason> {
    // Connect to Gateway (outbound)
    let channel = Channel::from_shared(args.gateway_addr.clone())
        .map_err(|e| anyhow::anyhow!("Invalid gateway address: {e}"))?
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("Cannot connect to gateway: {e}"))?;

    let mut client = WorkerRegistryClient::new(channel);

    // Step 1: Register with challenge-response
    let worker_nonce = generate_nonce();
    let reg_request = RegisterRequest {
        sandbox_id: args.sandbox_id.clone(),
        protocol_version: Some(ProtocolVersion {
            major: 2,
            minor: 0,
            patch: 0,
        }),
        capabilities: Some(WorkerCapabilities {
            supported_operations: vec![
                "run_command".into(),
                "read_file".into(),
                "write_file".into(),
                "list_files".into(),
            ],
            max_concurrent_commands: 4,
            max_output_bytes: 10 * 1024 * 1024, // 10MB
            max_file_size_bytes: 100 * 1024 * 1024, // 100MB
            supports_streaming: true,
            supports_compression: true,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }),
        worker_nonce: worker_nonce.clone(),
        worker_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let reg_response = client.register(reg_request).await?.into_inner();

    let session_token = match reg_response.status() {
        register_response::Status::Accepted => {
            tracing::info!("Registration accepted directly");
            reg_response.session_token
        }
        register_response::Status::Challenge => {
            tracing::info!("Challenge received, computing HMAC proof");
            let proof = compute_hmac_proof(
                &args.secret,
                &worker_nonce,
                &reg_response.gateway_nonce,
            );
            let challenge_resp = client.challenge_response(ChallengeProof { proof })
                .await?
                .into_inner();

            if challenge_resp.status() != register_response::Status::Accepted {
                anyhow::bail!("Challenge response rejected: {:?}", challenge_resp.status());
            }
            tracing::info!("Challenge accepted");
            challenge_resp.session_token
        }
        register_response::Status::Rejected => {
            anyhow::bail!("Registration rejected by gateway");
        }
        register_response::Status::VersionMismatch => {
            anyhow::bail!("Protocol version mismatch");
        }
    };

    tracing::info!(sandbox_id = %args.sandbox_id, "Worker authenticated, opening command stream");

    // Step 2: Open bidirectional command stream
    let (cmd_tx, cmd_rx) = mpsc::channel(256);

    // Send ReadySignal first
    cmd_tx.send(WorkerMessage {
        command_id: String::new(),
        payload: Some(worker_message::Payload::Ready(ReadySignal {
            session_token: session_token.clone(),
            working_dir: args.workdir.clone(),
        })),
    }).await?;

    let response_stream = client
        .command_stream(ReceiverStream::new(cmd_rx))
        .await?
        .into_inner();

    // Step 3: Process commands from Gateway
    run_command_loop(response_stream, cmd_tx.clone(), &args.workdir).await
}

fn generate_nonce() -> Vec<u8> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut nonce = ts.to_le_bytes().to_vec();
    nonce.extend_from_slice(&rand_bytes());
    nonce
}

fn rand_bytes() -> [u8; 24] {
    // Simple pseudo-random for nonce (not crypto, just unique)
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut bytes = [0u8; 24];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((ts.wrapping_mul((i as u64) + 1)) >> (i % 8)) as u8;
    }
    bytes
}

fn compute_hmac_proof(secret: &str, worker_nonce: &[u8], gateway_nonce: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(worker_nonce);
    mac.update(gateway_nonce);
    mac.finalize().into_bytes().to_vec()
}

/// Validate that a path doesn't escape the allowed base directories.
/// Prevents path traversal attacks like "../../../etc/passwd"
fn validate_path(path: &str) -> Result<String> {
    // Canonical allowed prefixes
    const ALLOWED_PREFIXES: &[&str] = &[
        "/workspace",
        "/tmp",
        "/home",
        "/opt",
        "/var/tmp",
    ];

    // Reject obviously malicious paths
    if path.contains("..") {
        anyhow::bail!("Path traversal detected: '{}' contains '..'", path);
    }

    // Must be absolute
    if !path.starts_with('/') {
        anyhow::bail!("Relative paths not allowed: '{}'", path);
    }

    // Must be under an allowed prefix
    let allowed = ALLOWED_PREFIXES.iter()
        .any(|prefix| path.starts_with(prefix));

    if !allowed {
        anyhow::bail!("Path '{}' is outside allowed directories", path);
    }

    Ok(path.to_string())
}

async fn run_command_loop(
    mut stream: tonic::Streaming<GatewayCommand>,
    tx: mpsc::Sender<WorkerMessage>,
    _workdir: &str,
) -> Result<ExitReason> {
    use worker_message::Payload;
    use gateway_command::Payload as CmdPayload;

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_COMMANDS));

    while let Some(cmd) = stream.message().await? {
        let command_id = cmd.command_id.clone();

        match cmd.payload {
            Some(CmdPayload::Run(run_req)) => {
                let tx = tx.clone();
                let sem = semaphore.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();

                    tracing::info!(command_id = %command_id, command = %run_req.command, "Executing command");
                    ACTIVE_COMMANDS.fetch_add(1, Ordering::Relaxed);

                    // Send ACK
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.clone(),
                        payload: Some(Payload::Ack(CommandAck {
                            state: command_ack::State::Executing as i32,
                        })),
                    }).await;

                    let start = std::time::Instant::now();
                    let output = execute_command(&run_req).await;
                    let duration_ms = start.elapsed().as_millis() as i64;

                    ACTIVE_COMMANDS.fetch_sub(1, Ordering::Relaxed);

                    match output {
                        Ok((stdout, stderr, exit_code)) => {
                            if !stdout.is_empty() {
                                let _ = tx.send(WorkerMessage {
                                    command_id: command_id.clone(),
                                    payload: Some(Payload::Stdout(StdoutChunk {
                                        data: stdout,
                                        sequence: 0,
                                    })),
                                }).await;
                            }
                            if !stderr.is_empty() {
                                let _ = tx.send(WorkerMessage {
                                    command_id: command_id.clone(),
                                    payload: Some(Payload::Stderr(StderrChunk {
                                        data: stderr,
                                        sequence: 0,
                                    })),
                                }).await;
                            }
                            let _ = tx.send(WorkerMessage {
                                command_id: command_id.clone(),
                                payload: Some(Payload::Exit(ExitResult {
                                    exit_code,
                                    duration_ms,
                                    timed_out: false,
                                    signal: String::new(),
                                })),
                            }).await;
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerMessage {
                                command_id: command_id.clone(),
                                payload: Some(Payload::Error(ErrorResult {
                                    error: e.to_string(),
                                    error_kind: "internal".into(),
                                    errno: 0,
                                })),
                            }).await;
                        }
                    }
                });
            }
            Some(CmdPayload::Ping(ping)) => {
                let health = collect_health();
                let _ = tx.send(WorkerMessage {
                    command_id: command_id.clone(),
                    payload: Some(Payload::Pong(PongResponse {
                        ping_timestamp: ping.timestamp,
                        worker_timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64,
                        health: Some(health),
                    })),
                }).await;
            }
            Some(CmdPayload::Shutdown(shutdown)) => {
                tracing::info!(
                    reason = %shutdown.reason,
                    grace_level = ?shutdown.grace_level(),
                    "Shutdown requested"
                );

                let active = active_command_count();

                match shutdown.grace_level() {
                    shutdown_request::GraceLevel::Graceful => {
                        let _ = tx.send(WorkerMessage {
                            command_id: command_id.clone(),
                            payload: Some(Payload::ShutdownAck(ShutdownAck {
                                pending_commands: active,
                                will_drain: true,
                            })),
                        }).await;
                        return Ok(ExitReason::Shutdown);
                    }
                    shutdown_request::GraceLevel::Draining => {
                        let _ = tx.send(WorkerMessage {
                            command_id: command_id.clone(),
                            payload: Some(Payload::ShutdownAck(ShutdownAck {
                                pending_commands: active,
                                will_drain: true,
                            })),
                        }).await;
                        return Ok(ExitReason::Shutdown);
                    }
                    shutdown_request::GraceLevel::Forceful => {
                        let _ = tx.send(WorkerMessage {
                            command_id: command_id.clone(),
                            payload: Some(Payload::ShutdownAck(ShutdownAck {
                                pending_commands: active,
                                will_drain: false,
                            })),
                        }).await;
                        std::process::exit(0);
                    }
                }
            }
            Some(CmdPayload::Read(read_req)) => {
                let tx = tx.clone();
                let sem = semaphore.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    handle_read(read_req, &command_id, &tx).await;
                });
            }
            Some(CmdPayload::Write(write_req)) => {
                let tx = tx.clone();
                let sem = semaphore.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    handle_write(write_req, &command_id, &tx).await;
                });
            }
            Some(CmdPayload::List(list_req)) => {
                let tx = tx.clone();
                let sem = semaphore.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    handle_list(list_req, &command_id, &tx).await;
                });
            }
            Some(CmdPayload::Cancel(cancel_req)) => {
                tracing::info!(target = %cancel_req.target_command_id, "Cancel requested (not yet supported)");
                let _ = tx.send(WorkerMessage {
                    command_id: command_id.clone(),
                    payload: Some(Payload::CancelAck(CancelAck {
                        cancelled: false,
                        error: "Cancel not yet implemented".into(),
                    })),
                }).await;
            }
            None => {
                tracing::warn!("Received command with no payload");
            }
        }
    }

    tracing::info!("Command stream closed");
    Ok(ExitReason::StreamEnded)
}

async fn handle_read(read_req: ReadFileRequest, command_id: &str, tx: &mpsc::Sender<WorkerMessage>) {
    use worker_message::Payload;

    match validate_path(&read_req.path) {
        Ok(safe_path) => {
            match tokio::fs::metadata(&safe_path).await {
                Ok(metadata) => {
                    let file_size = metadata.len() as i64;
                    let chunk_size: usize = 4 * 1024 * 1024; // 4MB
                    let total_chunks = (file_size as usize).div_ceil(chunk_size);

                    match tokio::fs::File::open(&safe_path).await {
                        Ok(mut file) => {
                            use tokio::io::AsyncReadExt;
                            let mut buf = vec![0u8; chunk_size];
                            let mut chunk_index: i32 = 0;
                            let mut offset: i64 = 0;

                            loop {
                                match file.read(&mut buf).await {
                                    Ok(0) => break, // EOF
                                    Ok(n) => {
                                        let is_last = (offset + n as i64) >= file_size;
                                        let _ = tx.send(WorkerMessage {
                                            command_id: command_id.to_string(),
                                            payload: Some(Payload::FileChunk(FileChunk {
                                                content: buf[..n].to_vec(),
                                                offset,
                                                is_last,
                                                chunk_index,
                                                total_chunks: total_chunks as i32,
                                            })),
                                        }).await;
                                        offset += n as i64;
                                        chunk_index += 1;
                                    }
                                    Err(e) => {
                                        let _ = tx.send(WorkerMessage {
                                            command_id: command_id.to_string(),
                                            payload: Some(Payload::Error(ErrorResult {
                                                error: e.to_string(),
                                                error_kind: "internal".into(),
                                                errno: e.raw_os_error().unwrap_or(0),
                                            })),
                                        }).await;
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerMessage {
                                command_id: command_id.to_string(),
                                payload: Some(Payload::Error(ErrorResult {
                                    error: e.to_string(),
                                    error_kind: "not_found".into(),
                                    errno: e.raw_os_error().unwrap_or(0),
                                })),
                            }).await;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.to_string(),
                        payload: Some(Payload::Error(ErrorResult {
                            error: e.to_string(),
                            error_kind: "not_found".into(),
                            errno: e.raw_os_error().unwrap_or(0),
                        })),
                    }).await;
                }
            }
        }
        Err(e) => {
            let _ = tx.send(WorkerMessage {
                command_id: command_id.to_string(),
                payload: Some(Payload::Error(ErrorResult {
                    error: e.to_string(),
                    error_kind: "permission".into(),
                    errno: 0,
                })),
            }).await;
        }
    }
}

async fn handle_write(write_req: WriteFileRequest, command_id: &str, tx: &mpsc::Sender<WorkerMessage>) {
    use worker_message::Payload;

    match validate_path(&write_req.path) {
        Ok(safe_path) => {
            // TODO: Add multi-chunk write support
            // For now, single-chunk write (total_chunks should be 1)
            match tokio::fs::write(&safe_path, &write_req.content).await {
                Ok(()) => {
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.to_string(),
                        payload: Some(Payload::Ack(CommandAck {
                            state: command_ack::State::Received as i32,
                        })),
                    }).await;
                }
                Err(e) => {
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.to_string(),
                        payload: Some(Payload::Error(ErrorResult {
                            error: e.to_string(),
                            error_kind: "permission".into(),
                            errno: e.raw_os_error().unwrap_or(0),
                        })),
                    }).await;
                }
            }
        }
        Err(e) => {
            let _ = tx.send(WorkerMessage {
                command_id: command_id.to_string(),
                payload: Some(Payload::Error(ErrorResult {
                    error: e.to_string(),
                    error_kind: "permission".into(),
                    errno: 0,
                })),
            }).await;
        }
    }
}

async fn handle_list(list_req: ListFilesRequest, command_id: &str, tx: &mpsc::Sender<WorkerMessage>) {
    use worker_message::Payload;

    match validate_path(&list_req.directory) {
        Ok(safe_dir) => {
            match list_directory(&safe_dir).await {
                Ok(entries) => {
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.to_string(),
                        payload: Some(Payload::FileList(FileList { entries })),
                    }).await;
                }
                Err(e) => {
                    let _ = tx.send(WorkerMessage {
                        command_id: command_id.to_string(),
                        payload: Some(Payload::Error(ErrorResult {
                            error: e.to_string(),
                            error_kind: "not_found".into(),
                            errno: 0,
                        })),
                    }).await;
                }
            }
        }
        Err(e) => {
            let _ = tx.send(WorkerMessage {
                command_id: command_id.to_string(),
                payload: Some(Payload::Error(ErrorResult {
                    error: e.to_string(),
                    error_kind: "permission".into(),
                    errno: 0,
                })),
            }).await;
        }
    }
}

async fn execute_command(req: &RunCommandRequest) -> Result<(Vec<u8>, Vec<u8>, i32)> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(&req.command);

    if !req.working_dir.is_empty() {
        cmd.current_dir(&req.working_dir);
    }

    for (k, v) in &req.env {
        cmd.env(k, v);
    }

    let output = cmd.output().await
        .map_err(|e| anyhow::anyhow!("Failed to execute: {e}"))?;

    Ok((output.stdout, output.stderr, output.status.code().unwrap_or(-1)))
}

async fn list_directory(dir: &str) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(dir).await
        .map_err(|e| anyhow::anyhow!("Cannot read dir: {e}"))?;

    while let Some(entry) = read_dir.next_entry().await? {
        let metadata = entry.metadata().await?;
        entries.push(FileEntry {
            path: entry.file_name().to_string_lossy().to_string(),
            is_directory: metadata.is_dir(),
            size_bytes: metadata.len() as i64,
            permissions: format!("{:o}", metadata.permissions().mode() & 0o777),
            modified_epoch_ms: metadata.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            file_type: if metadata.is_dir() { "dir".into() } else { "file".into() },
        });
    }

    Ok(entries)
}

fn collect_health() -> HealthReport {
    let mut report = HealthReport {
        cpu_usage_percent: 0.0,
        memory_used_bytes: 0,
        memory_total_bytes: 0,
        disk_free_bytes: 0,
        uptime_seconds: 0,
        active_commands: 0,
        pending_commands: 0,
        load_average_1m: 0.0,
    };

    // Track active commands
    report.active_commands = active_command_count();

    // Memory info from /proc/meminfo (Linux only)
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if let Some(val) = line.strip_prefix("MemTotal:") {
                report.memory_total_bytes = parse_kb(val) * 1024;
            } else if let Some(val) = line.strip_prefix("MemAvailable:") {
                let available = parse_kb(val) * 1024;
                report.memory_used_bytes = report.memory_total_bytes.saturating_sub(available);
            }
        }
    }

    // Load average from /proc/loadavg
    if let Ok(loadavg) = std::fs::read_to_string("/proc/loadavg")
        && let Some(part) = loadavg.split_whitespace().next()
    {
        report.load_average_1m = part.parse().unwrap_or(0.0);
    }

    // Uptime from /proc/uptime
    if let Ok(uptime) = std::fs::read_to_string("/proc/uptime")
        && let Some(part) = uptime.split_whitespace().next()
    {
        report.uptime_seconds = part.parse().unwrap_or(0);
    }

    report
}

fn parse_kb(s: &str) -> i64 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
