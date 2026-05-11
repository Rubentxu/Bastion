//! Worker Registry Service - gRPC server for worker connections.
//!
//! Inspired by Jenkins JNLP: workers initiate OUTBOUND connections to the gateway.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use hmac::Mac;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use bastion_domain::execution::command::CommandResult;
use bastion_domain::execution::stream::{ChunkType, CommandChunk};
use bastion_domain::file_ops::FileEntry;
use bastion_domain::provider::port::CommandStream;
use bastion_domain::provider::router::CommandRouter;
use bastion_domain::shared::DomainError;

use crate::sandbox::v2::worker_registry_server::WorkerRegistry;
use crate::sandbox::v2::*;
use crate::server::AuthConfig;
use bastion_infrastructure::metrics::{HeartbeatBridge, WorkerResources};

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Pending challenge data stored when CHALLENGE is sent, used during challenge_response verification
#[derive(Clone)]
struct PendingChallenge {
    worker_nonce: Vec<u8>,
    gateway_nonce: Vec<u8>,
}

/// Verify HMAC proof from worker
fn verify_hmac_proof(
    secret: &str,
    worker_nonce: &[u8],
    gateway_nonce: &[u8],
    proof: &[u8],
) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(worker_nonce);
    mac.update(gateway_nonce);
    mac.verify_slice(proof).is_ok()
}

/// Verify HMAC proof against any of the configured pre-shared keys.
/// Returns true if the proof is valid for at least one configured key.
/// Used when `pre_shared_key_enabled = true`.
fn verify_hmac_proof_against_keys(
    pre_shared_keys: &[String],
    worker_nonce: &[u8],
    gateway_nonce: &[u8],
    proof: &[u8],
) -> bool {
    pre_shared_keys
        .iter()
        .any(|key| verify_hmac_proof(key, worker_nonce, gateway_nonce, proof))
}

