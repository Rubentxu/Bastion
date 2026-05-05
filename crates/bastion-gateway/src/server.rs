//! MCP Server handler for Bastion Gateway.
//!
//! Implements the rmcp ServerHandler with sandbox tools.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use futures::StreamExt;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::RequestContext;
use rmcp::{schemars, tool, tool_handler, tool_router, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::RwLock;
use dashmap::DashMap;

use bastion_application::execution::{RunCommandStreamUseCase, RunCommandUseCase};
use bastion_application::file_ops::{ListFilesUseCase, ReadFileUseCase, WriteFileUseCase};
use bastion_application::sandbox::{
    CreateSandboxUseCase, GetSandboxInfoUseCase, ListSandboxesUseCase, TerminateSandboxUseCase,
};
use bastion_domain::execution::command::CommandSpec;
use bastion_domain::execution::stream::ChunkType;
use bastion_domain::catalog::experience::ExperienceRecord;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::secret::{parse_secret_ref, SecretResolver, SecretSource};
use bastion_domain::shared::{DomainError, id::SandboxId};
use bastion_domain::template::{
    ArtifactCatalog, CapabilityDescriptor, MaterializationMode, ProviderMaterializer,
    TemplateArtifact, ToolDescriptor, ToolchainRequest, ToolchainStrategy, ToolResolver,
};
use bastion_infrastructure::catalog::toml_advice_parser::{AdviceConfigStore, AdviceRegistry};
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::pool::SandboxPoolManager;
use bastion_infrastructure::template::{
    AptAdapter, AsdfAdapter, CapabilityRegistry, SdkmanAdapter, FsArtifactStore,
    PodmanOptimizedMaterializer, SnapshotManager,
};
/// Sync backend selection for sandbox file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncBackend {
    /// Use tar piped via podman exec (most compatible)
    Tar,
    /// Use rsync (fastest for large trees, requires rsync in sandbox)
    Rsync,
    /// Use podman cp (simplest, but limited)
    PodmanCp,
    /// Auto-detect best backend
    Auto,
}

/// Configuration for catalog-related components (experience, assertions, doctors, advice).
/// Wraps multiple optional catalog registries into a single config struct to reduce
/// the argument count in `BastionGateway::new()`.
#[derive(Clone, Default)]
pub struct CatalogConfig {
    /// Optional experience store for catalog recording (Phase 2)
    pub experience_store: Option<Arc<dyn bastion_domain::catalog::experience::ExperienceStore>>,
    /// Optional assertion registry loaded from TOML (Phase 3)
    pub assertion_registry: Option<Arc<bastion_infrastructure::catalog::toml_assertion_parser::AssertionRegistry>>,
    /// Optional doctor registry loaded from TOML (Phase 4)
    pub doctor_registry: Option<Arc<bastion_infrastructure::catalog::toml_doctor_parser::DoctorRegistry>>,
    /// Optional advice registry loaded from TOML (advice-catalog-engine)
    pub advice_registry: Option<Arc<AdviceRegistry>>,
    /// Optional advice config store (`.bastion/advice.toml`)
    pub advice_config: Option<Arc<AdviceConfigStore>>,
}

impl CatalogConfig {
    /// Create a new CatalogConfig with all fields set to None.
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Configuration for gateway operational settings (pool, metrics, TLS).
/// Groups operational/runtime config to reduce argument count in `BastionGateway::new()`.
#[derive(Clone)]
pub struct GatewayConfig {
    /// Optional sandbox pool manager
    pub pool_manager: Option<Arc<SandboxPoolManager>>,
    /// Gateway metrics collector
    pub metrics: GatewayMetrics,
    /// AutoTLS manager for mTLS
    #[allow(dead_code)]
    pub auto_tls: Arc<crate::auto_tls::AutoTls>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            pool_manager: None,
            metrics: GatewayMetrics::default(),
            auto_tls: Arc::new(crate::auto_tls::get_auto_tls().clone()),
        }
    }
}

/// Bastion MCP Gateway server.
///
/// Exposes sandbox management tools to AI agents via MCP protocol.
#[derive(Clone)]
pub struct BastionGateway {
    pub(crate) provider: Arc<dyn SandboxProvider>,
    /// All registered providers (keyed by name: "podman", "firecracker", "gvisor")
    providers: Arc<std::collections::HashMap<String, Arc<dyn SandboxProvider>>>,
    repository: Arc<dyn SandboxRepository>,
    secret_resolver: Arc<dyn SecretResolver>,
    /// Gateway operational config (pool, metrics, TLS)
    pub(crate) gateway_config: GatewayConfig,
    artifact_catalog: Arc<RwLock<ArtifactCatalog>>,
    artifact_store: Arc<FsArtifactStore>,
    /// Prepared environments keyed by env_ref
    prepared_environments: Arc<RwLock<std::collections::HashMap<String, std::collections::HashMap<String, String>>>>,
    /// Tracks the last env_ref per sandbox for auto-injection in sandbox_run
    last_env_ref: Arc<RwLock<std::collections::HashMap<String, String>>>,
    /// Sync backend preference
    sync_backend: SyncBackend,
    /// Rate limiter for MCP tool calls: global limit (legacy fallback)
    rate_limiter: Arc<Mutex<RateLimiter>>,
    /// Per-client rate limiter: each client gets its own token bucket
    per_client_rate_limiter: Arc<PerClientRateLimiter>,
    /// TOML-driven capability registry (tried before ToolResolver)
    capability_registry: Arc<RwLock<CapabilityRegistry>>,
    /// Cancel tokens for running commands: sandbox_id → cancel flag
    cancel_tokens: Arc<DashMap<String, Arc<AtomicBool>>>,
    /// Catalog configuration (experience, assertions, doctors, advice)
    pub(crate) catalog_config: CatalogConfig,
}

/// Simple token bucket rate limiter for MCP layer
struct RateLimiter {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: std::time::Instant,
}

impl RateLimiter {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: std::time::Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = std::time::Instant::now();
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

/// Per-client rate limiter using DashMap for concurrent access.
///
/// Each client (identified by session/connection key) gets its own token bucket.
/// Inactive clients are pruned after `prune_interval` to prevent memory growth.
struct PerClientRateLimiter {
    buckets: DashMap<String, Mutex<RateLimiter>>,
    max_tokens: f64,
    refill_rate: f64,
    /// Maximum number of clients tracked before eviction of least-recently-used
    max_clients: usize,
}

impl PerClientRateLimiter {
    fn new(max_tokens: f64, refill_rate: f64, max_clients: usize) -> Self {
        Self {
            buckets: DashMap::new(),
            max_tokens,
            refill_rate,
            max_clients,
        }
    }