/// Token bucket for rate limiting
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = (now - self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Handle to an active worker connection
#[derive(Clone)]
pub(crate) struct WorkerHandle {
    /// Channel to send commands to the worker
    cmd_tx: mpsc::Sender<GatewayCommand>,
    /// Session token for this worker
    session_token: String,
    /// Consecutive failures for circuit breaker
    consecutive_failures: Arc<AtomicU32>,
    /// Circuit open until timestamp
    circuit_open_until: Arc<Mutex<Option<Instant>>>,
}

/// Circuit breaker states for WorkerHandle
#[derive(PartialEq)]
enum CircuitState {
    Closed,   // Normal operation
    HalfOpen, // Testing with reduced load
    Open,     // Failing fast, no requests
}

impl WorkerHandle {
    fn get_circuit_state(&self) -> CircuitState {
        if let Ok(guard) = self.circuit_open_until.lock()
            && let Some(until) = *guard
        {
            if Instant::now() < until {
                return CircuitState::Open;
            }
            // Timeout expired - transition to HalfOpen
            return CircuitState::HalfOpen;
        }
        CircuitState::Closed
    }

    fn is_circuit_open(&self) -> bool {
        matches!(self.get_circuit_state(), CircuitState::Open)
    }

    fn record_success(&self) {
        let state = self.get_circuit_state();
        if state == CircuitState::HalfOpen {
            // Successful probe in HalfOpen state → close the circuit
            tracing::info!("Circuit breaker closing after successful probe");
        }
        self.consecutive_failures.store(0, Ordering::Relaxed);
        if let Ok(mut guard) = self.circuit_open_until.lock() {
            *guard = None;
        }
    }

    fn record_failure(&self) {
        let state = self.get_circuit_state();
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;

        match state {
            CircuitState::HalfOpen => {
                // Failure in HalfOpen → reopen circuit
                if let Ok(mut guard) = self.circuit_open_until.lock() {
                    *guard = Some(Instant::now() + Duration::from_secs(30));
                }
                tracing::warn!("Circuit breaker reopened after probe failure");
            }
            CircuitState::Closed if failures >= 3 => {
                // Open circuit for 30 seconds
                if let Ok(mut guard) = self.circuit_open_until.lock() {
                    *guard = Some(Instant::now() + Duration::from_secs(30));
                }
                tracing::warn!(
                    failures,
                    "Circuit breaker opened for 30s after {} consecutive failures",
                    failures
                );
            }
            _ => {
                // In Closed state but not yet at threshold - just increment
            }
        }
    }
}

/// Registry service that tracks all connected workers and routes commands to them.
#[derive(Clone)]
pub struct RegistryService {
    /// Active workers by sandbox_id
    workers: Arc<DashMap<String, WorkerHandle>>,
    /// Multi-response channels for collecting all messages per command, keyed by command_id
    pending_multi: Arc<DashMap<String, mpsc::Sender<Result<WorkerMessage, Status>>>>,
    /// Secrets for challenge-response auth, keyed by sandbox_id
    secrets: Arc<DashMap<String, String>>,
    /// Rate limiters per sandbox (token bucket)
    rate_limiters: Arc<DashMap<String, Mutex<TokenBucket>>>,
    /// Pending challenge data: stored when CHALLENGE is sent, keyed by sandbox_id
    pending_challenges: Arc<DashMap<String, PendingChallenge>>,
    /// JWT manager for session tokens
    #[allow(dead_code)]
    jwt_manager: crate::auth::JwtManager,
    /// AutoTLS instance for worker certificate generation
    #[allow(dead_code)]
    auto_tls: Arc<crate::auto_tls::AutoTls>,
    /// Authentication config (pre-shared key settings)
    auth_config: AuthConfig,
    /// Heartbeat bridge for per-sandbox resource usage tracking.
    /// Uses RwLock for interior mutability since RegistryService is Clone and may be
    /// used behind Arc. Set via set_heartbeat_bridge() after MetricsHub initialization.
    heartbeat_bridge:
        Arc<std::sync::RwLock<Option<Arc<HeartbeatBridge>>>>,
}

impl RegistryService {
    pub fn new(
        jwt_manager: crate::auth::JwtManager,
        auto_tls: Arc<crate::auto_tls::AutoTls>,
        auth_config: AuthConfig,
    ) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            pending_multi: Arc::new(DashMap::new()),
            secrets: Arc::new(DashMap::new()),
            rate_limiters: Arc::new(DashMap::new()),
            pending_challenges: Arc::new(DashMap::new()),
            jwt_manager,
            auto_tls,
            auth_config,
            heartbeat_bridge: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// Set the heartbeat bridge for per-sandbox resource tracking.
    /// This is called after MetricsHub is initialized in main.rs.
    pub fn set_heartbeat_bridge(self: &Arc<Self>, heartbeat_bridge: Arc<bastion_infrastructure::metrics::HeartbeatBridge>) {
        if let Ok(mut guard) = self.heartbeat_bridge.write() {
            *guard = Some(heartbeat_bridge);
        }
    }

    /// Update heartbeat bridge with worker resource data from a PongResponse.
    fn update_heartbeat_bridge(&self, sandbox_id: &str, health: &HealthReport) {
        let guard = match self.heartbeat_bridge.read() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(ref bridge) = *guard else {
            return;
        };

        // Convert bytes to MB for memory fields
        let mem_used_mb = health.memory_used_bytes as f64 / (1024.0 * 1024.0);
        let mem_limit_mb = health.memory_total_bytes as f64 / (1024.0 * 1024.0);

        // Note: HealthReport only has disk_free_bytes, not disk_total_bytes.
        // We can't compute disk_used_mb accurately, so set it to 0.
        // TODO: Extend the worker protocol to report disk_total_bytes so we can
        // compute disk_used_mb = disk_total - disk_free.
        let disk_used_mb = 0.0;

        let resources = WorkerResources {
            sandbox_id: sandbox_id.to_string(),
            cpu_percent: health.cpu_usage_percent,
            mem_used_mb,
            mem_limit_mb,
            disk_used_mb,
            loadavg_1m: health.load_average_1m,
            uptime_seconds: health.uptime_seconds as u64,
            last_heartbeat_epoch: chrono::Utc::now().timestamp(),
        };

        bridge.update_resources(resources);
    }

    /// Remove a sandbox's resource data from the heartbeat bridge.
    fn remove_heartbeat_bridge(&self, sandbox_id: &str) {
        let guard = match self.heartbeat_bridge.read() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(ref bridge) = *guard {
            bridge.remove_resources(sandbox_id);
        }
    }

    /// Start the watchdog background task to detect dead workers
    pub fn start_watchdog(self: &Arc<Self>, heartbeat_interval_ms: u64) {
        let registry = self.clone();
        let check_interval = Duration::from_millis(heartbeat_interval_ms * 3);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(check_interval);
            loop {
                interval.tick().await;

                // Check all workers — if the cmd_tx channel is closed, remove
                let dead_workers: Vec<String> = registry
                    .workers
                    .iter()
                    .filter(|entry| entry.cmd_tx.is_closed())
                    .map(|entry| entry.key().clone())
                    .collect();

                for id in dead_workers {
                    tracing::warn!(sandbox_id = %id, "Watchdog: removing dead worker");
                    // Remove from HeartbeatBridge
                    registry.remove_heartbeat_bridge(&id);
                    registry.workers.remove(&id);
                }
            }
        });
    }

    /// Register a worker's command sender handle
    pub fn register_worker(&self, sandbox_id: String, handle: WorkerHandle) {
        self.workers.insert(sandbox_id.clone(), handle);
        // Initialize rate limiter: 20 burst, 10 tokens/sec refill
        self.rate_limiters
            .insert(sandbox_id, Mutex::new(TokenBucket::new(20.0, 10.0)));
    }

    /// Remove a worker from the registry
    #[allow(dead_code)]
    pub fn unregister_worker(&self, sandbox_id: &str) {
        self.workers.remove(sandbox_id);
    }

    /// Get the session token for a worker
    #[allow(dead_code)]
    pub fn get_session_token(&self, sandbox_id: &str) -> Option<String> {
        self.workers
            .get(sandbox_id)
            .map(|h| h.session_token.clone())
    }

    /// Set secret for a sandbox (for challenge-response auth)
    pub fn set_secret(&self, sandbox_id: &str, secret: String) {
        self.secrets.insert(sandbox_id.to_string(), secret);
    }

    /// Get secret for a sandbox
    #[allow(dead_code)]
    pub fn get_secret(&self, sandbox_id: &str) -> Option<String> {
        self.secrets.get(sandbox_id).map(|s| s.clone())
    }

    /// Send a command and collect ALL response messages until ExitResult/Error/complete.
    ///
    /// This is used by CommandRouter implementations to get full command output
    /// (multiple stdout chunks, stderr chunks, exit code, etc.)
    pub async fn collect_responses(
        &self,
        sandbox_id: &str,
        command: GatewayCommand,
        timeout: Duration,
    ) -> Result<Vec<WorkerMessage>, RegistryError> {
        let handle = {
            let entry = self
                .workers
                .get(sandbox_id)
                .ok_or_else(|| RegistryError::WorkerNotFound(sandbox_id.to_string()))?;

            // Check circuit breaker
            if entry.is_circuit_open() {
                return Err(RegistryError::WorkerNotFound(format!(
                    "Circuit breaker open for {}",
                    sandbox_id
                )));
            }

            WorkerHandle {
                cmd_tx: entry.cmd_tx.clone(),
                session_token: entry.session_token.clone(),
                consecutive_failures: entry.consecutive_failures.clone(),
                circuit_open_until: entry.circuit_open_until.clone(),
            }
        };

        let command_id = command.command_id.clone();
        let sandbox_id = sandbox_id.to_string();

        // Extract command type for audit logging before moving command
        let command_type = format!(
            "{:?}",
            command
                .payload
                .as_ref()
                .map(|p| match p {
                    gateway_command::Payload::Run(_) => "run_command",
                    gateway_command::Payload::Read(_) => "read_file",
                    gateway_command::Payload::Write(_) => "write_file",
                    gateway_command::Payload::List(_) => "list_files",
                    gateway_command::Payload::Ping(_) => "ping",
                    gateway_command::Payload::Shutdown(_) => "shutdown",
                    gateway_command::Payload::Cancel(_) => "cancel",
                })
                .unwrap_or("unknown")
        );

        // Check rate limit before sending command
        if let Some(limiter) = self.rate_limiters.get(&sandbox_id)
            && let Ok(mut guard) = limiter.lock()
            && !guard.try_consume()
        {
            return Err(RegistryError::RateLimited(sandbox_id));
        }

        // Create multi-response channel
        let (tx, rx) = mpsc::channel::<Result<WorkerMessage, Status>>(16);
        self.pending_multi.insert(command_id.clone(), tx);

        // Send command
        if handle.cmd_tx.send(command).await.is_err() {
            handle.record_failure();
            self.pending_multi.remove(&command_id);
            return Err(RegistryError::WorkerDisconnected(sandbox_id));
        }

        // Collect responses until terminal message or timeout
        let result = tokio::time::timeout(timeout, async {
            let mut messages = Vec::new();
            let mut rx = rx;
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(msg) => {
                        // Determine if this is a terminal message
                        let is_terminal = match &msg.payload {
                            Some(worker_message::Payload::Exit(_)) => true,
                            Some(worker_message::Payload::Error(_)) => true,
                            Some(worker_message::Payload::FileList(_)) => true,
                            Some(worker_message::Payload::FileChunk(c)) => c.is_last,
                            _ => false,
                        };
                        messages.push(msg);
                        if is_terminal {
                            break;
                        }
                    }
                    Err(e) => {
                        return Err(RegistryError::CommandFailed(e.to_string()));
                    }
                }
            }
            Ok(messages)
        })
        .await;

        // Cleanup
        self.pending_multi.remove(&command_id);

        match result {
            Ok(Ok(messages)) => {
                handle.record_success();

                // Audit trail logging
                let exit_code = match messages.last() {
                    Some(WorkerMessage {
                        payload: Some(worker_message::Payload::Exit(e)),
                        ..
                    }) => e.exit_code.to_string(),
                    Some(WorkerMessage {
                        payload: Some(worker_message::Payload::Error(err)),
                        ..
                    }) => format!("error: {}", err.error),
                    _ => "unknown".to_string(),
                };

                tracing::info!(
                    audit = true,
                    sandbox_id = %sandbox_id,
                    command_id = %command_id,
                    command_type = %command_type,
                    exit_code = %exit_code,
                    response_count = messages.len(),
                    "Command completed (audit)"
                );

                Ok(messages)
            }
            Ok(Err(e)) => {
                handle.record_failure();
                Err(e)
            }
            Err(_) => {
                handle.record_failure();
                Err(RegistryError::CommandTimeout(command_id))
            }
        }
    }
}