    /// Try to consume a token for the given client key.
    /// Returns true if the request is allowed, false if rate-limited.
    fn try_consume(&self, client_key: &str) -> bool {
        // Evict oldest entries if we have too many clients
        if self.buckets.len() >= self.max_clients {
            // DashMap doesn't have ordered eviction, so we just remove a random entry
            if let Some(entry) = self.buckets.iter().next() {
                self.buckets.remove(entry.key());
            }
        }

        self.buckets
            .entry(client_key.to_string())
            .or_insert_with(|| Mutex::new(RateLimiter::new(self.max_tokens, self.refill_rate)))
            .value()
            .lock()
            .unwrap()
            .try_consume()
    }
}

impl BastionGateway {
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        providers: std::collections::HashMap<String, Arc<dyn SandboxProvider>>,
        repository: Arc<dyn SandboxRepository>,
        secret_resolver: Arc<dyn SecretResolver>,
        gateway_config: GatewayConfig,
        capability_registry: CapabilityRegistry,
        catalog_config: CatalogConfig,
    ) -> Self {
        Self {
            provider,
            providers: Arc::new(providers),
            repository,
            secret_resolver,
            gateway_config,
            artifact_catalog: Arc::new(RwLock::new(ArtifactCatalog::new())),
            artifact_store: Arc::new(FsArtifactStore::new(PathBuf::from("/tmp/bastion-artifacts"))),
            prepared_environments: Arc::new(RwLock::new(std::collections::HashMap::new())),
            last_env_ref: Arc::new(RwLock::new(std::collections::HashMap::new())),
            sync_backend: SyncBackend::Auto,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(500.0, 100.0))),
            per_client_rate_limiter: Arc::new(PerClientRateLimiter::new(100.0, 20.0, 1000)),
            capability_registry: Arc::new(RwLock::new(capability_registry)),
            cancel_tokens: Arc::new(DashMap::new()),
            catalog_config,
        }
    }

    /// Check rate limit (global + per-client) and return error response if exceeded.
    /// Uses per-client limiting with a global fallback.
    fn check_per_client_rate_limit(&self, client_key: &str) -> Option<String> {
        // Per-client rate limit
        if !self.per_client_rate_limiter.try_consume(client_key) {
            return Some(serde_json::json!({
                "error": "Per-client rate limit exceeded",
                "hint": "This client is sending requests too fast. Slow down.",
                "client": client_key
            }).to_string());
        }

        // Global rate limit (backstop)
        let mut limiter = self.rate_limiter.lock().unwrap();
        if limiter.try_consume() {
            None // OK
        } else {
            Some(serde_json::json!({
                "error": "Global rate limit exceeded",
                "hint": "The gateway is receiving too many requests overall. Reduce request frequency."
            }).to_string())
        }
    }

    /// Record an experience if the store is configured.
    async fn record_experience(&self, record: ExperienceRecord) {
        let Some(ref store) = self.catalog_config.experience_store else {
            return;
        };
        if let Err(e) = store.save(&record).await {
            tracing::warn!(error = %e, "Failed to record experience");
        }
    }

    /// Resolve secrets in a map of environment variables.
    ///
    /// Values matching `${{secret:KEY}}` are resolved via the secret resolver.
    /// All other values are passed through unchanged.
    async fn resolve_secrets(
        &self,
        env_vars: &std::collections::HashMap<String, String>,
    ) -> Result<std::collections::HashMap<String, String>, DomainError> {
        let mut resolved = std::collections::HashMap::new();
        for (key, value) in env_vars {
            if let Some(secret_key) = parse_secret_ref(value) {
                let secret = self.secret_resolver.resolve(secret_key).await?;
                tracing::debug!(key = %key, source = %secret.source, "Secret resolved");
                resolved.insert(key.clone(), secret.value);
            } else {
                resolved.insert(key.clone(), value.clone());
            }
        }
        Ok(resolved)
    }

    /// Generate worker TLS certificates for a sandbox
    #[allow(dead_code)]
    fn generate_worker_certs(&self, sandbox_id: &str) -> Result<WorkerCerts, anyhow::Error> {
        let (cert_pem, key_pem) = self.gateway_config.auto_tls.issue_worker_cert(sandbox_id)
            .map_err(|e| anyhow::anyhow!("Failed to issue worker cert: {}", e))?;

        let certs_dir = dirs::home_dir()
            .map(|h| h.join(".bastion").join("tls").join("workers").join(sandbox_id))
            .unwrap_or_else(|| PathBuf::from(".bastion/tls/workers").join(sandbox_id));
        std::fs::create_dir_all(&certs_dir)
            .map_err(|e| anyhow::anyhow!("Failed to create cert dir: {}", e))?;
        std::fs::write(certs_dir.join("worker-cert.pem"), &cert_pem)
            .map_err(|e| anyhow::anyhow!("Failed to write cert: {}", e))?;
        std::fs::write(certs_dir.join("worker-key.pem"), &key_pem)
            .map_err(|e| anyhow::anyhow!("Failed to write key: {}", e))?;

        Ok(WorkerCerts {
            cert_path: certs_dir.join("worker-cert.pem"),
            key_path: certs_dir.join("worker-key.pem"),
            ca_path: self.gateway_config.auto_tls.worker_ca_cert_path(),
        })
    }
}