impl std::fmt::Debug for RegistryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryService")
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum RegistryError {
    WorkerNotFound(String),
    WorkerDisconnected(String),
    CommandTimeout(String),
    CommandFailed(String),
    RateLimited(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::WorkerNotFound(id) => write!(f, "Worker {} not found", id),
            RegistryError::WorkerDisconnected(id) => write!(f, "Worker {} disconnected", id),
            RegistryError::CommandTimeout(id) => write!(f, "Command {} timed out", id),
            RegistryError::CommandFailed(e) => write!(f, "Command failed: {}", e),
            RegistryError::RateLimited(id) => write!(f, "Rate limited for sandbox {}", id),
        }
    }
}

impl std::error::Error for RegistryError {}

#[async_trait]
impl CommandRouter for RegistryService {
    async fn route_run_command(
        &self,
        sandbox_id: &str,
        command: &str,
        args: &[String],
        working_dir: &str,
        env: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<CommandResult, DomainError> {
        let command_id = Uuid::new_v4().to_string();

        let cmd = GatewayCommand {
            command_id: command_id.clone(),
            session_token: String::new(), // Will be filled from handle
            payload: Some(gateway_command::Payload::Run(RunCommandRequest {
                command: command.to_string(),
                args: args.to_vec(),
                working_dir: working_dir.to_string(),
                env: env.clone(),
                timeout_ms: timeout_ms as i64,
                persistent: false,
                session_id: String::new(),
            })),
        };

        // Collect ALL messages for this command_id
        let responses = self
            .collect_responses(sandbox_id, cmd, Duration::from_millis(timeout_ms))
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        // Aggregate responses into CommandResult
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = -1i32;
        let mut duration_ms = 0u64;
        let mut timed_out = false;

        for msg in responses {
            match msg.payload {
                Some(worker_message::Payload::Stdout(chunk)) => {
                    stdout.extend_from_slice(&chunk.data)
                }
                Some(worker_message::Payload::Stderr(chunk)) => {
                    stderr.extend_from_slice(&chunk.data)
                }
                Some(worker_message::Payload::Exit(result)) => {
                    exit_code = result.exit_code;
                    duration_ms = result.duration_ms as u64;
                    timed_out = result.timed_out;
                }
                Some(worker_message::Payload::Error(err)) => {
                    return Err(DomainError::Internal(err.error));
                }
                _ => {}
            }
        }

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
            duration_ms,
            timed_out,
        })
    }

    async fn route_run_command_stream(
        &self,
        sandbox_id: &str,
        command: &str,
        args: &[String],
        working_dir: &str,
        env: &HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<CommandStream, DomainError> {
        let command_id = Uuid::new_v4().to_string();

        let cmd = GatewayCommand {
            command_id: command_id.clone(),
            session_token: String::new(),
            payload: Some(gateway_command::Payload::Run(RunCommandRequest {
                command: command.to_string(),
                args: args.to_vec(),
                working_dir: working_dir.to_string(),
                env: env.clone(),
                timeout_ms: timeout_ms as i64,
                persistent: false,
                session_id: String::new(),
            })),
        };

        let handle = {
            let entry = self.workers.get(sandbox_id).ok_or_else(|| {
                DomainError::Internal(format!("Worker not found: {}", sandbox_id))
            })?;

            if entry.is_circuit_open() {
                return Err(DomainError::Internal(format!(
                    "Circuit breaker open for {}",
                    sandbox_id
                )));
            }

            WorkerHandle {
                cmd_tx: entry.cmd_tx.clone(),
                session_token: entry.session_token.clone(),
                consecutive_failures: entry.consecutive_failures.clone(),
                circuit_open_until: entry.circuit_open_until.clone(),
            }
        };

        // Check rate limit
        if let Some(limiter) = self.rate_limiters.get(sandbox_id)
            && let Ok(mut guard) = limiter.lock()
            && !guard.try_consume()
        {
            return Err(DomainError::Internal(format!(
                "Rate limited for {}",
                sandbox_id
            )));
        }

        // Create channel for streaming output chunks
        let (chunk_tx, chunk_rx) = mpsc::channel::<Result<CommandChunk, DomainError>>(64);

        // Create response channel registered in pending_multi
        let (resp_tx, mut resp_rx) = mpsc::channel::<Result<WorkerMessage, Status>>(16);
        self.pending_multi.insert(command_id.clone(), resp_tx);

        // Send command to worker
        if handle.cmd_tx.send(cmd).await.is_err() {
            handle.record_failure();
            self.pending_multi.remove(&command_id);
            return Err(DomainError::Internal(format!(
                "Worker disconnected: {}",
                sandbox_id
            )));
        }

        let pending_multi = self.pending_multi.clone();
        let cmd_id = command_id.clone();

        // Spawn task to forward worker responses as CommandChunks
        tokio::spawn(async move {
            while let Some(result) = resp_rx.recv().await {
                match result {
                    Ok(msg) => match msg.payload {
                        Some(worker_message::Payload::Stdout(chunk)) => {
                            if chunk_tx
                                .send(Ok(CommandChunk::stdout(chunk.data)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Some(worker_message::Payload::Stderr(chunk)) => {
                            if chunk_tx
                                .send(Ok(CommandChunk::stderr(chunk.data)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Some(worker_message::Payload::Exit(result)) => {
                            let exit_bytes = result.exit_code.to_le_bytes().to_vec();
                            let _ = chunk_tx
                                .send(Ok(CommandChunk {
                                    chunk_type: ChunkType::ExitCode,
                                    data: exit_bytes,
                                    is_final: true,
                                }))
                                .await;
                            break;
                        }
                        Some(worker_message::Payload::Error(err)) => {
                            let _ = chunk_tx.send(Err(DomainError::Internal(err.error))).await;
                            break;
                        }
                        _ => {}
                    },
                    Err(e) => {
                        let _ = chunk_tx
                            .send(Err(DomainError::Internal(e.to_string())))
                            .await;
                        break;
                    }
                }
            }
            // Cleanup the pending_multi entry
            pending_multi.remove(&cmd_id);
        });

        let stream: CommandStream = Box::pin(ReceiverStream::new(chunk_rx));
        Ok(stream)
    }

    async fn route_write_file(
        &self,
        sandbox_id: &str,
        path: &str,
        content: &[u8],
    ) -> Result<(), DomainError> {
        let command_id = Uuid::new_v4().to_string();

        let cmd = GatewayCommand {
            command_id: command_id.clone(),
            session_token: String::new(),
            payload: Some(gateway_command::Payload::Write(WriteFileRequest {
                path: path.to_string(),
                mode: 0o644,
                total_size: content.len() as i64,
                chunk_index: 0,
                total_chunks: 1,
                content: content.to_vec(),
            })),
        };

        let responses = self
            .collect_responses(sandbox_id, cmd, Duration::from_secs(30))
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        // Check for errors
        for msg in responses {
            if let Some(worker_message::Payload::Error(err)) = msg.payload {
                return Err(DomainError::Internal(err.error));
            }
        }

        Ok(())
    }

    async fn route_read_file(&self, sandbox_id: &str, path: &str) -> Result<Vec<u8>, DomainError> {
        let command_id = Uuid::new_v4().to_string();

        let cmd = GatewayCommand {
            command_id: command_id.clone(),
            session_token: String::new(),
            payload: Some(gateway_command::Payload::Read(ReadFileRequest {
                path: path.to_string(),
                offset: 0,
                length: -1,
            })),
        };

        let responses = self
            .collect_responses(sandbox_id, cmd, Duration::from_secs(30))
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        // Aggregate file chunks
        let mut content = Vec::new();
        for msg in responses {
            match msg.payload {
                Some(worker_message::Payload::FileChunk(chunk)) => {
                    content.extend_from_slice(&chunk.content);
                }
                Some(worker_message::Payload::Error(err)) => {
                    return Err(DomainError::Internal(err.error));
                }
                _ => {}
            }
        }

        Ok(content)
    }

    async fn route_list_files(
        &self,
        sandbox_id: &str,
        directory: &str,
    ) -> Result<Vec<FileEntry>, DomainError> {
        let command_id = Uuid::new_v4().to_string();

        let cmd = GatewayCommand {
            command_id: command_id.clone(),
            session_token: String::new(),
            payload: Some(gateway_command::Payload::List(ListFilesRequest {
                directory: directory.to_string(),
                recursive: false,
                max_depth: 1,
            })),
        };

        let responses = self
            .collect_responses(sandbox_id, cmd, Duration::from_secs(30))
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        // Extract file list
        for msg in responses {
            match msg.payload {
                Some(worker_message::Payload::FileList(list)) => {
                    return Ok(list
                        .entries
                        .into_iter()
                        .map(|e| {
                            let modified_at = if e.modified_epoch_ms > 0 {
                                chrono::DateTime::from_timestamp_millis(e.modified_epoch_ms)
                            } else {
                                None
                            };
                            FileEntry {
                                path: e.path,
                                is_directory: e.is_directory,
                                size_bytes: e.size_bytes as u64,
                                permissions: e.permissions,
                                modified_at,
                            }
                        })
                        .collect());
                }
                Some(worker_message::Payload::Error(err)) => {
                    return Err(DomainError::Internal(err.error));
                }
                _ => {}
            }
        }

        Ok(vec![])
    }

    fn set_sandbox_secret(&self, sandbox_id: &str, secret: &str) {
        self.set_secret(sandbox_id, secret.to_string());
    }

    fn is_worker_connected(&self, sandbox_id: &str) -> bool {
        self.workers.contains_key(sandbox_id)
    }
}

#[tonic::async_trait]
impl WorkerRegistry for RegistryService {
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();
        tracing::info!(sandbox_id = %req.sandbox_id, "Worker registration request");

        // Store the secret for later challenge verification
        // In production: secrets should be pre-provisioned and validated against a DB
        let secret = self.get_secret(&req.sandbox_id).unwrap_or_default();

        // For MVP: if no secret is set, auto-accept
        // In production: require pre-provisioned secrets
        if secret.is_empty() {
            tracing::warn!(sandbox_id = %req.sandbox_id, "No secret configured, auto-accepting (MVP mode)");
            return Ok(Response::new(RegisterResponse {
                status: i32::from(register_response::Status::Accepted),
                gateway_nonce: Vec::new(),
                session_token: format!("token-{}", req.sandbox_id),
                negotiated_version: None,
                heartbeat_interval_ms: 10000,
                command_timeout_ms: 30000,
                session_expiry_ms: 3600000,
                gateway_version: env!("CARGO_PKG_VERSION").to_string(),
            }));
        }

        // Challenge mode: generate gateway nonce and store both nonces for later verification
        let gateway_nonce = generate_nonce();
        self.pending_challenges.insert(
            req.sandbox_id.clone(),
            PendingChallenge {
                worker_nonce: req.worker_nonce,
                gateway_nonce: gateway_nonce.clone(),
            },
        );

        Ok(Response::new(RegisterResponse {
            status: i32::from(register_response::Status::Challenge),
            gateway_nonce,
            session_token: String::new(),
            negotiated_version: None,
            heartbeat_interval_ms: 10000,
            command_timeout_ms: 30000,
            session_expiry_ms: 3600000,
            gateway_version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn challenge_response(
        &self,
        request: Request<ChallengeProof>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();
        let sandbox_id = &req.sandbox_id;

        tracing::info!(sandbox_id = %sandbox_id, "Challenge response received");

        // Look up pending challenge data
        let pending = match self.pending_challenges.remove(sandbox_id) {
            Some((_, p)) => p,
            None => {
                tracing::warn!(sandbox_id = %sandbox_id, "No pending challenge found for sandbox");
                return Ok(Response::new(RegisterResponse {
                    status: i32::from(register_response::Status::Rejected),
                    gateway_nonce: Vec::new(),
                    session_token: String::new(),
                    negotiated_version: None,
                    heartbeat_interval_ms: 10000,
                    command_timeout_ms: 30000,
                    session_expiry_ms: 3600000,
                    gateway_version: env!("CARGO_PKG_VERSION").to_string(),
                }));
            }
        };

        // Verify HMAC proof using pre-shared keys (if enabled) or per-sandbox secret (legacy)
        let hmac_valid = if self.auth_config.pre_shared_key_enabled {
            // Pre-shared key mode: verify against any configured key
            if self.auth_config.pre_shared_keys.is_empty() {
                tracing::warn!(sandbox_id = %sandbox_id, "Pre-shared key mode enabled but no keys configured");
                false
            } else {
                verify_hmac_proof_against_keys(
                    &self.auth_config.pre_shared_keys,
                    &pending.worker_nonce,
                    &pending.gateway_nonce,
                    &req.proof,
                )
            }
        } else {
            // Legacy mode: verify against per-sandbox secret
            match self.get_secret(sandbox_id) {
                Some(secret) => verify_hmac_proof(
                    &secret,
                    &pending.worker_nonce,
                    &pending.gateway_nonce,
                    &req.proof,
                ),
                None => {
                    tracing::warn!(sandbox_id = %sandbox_id, "No secret found for sandbox");
                    false
                }
            }
        };

        if !hmac_valid {
            tracing::warn!(sandbox_id = %sandbox_id, "HMAC verification failed");
            return Ok(Response::new(RegisterResponse {
                status: i32::from(register_response::Status::Rejected),
                gateway_nonce: Vec::new(),
                session_token: String::new(),
                negotiated_version: None,
                heartbeat_interval_ms: 10000,
                command_timeout_ms: 30000,
                session_expiry_ms: 3600000,
                gateway_version: env!("CARGO_PKG_VERSION").to_string(),
            }));
        }

        tracing::info!(sandbox_id = %sandbox_id, "HMAC verification succeeded, issuing session token");

        Ok(Response::new(RegisterResponse {
            status: i32::from(register_response::Status::Accepted),
            gateway_nonce: Vec::new(),
            session_token: Uuid::new_v4().to_string(),
            negotiated_version: None,
            heartbeat_interval_ms: 10000,
            command_timeout_ms: 30000,
            session_expiry_ms: 3600000,
            gateway_version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    type CommandStreamStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<GatewayCommand, Status>> + Send>>;

    async fn command_stream(
        &self,
        request: Request<Streaming<WorkerMessage>>,
    ) -> Result<Response<Self::CommandStreamStream>, Status> {
        let mut in_stream = request.into_inner();

        // First message should be ReadySignal
        let first_msg = in_stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("Empty stream"))?
            .map_err(|e| Status::internal(format!("Stream error: {}", e)))?;

        let (ready_sandbox_id, session_token) = match first_msg.payload {
            Some(worker_message::Payload::Ready(ready)) => {
                tracing::info!(sandbox_id = %ready.session_token, "Worker ready signal received");
                (ready.session_token.clone(), ready.session_token)
            }
            _ => {
                return Err(Status::invalid_argument(
                    "Expected ReadySignal as first message",
                ));
            }
        };

        // Create channels for this worker
        let (cmd_tx, cmd_rx) = mpsc::channel::<GatewayCommand>(256);

        // Create worker handle with circuit breaker support
        let handle = WorkerHandle {
            cmd_tx: cmd_tx.clone(),
            session_token: session_token.clone(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            circuit_open_until: Arc::new(Mutex::new(None)),
        };
        self.register_worker(ready_sandbox_id.clone(), handle);

        // Spawn task to handle worker messages - route to pending_multi
        // and update HeartbeatBridge when PongResponse with HealthReport arrives
        let pending_multi = self.pending_multi.clone();
        let registry_for_hb = self.clone();
        let ready_sandbox_id_clone = ready_sandbox_id.clone();
        tokio::spawn(async move {
            while let Some(msg_result) = in_stream.next().await {
                match msg_result {
                    Ok(msg) => {
                        // Route to multi-response channel if applicable
                        if !msg.command_id.is_empty()
                            && let Some(sender) = pending_multi.get(&msg.command_id)
                        {
                            let _ = sender.send(Ok(msg.clone())).await;
                        }

                        // Update HeartbeatBridge if this is a PongResponse with HealthReport
                        if let Some(worker_message::Payload::Pong(pong)) = &msg.payload {
                            if let Some(ref health) = pong.health {
                                registry_for_hb.update_heartbeat_bridge(
                                    &ready_sandbox_id_clone,
                                    health,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Worker stream error: {}", e);
                        break;
                    }
                }
            }
            // Remove sandbox from HeartbeatBridge when worker stream ends
            registry_for_hb.remove_heartbeat_bridge(&ready_sandbox_id_clone);
            tracing::info!("Worker stream ended");
        });

        // Get heartbeat interval from registry (default 10s)
        let heartbeat_interval_ms = 10000u64;
        let heartbeat_cmd_tx = cmd_tx.clone();
        let sandbox_id_for_hb = ready_sandbox_id.clone();

        // Spawn heartbeat task
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval_ms));
            loop {
                interval.tick().await;
                let ping = GatewayCommand {
                    command_id: uuid::Uuid::new_v4().to_string(),
                    session_token: String::new(),
                    payload: Some(gateway_command::Payload::Ping(PingRequest {
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64,
                    })),
                };
                if heartbeat_cmd_tx.send(ping).await.is_err() {
                    tracing::info!(sandbox_id = %sandbox_id_for_hb, "Heartbeat: worker disconnected");
                    break;
                }
            }
        });

        // Return command stream
        let out_stream = ReceiverStream::new(cmd_rx).map(Ok);

        Ok(Response::new(Box::pin(out_stream)))
    }
}

fn generate_nonce() -> Vec<u8> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut nonce = ts.to_le_bytes().to_vec();
    // Add some pseudo-random bytes
    for i in 0..8 {
        nonce.push(((ts >> (i * 8)) & 0xff) as u8);
    }
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: compute HMAC-SHA256 proof for testing
    fn compute_hmac(secret: &str, worker_nonce: &[u8], gateway_nonce: &[u8]) -> Vec<u8> {
        use hmac::Mac;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(worker_nonce);
        mac.update(gateway_nonce);
        mac.finalize().into_bytes().to_vec()
    }

    #[test]
    fn test_verify_hmac_proof_valid() {
        let secret = "test-secret-key";
        let worker_nonce = b"worker-nonce-123";
        let gateway_nonce = b"gateway-nonce-456";
        let proof = compute_hmac(secret, worker_nonce, gateway_nonce);

        assert!(verify_hmac_proof(
            secret,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_invalid() {
        let secret = "test-secret-key";
        let worker_nonce = b"worker-nonce-123";
        let gateway_nonce = b"gateway-nonce-456";
        let wrong_proof = b"invalid-proof-data".to_vec();

        assert!(!verify_hmac_proof(
            secret,
            worker_nonce,
            gateway_nonce,
            &wrong_proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_wrong_secret() {
        let secret = "correct-secret";
        let wrong_secret = "wrong-secret";
        let worker_nonce = b"worker-nonce-123";
        let gateway_nonce = b"gateway-nonce-456";
        let proof = compute_hmac(secret, worker_nonce, gateway_nonce);

        assert!(!verify_hmac_proof(
            wrong_secret,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_against_keys_known_key_accepted() {
        // GIVEN pre-shared keys ["key1", "key2", "key3"]
        let pre_shared_keys = vec!["key1".to_string(), "key2".to_string(), "key3".to_string()];
        let worker_nonce = b"worker-nonce";
        let gateway_nonce = b"gateway-nonce";

        // WHEN worker presents proof made with "key2"
        let proof = compute_hmac("key2", worker_nonce, gateway_nonce);

        // THEN verification succeeds
        assert!(verify_hmac_proof_against_keys(
            &pre_shared_keys,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_against_keys_unknown_key_rejected() {
        // GIVEN pre-shared keys ["key1", "key2"]
        let pre_shared_keys = vec!["key1".to_string(), "key2".to_string()];
        let worker_nonce = b"worker-nonce";
        let gateway_nonce = b"gateway-nonce";

        // WHEN worker presents proof made with "unknown-key"
        let proof = compute_hmac("unknown-key", worker_nonce, gateway_nonce);

        // THEN verification fails
        assert!(!verify_hmac_proof_against_keys(
            &pre_shared_keys,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_against_keys_empty_keys_rejected() {
        // GIVEN no pre-shared keys configured
        let pre_shared_keys: Vec<String> = vec![];
        let worker_nonce = b"worker-nonce";
        let gateway_nonce = b"gateway-nonce";
        let proof = compute_hmac("any-key", worker_nonce, gateway_nonce);

        // THEN verification fails (no keys to validate against)
        assert!(!verify_hmac_proof_against_keys(
            &pre_shared_keys,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_verify_hmac_proof_against_keys_first_key_matched() {
        // GIVEN pre-shared keys ["key1", "key2", "key3"]
        let pre_shared_keys = vec!["key1".to_string(), "key2".to_string(), "key3".to_string()];
        let worker_nonce = b"worker-nonce";
        let gateway_nonce = b"gateway-nonce";

        // WHEN worker presents proof made with "key1" (first key)
        let proof = compute_hmac("key1", worker_nonce, gateway_nonce);

        // THEN verification succeeds
        assert!(verify_hmac_proof_against_keys(
            &pre_shared_keys,
            worker_nonce,
            gateway_nonce,
            &proof
        ));
    }

    #[test]
    fn test_auth_config_default_disabled() {
        let auth_config = AuthConfig::default();
        assert!(!auth_config.pre_shared_key_enabled);
        assert!(auth_config.pre_shared_keys.is_empty());
    }

    #[test]
    fn test_auth_config_with_pre_shared_keys() {
        let auth_config = AuthConfig {
            pre_shared_key_enabled: true,
            pre_shared_keys: vec!["my-secret-key".to_string(), "another-key".to_string()],
        };
        assert!(auth_config.pre_shared_key_enabled);
        assert_eq!(auth_config.pre_shared_keys.len(), 2);
        assert_eq!(auth_config.pre_shared_keys[0], "my-secret-key");
    }
}

// Re-export for convenience
pub use crate::sandbox::v2::worker_registry_server::WorkerRegistryServer;