#[allow(dead_code)]
struct WorkerCerts {
    cert_path: PathBuf,
    key_path: PathBuf,
    ca_path: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxCreateParams {
    /// Template (base image) for the sandbox
    pub template: String,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Provider to use: podman, firecracker, gvisor (default: podman)
    #[serde(default = "default_provider_name")]
    pub provider: String,
}

fn default_timeout() -> u64 {
    3_600_000
}

fn default_provider_name() -> String {
    "podman".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunParams {
    /// ID of the sandbox
    pub sandbox_id: String,
    /// Command to execute
    pub command: String,
    /// Optional environment reference from sandbox_prepare
    #[serde(default)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct SandboxWriteParams {
    pub sandbox_id: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxReadParams {
    pub sandbox_id: String,
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxTerminateParams {
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxCancelParams {
    pub sandbox_id: String,
    /// Grace period in milliseconds before sending SIGKILL after SIGTERM. Default: 5000ms
    #[serde(default = "default_grace_period_ms")]
    pub grace_period_ms: u64,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_grace_period_ms() -> u64 {
    5000
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxInfoParams {
    pub sandbox_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxListFilesParams {
    pub sandbox_id: String,
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxRunStreamParams {
    /// ID of the sandbox
    pub sandbox_id: String,
    /// Command to execute
    pub command: String,
    /// Optional environment reference from sandbox_prepare
    #[serde(default)]
    #[allow(dead_code)]
    pub env_ref: Option<String>,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterArtifactParams {
    pub name: String,
    pub version: String,
    pub digest: String,
    pub capability: String,
    #[serde(default)]
    pub tools: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxPrepareParams {
    pub sandbox_id: String,
    pub capability: String,
    /// Timeout in ms for the entire prepare operation (default: 600s for network-heavy ops)
    /// Reserved for future use — currently uses a fixed timeout
    #[serde(default = "default_prepare_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Toolchain strategy override (default: auto)
    ///
    /// - "auto": Let the resolver pick the best approach
    /// - "system_package": Prefer system package managers (apt)
    /// - "version_manager": Prefer version managers (asdf, sdkman)
    /// - "content_addressed": Use pre-packaged artifacts from CA store
    #[serde(default)]
    pub strategy: ToolchainStrategy,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_prepare_timeout() -> u64 {
    600_000 // 10 minutes — covers apt-get install + downloads
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSnapshotParams {
    pub action: String, // "create", "restore", "list", "delete"
    pub sandbox_id: Option<String>,
    pub name: Option<String>,
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SandboxSyncParams {
    pub sandbox_id: String,
    pub mode: String, // "push", "pull", "auto"
    pub source: String,
    pub target: String,
    /// Exclude patterns for sync (reserved for future rsync --exclude support)
    #[serde(default)]
    #[allow(dead_code)]
    pub exclude: Vec<String>,
    /// Optional sync backend override: tar, rsync, podman-cp, auto
    #[serde(default)]
    pub backend: Option<String>,
    /// Timeout in ms (default: 300s for large transfers)
    /// Reserved for future use — currently uses backend-level defaults
    #[serde(default = "default_sync_timeout")]
    #[allow(dead_code)]
    pub timeout_ms: u64,
    /// Optional trace ID to correlate experiences across tools
    #[serde(default)]
    pub trace_id: Option<String>,
}

fn default_sync_timeout() -> u64 {
    300_000 // 5 minutes
}

#[tool_router(router = server_handler, server_handler = false)]
impl BastionGateway {
    /// Resolve a provider by name, falling back to default
    fn resolve_provider(&self, name: &str) -> Arc<dyn SandboxProvider> {
        let name_lower = name.to_lowercase();
        self.providers
            .get(&name_lower)
            .cloned()
            .unwrap_or_else(|| {
                tracing::warn!(requested = name, "Provider not found, falling back to default");
                self.provider.clone()
            })
    }

    #[tool(description = "Create a new isolated sandbox environment")]
    async fn sandbox_create(&self, Parameters(params): Parameters<SandboxCreateParams>) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        let selected_provider = self.resolve_provider(&params.provider);
        tracing::info!(template = %params.template, provider = %params.provider, "Creating sandbox");

        // Try pool checkout first if pool is available
        if let Some(ref pool) = self.gateway_config.pool_manager {
            match pool.checkout(&params.template, params.timeout_ms).await {
                Ok(sandbox) => {
                    tracing::debug!(
                        sandbox_id = %sandbox.id,
                        template = %params.template,
                        "Sandbox created via pool checkout"
                    );
                    self.gateway_config.metrics.record_sandbox_created();
                    return serde_json::json!({
                        "sandbox_id": sandbox.id.to_string(),
                        "status": sandbox.status.to_string(),
                        "template": sandbox.template_id.to_string(),
                        "from_pool": true
                    })
                    .to_string();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Pool checkout failed, falling back to direct creation");
                    // Fall through to direct creation
                }
            }
        }

        // Direct creation (fallback or when pool is disabled)
        let use_case = CreateSandboxUseCase::new(
            self.repository.clone(),
            bastion_domain::shared::id::ProviderId::new(&params.provider),
        );

        let input = bastion_application::sandbox::create::CreateSandboxInput {
            template_id: params.template.clone(),
            provider_id: None,
            resources: bastion_domain::sandbox::value_objects::ResourcesSpec::default(),
            network: bastion_domain::sandbox::value_objects::NetworkSpec::default(),
            env_vars: std::collections::HashMap::new(),
            timeout_ms: params.timeout_ms,
        };

        match use_case.execute(input, selected_provider.as_ref()).await {
            Ok(sandbox) => {
                self.gateway_config.metrics.record_sandbox_created();
                serde_json::json!({
                    "sandbox_id": sandbox.id.to_string(),
                    "status": sandbox.status.to_string(),
                    "template": sandbox.template_id.to_string(),
                    "from_pool": false
                })
                .to_string()
            }
Err(e) => {
                self.gateway_config.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
}
    }

    #[tool(description = "Execute a command in a sandbox")]
    async fn sandbox_run(&self, Parameters(params): Parameters<SandboxRunParams>) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this command execution
        let mut experience = ExperienceRecord::new("sandbox_run")
            .with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        let use_case = RunCommandUseCase::new(self.repository.clone());

        let mut command_spec = CommandSpec::new(&params.command);

        // Resolve env_ref: explicit parameter takes priority, otherwise auto-inject from sandbox_prepare
        let resolved_env_ref = if let Some(ref env_ref) = params.env_ref {
            Some(env_ref.clone())
        } else {
            let last_refs = self.last_env_ref.read().await;
            last_refs.get(&params.sandbox_id).cloned()
        };

        if let Some(ref env_ref) = resolved_env_ref {
            tracing::debug!(sandbox_id = %sandbox_id, env_ref = %env_ref, "Merging environment from env_ref");
            let envs = self.prepared_environments.read().await;
            if let Some(env) = envs.get(env_ref) {
                for (key, value) in env {
                    command_spec = command_spec.with_env(key, value);
                }
            }
        }

        // Resolve secrets
        let secrets_to_inject: Vec<(String, String)> = {
            let mut results = Vec::new();
            for (key, secret_source) in command_spec.secrets.iter() {
                let resolved_value = match secret_source {
                    SecretSource::Inline(value) => value.clone(),
                    SecretSource::Ref(secret_key) => {
                        match self.secret_resolver.resolve(secret_key).await {
                            Ok(secret) => {
                                tracing::debug!(key = %key, source = %secret.source, "Secret resolved");
                                secret.value
                            }
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secret '{}': {}", secret_key, e)}).to_string();
                            }
                        }
                    }
                };
                results.push((key.clone(), resolved_value));
            }
            results
        };
        for (k, v) in secrets_to_inject {
            command_spec = command_spec.with_env(k, v);
        }

        let t0 = std::time::Instant::now();
        match use_case
            .execute(&sandbox_id, &command_spec, self.provider.as_ref())
            .await
        {
            Ok(result) => {
                let duration_us = t0.elapsed().as_micros() as u64;
                self.gateway_config.metrics.record_command(duration_us);

                // Record successful experience
                experience = experience
                    .with_stdout(&result.stdout)
                    .with_stderr(&result.stderr)
                    .completed(result.exit_code);
                if result.timed_out {
                    experience = experience.timed_out();
                }
                self.record_experience(experience).await;

                serde_json::json!({
                    "exit_code": result.exit_code,
                    "stdout": String::from_utf8_lossy(&result.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&result.stderr).to_string(),
                    "duration_ms": result.duration_ms,
                    "timed_out": result.timed_out
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Record failed experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(
        description = "Execute a command with streaming output (returns stdout/stderr separately with exit code)"
    )]
    async fn sandbox_run_stream(
        &self,
        Parameters(params): Parameters<SandboxRunStreamParams>,
        request_ctx: RequestContext<RoleServer>,
    ) -> String {
        // Check rate limit (per-client + global)
        if let Some(rate_limit_error) = self.check_per_client_rate_limit("mcp-client") {
            return rate_limit_error;
        }

        // Extract progress token from meta if present
        let progress_token = request_ctx.meta.get_progress_token();
        let peer = request_ctx.peer.clone();

        tracing::info!(sandbox_id = %params.sandbox_id, command = %params.command, "Running streaming command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());
        let mut command_spec = CommandSpec::new(&params.command);

        // Auto-inject env_ref (same logic as sandbox_run)
        let resolved_env_ref = params.env_ref.clone().or(None);
        let resolved_env_ref = if let Some(ref env_ref) = resolved_env_ref {
            Some(env_ref.clone())
        } else {
            let last_refs = self.last_env_ref.read().await;
            last_refs.get(&params.sandbox_id).cloned()
        };
        if let Some(ref env_ref) = resolved_env_ref {
            let envs = self.prepared_environments.read().await;
            if let Some(env) = envs.get(env_ref) {
                for (key, value) in env {
                    command_spec = command_spec.with_env(key, value);
                }
            }
        }

        // Resolve secrets: collect resolved values first to avoid borrow conflict
        let secrets_to_inject: Vec<(String, String)> = {
            let mut results = Vec::new();
            for (key, secret_source) in command_spec.secrets.iter() {
                let resolved_value = match secret_source {
                    SecretSource::Inline(value) => value.clone(),
                    SecretSource::Ref(secret_key) => {
                        match self.secret_resolver.resolve(secret_key).await {
                            Ok(secret) => {
                                tracing::debug!(key = %key, source = %secret.source, "Secret resolved");
                                secret.value
                            }
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secret '{}': {}", secret_key, e)}).to_string();
                            }
                        }
                    }
                };
                results.push((key.clone(), resolved_value));
            }
            results
        };
        for (k, v) in secrets_to_inject {
            command_spec = command_spec.with_env(k, v);
        }

        let use_case = RunCommandStreamUseCase::new(self.repository.clone());
        let start_time = std::time::Instant::now();

        // Register cancel token for this streaming command
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_tokens.insert(params.sandbox_id.clone(), cancel_flag.clone());

        match use_case
            .execute(&sandbox_id, &command_spec, self.provider.as_ref())
            .await
        {
            Ok(mut stream) => {
                let mut stdout_parts = Vec::new();
                let mut stderr_parts = Vec::new();
                let mut exit_code = -1i32;
                let mut chunk_count = 0u32;

                while let Some(chunk_result) = stream.next().await {
                    // Check cancel flag — if set, stop streaming
                    if cancel_flag.load(Ordering::Relaxed) {
                        tracing::info!(sandbox_id = %params.sandbox_id, "Streaming command cancelled");
                        stderr_parts.push("[CANCELLED] Command was cancelled by user".to_string());
                        exit_code = -1;
                        break;
                    }

                    // Send progress notification if token is present
                    if let Some(ref token) = progress_token {
                        chunk_count += 1;
                        // Estimate progress based on chunk count (0.0 to 0.9 until complete)
                        let progress = (chunk_count as f64 / 100.0).min(0.9);
                        let message = Self::build_progress_message(&stdout_parts, &stderr_parts, chunk_count);
                        if let Some(ref msg) = message {
                            Self::send_progress(&peer, token, progress, Some(msg.as_str())).await;
                        }
                    }

                    match chunk_result {
                        Ok(chunk) => match chunk.chunk_type {
                            ChunkType::Stdout => {
                                stdout_parts.push(String::from_utf8_lossy(&chunk.data).to_string())
                            }
                            ChunkType::Stderr => {
                                stderr_parts.push(String::from_utf8_lossy(&chunk.data).to_string())
                            }
                            ChunkType::ExitCode => {
                                if chunk.data.len() >= 4 {
                                    exit_code = i32::from_le_bytes(
                                        chunk.data[..4].try_into().unwrap_or([-1i8 as u8, 0, 0, 0]),
                                    );
                                }
                            }
                            _ => {}
                        },
                        Err(e) => {
                            stderr_parts.push(format!("Stream error: {}", e));
                        }
                    }
                }

                // Remove cancel token — command finished or was cancelled
                self.cancel_tokens.remove(&params.sandbox_id);

                // Send final progress notification
                if let Some(ref token) = progress_token {
                    Self::send_progress(&peer, token, 1.0, Some("Complete")).await;
                }

                let duration_us = start_time.elapsed().as_micros() as u64;
                self.gateway_config.metrics.record_command(duration_us);

                // Record successful experience
                let stdout_bytes = stdout_parts.join("").into_bytes();
                let stderr_bytes = stderr_parts.join("").into_bytes();
                let mut experience = ExperienceRecord::new("sandbox_run_stream")
                    .with_sandbox_id(sandbox_id.clone());
                if let Some(ref trace_id) = params.trace_id {
                    experience = experience.with_trace_id(trace_id.clone());
                }
                experience = experience
                    .with_stdout(&stdout_bytes)
                    .with_stderr(&stderr_bytes)
                    .completed(exit_code);
                if cancel_flag.load(Ordering::Relaxed) {
                    experience = experience.cancelled();
                }
                self.record_experience(experience).await;

                serde_json::json!({
                    "exit_code": exit_code,
                    "stdout": stdout_parts.join(""),
                    "stderr": stderr_parts.join(""),
                    "chunks_received": stdout_parts.len() + stderr_parts.len(),
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Record failed experience
                let mut experience = ExperienceRecord::new("sandbox_run_stream")
                    .with_sandbox_id(sandbox_id.clone());
                if let Some(ref trace_id) = params.trace_id {
                    experience = experience.with_trace_id(trace_id.clone());
                }
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(description = "Write a file to a sandbox")]
    async fn sandbox_write(&self, Parameters(params): Parameters<SandboxWriteParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Writing file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = WriteFileUseCase::new(self.repository.clone());

        match use_case
            .execute(
                &sandbox_id,
                &params.path,
                params.content.as_bytes(),
                self.provider.as_ref(),
            )
            .await
        {
            Ok(()) => serde_json::json!({"status": "ok"}).to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Read a file from a sandbox")]
    async fn sandbox_read(&self, Parameters(params): Parameters<SandboxReadParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Reading file");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = ReadFileUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, self.provider.as_ref())
            .await
        {
            Ok(content) => serde_json::json!({
                "content": base64::engine::general_purpose::STANDARD.encode(&content),
                "encoding": "base64",
                "size_bytes": content.len()
            })
            .to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "List files in a directory inside a sandbox")]
    async fn sandbox_list_files(
        &self,
        Parameters(params): Parameters<SandboxListFilesParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, path = %params.path, "Listing files");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = ListFilesUseCase::new(self.repository.clone());

        match use_case
            .execute(&sandbox_id, &params.path, self.provider.as_ref())
            .await
        {
            Ok(entries) => {
                let list: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "path": e.path,
                            "is_directory": e.is_directory,
                            "size_bytes": e.size_bytes,
                            "permissions": e.permissions,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "count": list.len(),
                    "entries": list
                })
                .to_string()
            }
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "List available sandbox templates (container images)")]
    async fn sandbox_list_templates(&self) -> String {
        tracing::info!("Listing available templates");

        let mut templates: Vec<serde_json::Value> = Vec::new();

        // Query podman for available images
        match tokio::process::Command::new("podman")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let image = line.trim().to_string();
                    if image.is_empty() || image == "<none>:<none>" {
                        continue;
                    }
                    // Suggest as template
                    templates.push(serde_json::json!({
                        "image": image,
                        "suggested_name": image.trim_start_matches("localhost/").trim_start_matches("docker.io/"),
                    }));
                }
            }
            Ok(_) => {
                tracing::warn!("podman images returned non-zero exit");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list podman images");
            }
        }

        // Add default templates known to work
        let defaults = ["debian:bookworm-slim", "ubuntu:22.04", "fedora:39", "alpine:3.19"];
        for d in &defaults {
            if !templates.iter().any(|t| t["image"].as_str() == Some(d)) {
                templates.push(serde_json::json!({
                    "image": d,
                    "suggested_name": d,
                    "note": "default — may need to be pulled first"
                }));
            }
        }

        serde_json::json!({
            "count": templates.len(),
            "templates": templates
        })
        .to_string()
    }

    #[tool(description = "Terminate and destroy a sandbox")]
    async fn sandbox_terminate(
        &self,
        Parameters(params): Parameters<SandboxTerminateParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Terminating sandbox");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Try to return to pool first if pool is available
        if let Some(ref pool) = self.gateway_config.pool_manager {
            match pool.checkin(&sandbox_id).await {
                Ok(()) => {
                    tracing::debug!(sandbox_id = %params.sandbox_id, "Sandbox returned to pool");
                    self.gateway_config.metrics.record_sandbox_terminated();
                    return serde_json::json!({
                        "status": "pooled",
                        "sandbox_id": params.sandbox_id
                    })
                    .to_string();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Pool checkin failed, terminating directly");
                    // Fall through to direct termination
                }
            }
        }

        let use_case = TerminateSandboxUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id, self.provider.as_ref()).await {
            Ok(()) => {
                self.gateway_config.metrics.record_sandbox_terminated();
                serde_json::json!({"status": "terminated"}).to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(description = "Cancel a running command in a sandbox")]
    async fn sandbox_cancel(
        &self,
        Parameters(params): Parameters<SandboxCancelParams>,
    ) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, grace_period_ms = params.grace_period_ms, "Cancelling sandbox command");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this cancel operation
        let mut experience = ExperienceRecord::new("sandbox_cancel")
            .with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // Signal local cancel token (for streaming commands)
        if let Some(token) = self.cancel_tokens.get(&params.sandbox_id) {
            token.store(true, Ordering::Relaxed);
            tracing::info!(sandbox_id = %params.sandbox_id, "Cancel flag set");
        }

        // Also ask the provider to cancel the command (SIGTERM/SIGKILL)
        match self.provider.cancel_command(&sandbox_id, params.grace_period_ms).await {
            Ok(cancelled) => {
                // Record cancelled experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": if cancelled { "cancelled" } else { "no_running_command" },
                    "sandbox_id": params.sandbox_id
                })
                .to_string()
            }
            Err(e) => {
                self.gateway_config.metrics.record_error();

                // Record failed experience
                experience = experience.cancelled();
                self.record_experience(experience).await;

                serde_json::json!({"error": e.to_string()}).to_string()
            }
        }
    }

    #[tool(description = "Get information about a sandbox")]
    async fn sandbox_info(&self, Parameters(params): Parameters<SandboxInfoParams>) -> String {
        tracing::info!(sandbox_id = %params.sandbox_id, "Getting sandbox info");

        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        let use_case = GetSandboxInfoUseCase::new(self.repository.clone());

        match use_case.execute(&sandbox_id).await {
            Ok(info) => serde_json::json!({
                "sandbox_id": info.id.to_string(),
                "status": info.status.to_string(),
                "template": info.template_id.to_string(),
                "created_at": info.created_at.to_rfc3339(),
                "expires_at": info.expires_at.map(|t| t.to_rfc3339()),
            })
            .to_string(),
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "List all active sandboxes")]
    async fn sandbox_list(&self) -> String {
        tracing::info!("Listing active sandboxes");

        let use_case = ListSandboxesUseCase::new(self.repository.clone());

        match use_case.execute().await {
            Ok(sandboxes) => {
                let list: Vec<serde_json::Value> = sandboxes
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "sandbox_id": s.id.to_string(),
                            "status": s.status.to_string(),
                            "template": s.template_id.to_string(),
                            "created_at": s.created_at.to_rfc3339(),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "count": list.len(),
                    "sandboxes": list
                })
                .to_string()
            }
            Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
        }
    }

    #[tool(description = "Get sandbox pool statistics")]
    async fn sandbox_pool_stats(&self) -> String {
        tracing::trace!("Getting pool statistics");

        if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            serde_json::json!({
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle,
                "total": stats.total,
                "templates": stats.templates.iter().map(|t| {
                    serde_json::json!({
                        "template": t.template,
                        "idle": t.idle,
                        "min_idle": t.min_idle,
                        "max_idle": t.max_idle
                    })
                }).collect::<Vec<_>>()
            })
            .to_string()
        } else {
            serde_json::json!({
                "enabled": false,
                "message": "Pool is not enabled"
            })
            .to_string()
        }
    }

    #[tool(description = "Check gateway health including provider connectivity and pool status")]
    async fn sandbox_health(&self) -> String {
        let mut checks = Vec::new();

        // Check provider connectivity
        checks.push(serde_json::json!({
            "component": "provider",
            "provider": self.provider.name(),
            "status": "ok"
        }));

        // Check pool status
        if let Some(ref pool) = self.gateway_config.pool_manager {
            let stats = pool.stats().await;
            checks.push(serde_json::json!({
                "component": "pool",
                "status": "ok",
                "enabled": true,
                "active": stats.active,
                "idle": stats.idle
            }));
        } else {
            checks.push(serde_json::json!({
                "component": "pool",
                "status": "disabled"
            }));
        }

        serde_json::json!({
            "status": "healthy",
            "version": env!("CARGO_PKG_VERSION"),
            "checks": checks
        })
        .to_string()
    }

    #[tool(description = "Get gateway metrics in Prometheus format")]
    async fn sandbox_metrics(&self) -> String {
        tracing::debug!("Getting metrics");
        self.gateway_config.metrics.prometheus_export()
    }

    #[tool(description = "Register a template artifact that provides a capability")]
    async fn sandbox_register_artifact(
        &self,
        Parameters(params): Parameters<RegisterArtifactParams>,
    ) -> String {
        let tools: Vec<ToolDescriptor> = params
            .tools
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let parts: Vec<&str> = s.trim().split(':').collect();
                ToolDescriptor {
                    name: parts.first().unwrap_or(&"").to_string(),
                    version: parts.get(1).unwrap_or(&"any").to_string(),
                    category: bastion_domain::template::Category::Generic,
                    manager_preference: vec![],
                }
            })
            .collect();

        let artifact = TemplateArtifact::builder(&params.name, &params.version)
            .digest(&params.digest)
            .add_capability(CapabilityDescriptor {
                name: params.capability.clone(),
                tools,
                verification: vec![],
            })
            .build();

        {
            let mut catalog = self.artifact_catalog.write().await;
            catalog.register(artifact);
        }

        serde_json::json!({
            "status": "registered",
            "name": params.name,
            "capability": params.capability
        })
        .to_string()
    }

    #[tool(description = "Prepare a sandbox with a specific capability (e.g. jvm-build)")]
    async fn sandbox_prepare(
        &self,
        Parameters(params): Parameters<SandboxPrepareParams>,
    ) -> String {
        let sandbox_id = SandboxId::new(&params.sandbox_id);
        let capability = &params.capability;

        // Create experience record for this prepare operation
        let mut experience = ExperienceRecord::new("sandbox_prepare")
            .with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // Try artifact catalog first
        let artifact = {
            let catalog = self.artifact_catalog.read().await;
            catalog.resolve(capability).cloned()
        };

        // If artifact found, use materializer
        if let Ok(artifact) = artifact {
            let materializer = PodmanOptimizedMaterializer::new(
                self.provider.clone(),
                self.artifact_store.clone(),
                PathBuf::from("/tmp/bastion-cache"),
            );
            match materializer
                .materialize(&sandbox_id, &artifact, MaterializationMode::Auto)
                .await
            {
                Ok(result) => {
                    // Auto-inject: register this env_ref as default for this sandbox
                    if let Some(ref env_ref) = result.env_ref {
                        let mut last_refs = self.last_env_ref.write().await;
                        last_refs.insert(sandbox_id.to_string(), env_ref.clone());
                    }

                    // Record successful prepare experience
                    experience = experience.completed(0);
                    self.record_experience(experience).await;

                    return serde_json::json!({
                        "status": "ready",
                        "method": "artifact",
                        "env_ref": result.env_ref,
                        "cache_hit": result.cache_hit,
                        "duration_ms": result.duration_ms
                    })
                    .to_string();
                }
                Err(e) => {
                    tracing::warn!("Artifact materialization failed: {}, falling back to resolver", e);
                }
            }
        }

        // Try TOML-driven CapabilityRegistry first (if capability is registered)
        if let Some(plan) = self.capability_registry.read().await.resolve(capability, params.strategy.clone()) {
            // Execute the TOML-defined plan steps in the sandbox
            use bastion_domain::execution::command::CommandSpec;
            let t0 = std::time::Instant::now();

            for step in &plan.steps {
                let mut cmd = CommandSpec::new(&step.command)
                    .with_timeout(step.timeout_ms);
                for (k, v) in &step.env {
                    // Resolve secret refs in env var values (e.g., "${{secret:GITHUB_TOKEN}}")
                    let mut env_map: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    env_map.insert(k.clone(), v.clone());
                    let resolved = match self.resolve_secrets(&env_map).await {
                        Ok(r) => r,
                        Err(e) => {
                            return serde_json::json!({"error": format!("Failed to resolve secrets: {}", e)}).to_string();
                        }
                    };
                    cmd = cmd.with_env(k.as_str(), resolved.get(k).cloned().unwrap_or_else(|| v.clone()));
                }

                match self.provider.run_command(&sandbox_id, &cmd).await {
                    Ok(result) => {
                        if result.exit_code != step.expected_exit_code {
                            return serde_json::json!({
                                "error": format!("Step '{}' failed: exit {} (expected {})",
                                    step.description, result.exit_code, step.expected_exit_code)
                            }).to_string();
                        }
                    }
                    Err(e) => {
                        return serde_json::json!({
                            "error": format!("Step '{}' error: {}", step.description, e)
                        }).to_string();
                    }
                }
            }

            // Run verification steps if present
            for verify in &plan.verification {
                let cmd = CommandSpec::new(&verify.command)
                    .with_timeout(60000); // 60s timeout for verification

                match self.provider.run_command(&sandbox_id, &cmd).await {
                    Ok(result) => {
                        if result.exit_code != verify.expected_exit_code {
                            return serde_json::json!({
                                "error": format!("Verification '{}' failed: exit {} (expected {})",
                                    verify.label, result.exit_code, verify.expected_exit_code)
                            }).to_string();
                        }
                        if let Some(expected) = &verify.expected_output_contains {
                            let stdout_str = String::from_utf8_lossy(&result.stdout);
                            if !stdout_str.contains(expected) {
                                return serde_json::json!({
                                    "error": format!("Verification '{}' output mismatch", verify.label)
                                }).to_string();
                            }
                        }
                    }
                    Err(e) => {
                        return serde_json::json!({
                            "error": format!("Verification '{}' error: {}", verify.label, e)
                        }).to_string();
                    }
                }
            }

            let duration_ms = t0.elapsed().as_millis() as u64;

            // Generate env_ref and store the environment for later use by sandbox_run
            let env_ref = format!("registry:{}:{}", sandbox_id, capability);
            {
                let mut envs = self.prepared_environments.write().await;
                envs.insert(env_ref.clone(), plan.env.clone());
            }
            // Auto-inject: register this env_ref as the default for this sandbox
            {
                let mut last_refs = self.last_env_ref.write().await;
                last_refs.insert(sandbox_id.to_string(), env_ref.clone());
            }

            tracing::info!(capability = %capability, adapter = %plan.adapter_used, "Sandbox prepared via TOML capability registry");

            // Record successful prepare experience
            experience = experience.completed(0);
            self.record_experience(experience).await;

            return serde_json::json!({
                "status": "ready",
                "method": "registry",
                "adapter_used": plan.adapter_used,
                "capability": capability,
                "env_ref": env_ref,
                "env": plan.env,
                "path_prefix": plan.path_prefix,
                "duration_ms": duration_ms
            })
            .to_string();
        }

        // Fallback: use ToolResolver with adapters (hardcoded)
        let mut resolver = ToolResolver::new();
        resolver.register(Box::new(AptAdapter));
        resolver.register(Box::new(AsdfAdapter));
        resolver.register(Box::new(SdkmanAdapter));

        let req = ToolchainRequest {
            sandbox_id: sandbox_id.clone(),
            capability: capability.clone(),
            constraints: std::collections::HashMap::new(),
            strategy: params.strategy,
        };

        match resolver.resolve(&req).await {
            Ok(plan) => {
                // Execute the plan steps in the sandbox
                use bastion_domain::execution::command::CommandSpec;
                let t0 = std::time::Instant::now();

                for step in &plan.steps {
                    let mut cmd = CommandSpec::new(&step.command)
                        .with_timeout(step.timeout_ms);
                    for (k, v) in &step.env {
                        // Resolve secret refs in env var values (e.g., "${{secret:GITHUB_TOKEN}}")
                        let mut env_map: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        env_map.insert(k.clone(), v.clone());
                        let resolved = match self.resolve_secrets(&env_map).await {
                            Ok(r) => r,
                            Err(e) => {
                                return serde_json::json!({"error": format!("Failed to resolve secrets: {}", e)}).to_string();
                            }
                        };
                        cmd = cmd.with_env(k.as_str(), resolved.get(k).cloned().unwrap_or_else(|| v.clone()));
                    }

                    match self.provider.run_command(&sandbox_id, &cmd).await {
                        Ok(result) => {
                            if result.exit_code != step.expected_exit_code {
                                return serde_json::json!({
                                    "error": format!("Step '{}' failed: exit {} (expected {})",
                                        step.description, result.exit_code, step.expected_exit_code)
                                }).to_string();
                            }
                        }
                        Err(e) => {
                            return serde_json::json!({
                                "error": format!("Step '{}' error: {}", step.description, e)
                            }).to_string();
                        }
                    }
                }

                let duration_ms = t0.elapsed().as_millis() as u64;

                // Generate env_ref and store the environment for later use by sandbox_run
                let env_ref = format!("resolver:{}:{}", sandbox_id, capability);
                {
                    let mut envs = self.prepared_environments.write().await;
                    envs.insert(env_ref.clone(), plan.env.clone());
                }
                // Auto-inject: register this env_ref as the default for this sandbox
                {
                    let mut last_refs = self.last_env_ref.write().await;
                    last_refs.insert(sandbox_id.to_string(), env_ref.clone());
                }

                // Record successful prepare experience
                experience = experience.completed(0);
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": "ready",
                    "method": "resolver",
                    "adapter_used": plan.adapter_used,
                    "capability": capability,
                    "env_ref": env_ref,
                    "env": plan.env,
                    "path_prefix": plan.path_prefix,
                    "duration_ms": duration_ms
                })
                .to_string()
            }
            Err(e) => {
                serde_json::json!({"error": format!("{}", e)}).to_string()
            }
        }
    }

    #[tool(description = "Manage sandbox snapshots (create, restore, list, delete)")]
    async fn sandbox_snapshot(
        &self,
        Parameters(params): Parameters<SandboxSnapshotParams>,
    ) -> String {
        let snapshot_manager = SnapshotManager::new(bastion_domain::template::ProviderKind::Podman);

        match params.action.as_str() {
            "create" => {
                let sandbox_id = match &params.sandbox_id {
                    Some(id) => SandboxId::new(id),
                    None => return serde_json::json!({"error": "sandbox_id required for create"}).to_string(),
                };
                let name = match &params.name {
                    Some(n) => n.as_str(),
                    None => return serde_json::json!({"error": "name required for create"}).to_string(),
                };

                match snapshot_manager.create_snapshot(&sandbox_id, name).await {
                    Ok(info) => serde_json::json!({
                        "status": "created",
                        "snapshot_id": info.snapshot_id,
                        "sandbox_id": info.sandbox_id,
                        "name": info.name,
                        "created_at": info.created_at.to_rfc3339(),
                        "size_bytes": info.size_bytes
                    }).to_string(),
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            "restore" => {
                let snapshot_id = match &params.snapshot_id {
                    Some(id) => id.as_str(),
                    None => return serde_json::json!({"error": "snapshot_id required for restore"}).to_string(),
                };

                match snapshot_manager.restore_snapshot(snapshot_id).await {
                    Ok(sandbox) => {
                        // Register the restored sandbox in the gateway's repository
                        // so it's visible to sandbox_run, sandbox_info, etc.
                        if let Err(e) = self.repository.save(&sandbox).await {
                            tracing::error!(sandbox_id = %sandbox.id, error = %e, "Failed to register restored sandbox");
                        }
                        self.gateway_config.metrics.record_sandbox_created();
                        serde_json::json!({
                            "status": "restored",
                            "sandbox_id": sandbox.id.to_string(),
                            "snapshot_id": snapshot_id
                        }).to_string()
                    },
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            "list" => {
                match snapshot_manager.list_snapshots().await {
                    Ok(list) => serde_json::json!({
                        "status": "ok",
                        "snapshots": list.iter().map(|s| {
                            serde_json::json!({
                                "snapshot_id": s.snapshot_id,
                                "name": s.name,
                                "created_at": s.created_at.to_rfc3339(),
                                "size_bytes": s.size_bytes
                            })
                        }).collect::<Vec<_>>(),
                        "count": list.len()
                    }).to_string(),
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            "delete" => {
                let snapshot_id = match &params.snapshot_id {
                    Some(id) => id.as_str(),
                    None => return serde_json::json!({"error": "snapshot_id required for delete"}).to_string(),
                };

                match snapshot_manager.delete_snapshot(snapshot_id).await {
                    Ok(()) => serde_json::json!({
                        "status": "deleted",
                        "snapshot_id": snapshot_id
                    }).to_string(),
                    Err(e) => serde_json::json!({"error": format!("{}", e)}).to_string(),
                }
            }
            _ => serde_json::json!({"error": format!("Unknown action: {}", params.action)}).to_string(),
        }
    }

    #[tool(description = "Sync files between host and sandbox (push/pull)")]
    async fn sandbox_sync(
        &self,
        Parameters(params): Parameters<SandboxSyncParams>,
    ) -> String {
        let sandbox_id = SandboxId::new(params.sandbox_id.clone());

        // Create experience record for this sync operation
        let mut experience = ExperienceRecord::new("sandbox_sync")
            .with_sandbox_id(sandbox_id.clone());
        if let Some(ref trace_id) = params.trace_id {
            experience = experience.with_trace_id(trace_id.clone());
        }

        // SYNC-01: Liveness pre-check before sync
        let provider = self.resolve_provider("podman");
        match provider.is_alive(&sandbox_id).await {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                return serde_json::json!({
                    "error": format!("sandbox not alive: {}", sandbox_id),
                    "sandbox_id": sandbox_id.to_string()
                }).to_string();
            }
        }

        let mode = params.mode.as_str();
        let source = params.source.as_str();
        let target = params.target.as_str();
        let timeout_ms = params.timeout_ms;

        // Determine backend: explicit override > gateway default > auto
        let backend: SyncBackend = params
            .backend
            .as_deref()
            .and_then(|b| match b {
                "tar" => Some(SyncBackend::Tar),
                "rsync" => Some(SyncBackend::Rsync),
                "podman-cp" | "podman_cp" => Some(SyncBackend::PodmanCp),
                "auto" => Some(SyncBackend::Auto),
                _ => None,
            })
            .unwrap_or(self.sync_backend);

        // Auto-detect best backend for Podman
        let effective_backend = if backend == SyncBackend::Auto {
            // tar is most compatible for rootless podman
            SyncBackend::Tar
        } else {
            backend
        };

        let container_name = sandbox_id.to_string();

        tracing::info!(
            sandbox_id = %sandbox_id,
            mode = mode,
            backend = ?effective_backend,
            source = source,
            target = target,
            "Syncing files"
        );

        let result = match (mode, effective_backend) {
            ("push", SyncBackend::Tar) | ("push", SyncBackend::Auto) => {
                // tar pipe: local tar -> podman exec tar
                let cmd = format!(
                    "tar czf - -C \"$(dirname '{}')\" \"$(basename '{}')\" 2>/dev/null | podman exec -i {} tar xzf - -C \"{}\"",
                    source, source, container_name, target
                );
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output(),
                )
                .await
            }
            ("pull", SyncBackend::Tar) | ("pull", SyncBackend::Auto) => {
                // tar pipe: podman exec tar -> local tar
                let cmd = format!(
                    "podman exec {} tar czf - -C \"$(dirname '{}')\" \"$(basename '{}')\" 2>/dev/null | tar xzf - -C \"{}\"",
                    container_name, source, source, target
                );
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output(),
                )
                .await
            }
            ("push", SyncBackend::PodmanCp) => {
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("podman")
                        .args(["cp", source, &format!("{}:{}", container_name, target)])
                        .output(),
                )
                .await
            }
            ("pull", SyncBackend::PodmanCp) => {
                tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    tokio::process::Command::new("podman")
                        .args(["cp", &format!("{}:{}", container_name, source), target])
                        .output(),
                )
                .await
            }
            ("push" | "pull", SyncBackend::Rsync) => {
                // rsync requires rsync in the sandbox; fall back with clear error
                return serde_json::json!({
                    "error": "rsync backend requires rsync installed in the sandbox. Use 'tar' or 'podman-cp' instead.",
                    "hint": "Set backend to 'tar' or 'podman-cp'"
                }).to_string();
            }
            _ => {
                return serde_json::json!({
                    "error": format!("Unknown mode '{}' or backend combination", mode)
                }).to_string();
            }
        };

        match result {
            Ok(Ok(output)) if output.status.success() => {
                // Record successful sync experience
                let exit_code = output.status.code().unwrap_or(0);
                experience = experience.completed(exit_code);
                self.record_experience(experience).await;

                serde_json::json!({
                    "status": "synced",
                    "mode": mode,
                    "backend": format!("{:?}", effective_backend).to_lowercase(),
                    "sandbox_id": params.sandbox_id,
                    "source": source,
                    "target": target
                }).to_string()
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr_str: &str = &stderr;
                // SYNC-02: Include exit_code when stderr is empty instead of "unknown error"
                if stderr_str.trim().is_empty() {
                    let exit_code = output.status.code().unwrap_or(-1);
                    serde_json::json!({
                        "error": format!("Sync failed with exit code {}", exit_code),
                        "mode": mode,
                        "backend": format!("{:?}", effective_backend).to_lowercase(),
                        "exit_code": exit_code,
                        "stderr": "(empty)"
                    }).to_string()
                } else {
                    serde_json::json!({
                        "error": format!("Sync failed: {}", stderr.lines().next().unwrap_or("unknown error")),
                        "mode": mode,
                        "backend": format!("{:?}", effective_backend).to_lowercase(),
                        "exit_code": output.status.code().unwrap_or(-1),
                        "stderr": stderr_str
                    }).to_string()
                }
            }
            Ok(Err(e)) => {
                serde_json::json!({"error": format!("Sync command error: {}", e)}).to_string()
            }
            Err(_) => {
                serde_json::json!({
                    "error": format!("Sync timed out after {}ms. Increase timeout_ms for large transfers.", timeout_ms),
                    "timeout_ms": timeout_ms,
                }).to_string()
            }
        }
    }

    /// Send a progress notification to the MCP client.
    /// If sending fails, logs a warning but continues execution.
    async fn send_progress(
        peer: &rmcp::Peer<rmcp::RoleServer>,
        token: &ProgressToken,
        progress: f64,
        message: Option<&str>,
    ) {
        let params = match message {
            Some(msg) => ProgressNotificationParam::new(token.clone(), progress).with_message(msg),
            None => ProgressNotificationParam::new(token.clone(), progress),
        };
        if let Err(e) = peer.notify_progress(params).await {
            tracing::warn!(error = %e, "Failed to send progress notification");
        }
    }

    /// Build a progress message from current stdout/stderr accumulated output.
    fn build_progress_message(
        stdout_parts: &[String],
        stderr_parts: &[String],
        chunk_count: u32,
    ) -> Option<String> {
        // Show last 200 chars of stdout as preview, truncated for notification size
        let stdout_preview = stdout_parts
            .last()
            .map(|s| {
                if s.len() > 200 {
                    format!("{}...", &s[s.len() - 200..])
                } else {
                    s.clone()
                }
            })
            .filter(|s| !s.is_empty());

        let message = match (stdout_preview, stderr_parts.is_empty()) {
            (Some(preview), true) => format!("[{} chunks] {}", chunk_count, preview),
            (Some(preview), false) => format!("[{} chunks] {} (+stderr)", chunk_count, preview),
            (None, false) => format!("[{} chunks] (stderr: {})", chunk_count, stderr_parts.len()),
            (None, true) => format!("[{} chunks] processing...", chunk_count),
        };

        // Truncate message if too long for notification
        if message.len() > 500 {
            Some(format!("{}...", &message[..500]))
        } else {
            Some(message)
        }
    }
}

/// Combine server_handler, catalog_tools, doctor_tools, and advice_tools routers into a single ServerHandler impl.
#[tool_handler(router = (Self::server_handler() + crate::catalog_tools::catalog_tools() + crate::doctor_tools::doctor_tools() + crate::advice_tools::advice_tools()))]
impl ServerHandler for BastionGateway {}
