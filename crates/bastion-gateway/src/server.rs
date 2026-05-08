//! MCP Server handler for Bastion Gateway.
//!
//! Implements the rmcp ServerHandler with sandbox tools.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use rmcp::{ServerHandler, tool_handler};
use tokio::sync::RwLock;

use bastion_domain::catalog::experience::ExperienceRecord;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::secret::{SecretResolver, parse_secret_ref};
use bastion_domain::shared::DomainError;
use bastion_domain::template::ArtifactCatalog;
use bastion_infrastructure::catalog::toml_advice_parser::{AdviceConfigStore, AdviceRegistry};
use bastion_infrastructure::enrichment::{BastionEnrichmentAdapter, EnrichmentConfig};
use bastion_infrastructure::metrics::GatewayMetrics;
use bastion_infrastructure::pool::SandboxPoolManager;
use bastion_infrastructure::template::{CapabilityRegistry, FsArtifactStore};

#[path = "sandbox_tools.rs"]
mod sandbox_tools;

// Re-export SyncBackend from sandbox_tools for backward compatibility
#[allow(unused_imports)]
pub use sandbox_tools::SyncBackend;

/// Configuration for catalog-related components (experience, assertions, doctors, advice).
/// Wraps multiple optional catalog registries into a single config struct to reduce
/// the argument count in `BastionGateway::new()`.
#[derive(Clone, Default)]
pub struct CatalogConfig {
    /// Optional experience store for catalog recording (Phase 2)
    pub experience_store: Option<Arc<dyn bastion_domain::catalog::experience::ExperienceStore>>,
    /// Optional assertion registry loaded from TOML (Phase 3)
    pub assertion_registry:
        Option<Arc<bastion_infrastructure::catalog::toml_assertion_parser::AssertionRegistry>>,
    /// Optional doctor registry loaded from TOML (Phase 4)
    pub doctor_registry:
        Option<Arc<bastion_infrastructure::catalog::toml_doctor_parser::DoctorRegistry>>,
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

/// Authentication configuration for worker registry.
/// Controls HMAC pre-shared key validation for worker registration.
#[derive(Clone, Default)]
pub struct AuthConfig {
    /// When true, workers must present an HMAC proof verifiable by
    /// one of the configured pre-shared keys to register.
    pub pre_shared_key_enabled: bool,
    /// List of pre-shared keys accepted for worker registration.
    /// Used when `pre_shared_key_enabled` is true.
    pub pre_shared_keys: Vec<String>,
}

/// Configuration for gateway operational settings (pool, metrics, TLS, auth).
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
    /// Authentication config (HMAC pre-shared key settings)
    #[allow(dead_code)]
    pub auth: AuthConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            pool_manager: None,
            metrics: GatewayMetrics::default(),
            auto_tls: Arc::new(crate::auto_tls::get_auto_tls().clone()),
            auth: AuthConfig::default(),
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
    pub(crate) repository: Arc<dyn SandboxRepository>,
    secret_resolver: Arc<dyn SecretResolver>,
    /// Gateway operational config (pool, metrics, TLS)
    pub(crate) gateway_config: GatewayConfig,
    artifact_catalog: Arc<RwLock<ArtifactCatalog>>,
    artifact_store: Arc<FsArtifactStore>,
    /// Prepared environments keyed by env_ref
    prepared_environments:
        Arc<RwLock<std::collections::HashMap<String, std::collections::HashMap<String, String>>>>,
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
    /// Optional enrichment adapter for sandbox command enrichment
    pub(crate) enrichment_adapter: Arc<Option<BastionEnrichmentAdapter>>,
    /// Enrichment configuration
    enrichment_config: EnrichmentConfig,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        providers: std::collections::HashMap<String, Arc<dyn SandboxProvider>>,
    repository: Arc<dyn SandboxRepository>,
        secret_resolver: Arc<dyn SecretResolver>,
        gateway_config: GatewayConfig,
        capability_registry: CapabilityRegistry,
        catalog_config: CatalogConfig,
        enrichment_adapter: Arc<Option<BastionEnrichmentAdapter>>,
        enrichment_config: EnrichmentConfig,
    ) -> Self {
        Self {
            provider,
            providers: Arc::new(providers),
            repository,
            secret_resolver,
            gateway_config,
            artifact_catalog: Arc::new(RwLock::new(ArtifactCatalog::new())),
            artifact_store: Arc::new(FsArtifactStore::new(PathBuf::from(
                "/tmp/bastion-artifacts",
            ))),
            prepared_environments: Arc::new(RwLock::new(std::collections::HashMap::new())),
            last_env_ref: Arc::new(RwLock::new(std::collections::HashMap::new())),
            sync_backend: SyncBackend::Auto,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(500.0, 100.0))),
            per_client_rate_limiter: Arc::new(PerClientRateLimiter::new(100.0, 20.0, 1000)),
            capability_registry: Arc::new(RwLock::new(capability_registry)),
            cancel_tokens: Arc::new(DashMap::new()),
            catalog_config,
            enrichment_adapter,
            enrichment_config,
        }
    }

    /// Check rate limit (global + per-client) and return error response if exceeded.
    /// Uses per-client limiting with a global fallback.
    fn check_per_client_rate_limit(&self, client_key: &str) -> Option<String> {
        // Per-client rate limit
        if !self.per_client_rate_limiter.try_consume(client_key) {
            return Some(
                serde_json::json!({
                    "error": "Per-client rate limit exceeded",
                    "hint": "This client is sending requests too fast. Slow down.",
                    "client": client_key
                })
                .to_string(),
            );
        }

        // Global rate limit (backstop)
        // Use Ok pattern to handle poisoned lock gracefully instead of unwrap()
        let Ok(mut limiter) = self.rate_limiter.lock() else {
            tracing::error!("Rate limiter mutex poisoned - returning error instead of panicking");
            return Some(
                serde_json::json!({
                    "error": "Internal rate-limit lock error",
                    "hint": "The rate limiter is in an inconsistent state. Please retry."
                })
                .to_string(),
            );
        };
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
        let (cert_pem, key_pem) = self
            .gateway_config
            .auto_tls
            .issue_worker_cert(sandbox_id)
            .map_err(|e| anyhow::anyhow!("Failed to issue worker cert: {}", e))?;

        let certs_dir = dirs::home_dir()
            .map(|h| {
                h.join(".bastion")
                    .join("tls")
                    .join("workers")
                    .join(sandbox_id)
            })
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

// Sandbox tool params are defined in sandbox_tools.rs
// Re-export param types for backward compatibility
#[allow(unused_imports)]
pub use sandbox_tools::{
    RegisterArtifactParams, SandboxCancelParams, SandboxCreateParams, SandboxInfoParams,
    SandboxListFilesParams, SandboxPrepareParams, SandboxReadParams, SandboxRunParams,
    SandboxRunStreamParams, SandboxSnapshotParams, SandboxSyncParams, SandboxTerminateParams,
    SandboxWriteParams,
};

/// Combine catalog_tools, doctor_tools, advice_tools, enrichment_tools, and sandbox_tools routers into a single ServerHandler impl.
#[tool_handler(router = (crate::catalog_tools::catalog_tools() + crate::doctor_tools::doctor_tools() + crate::advice_tools::advice_tools() + crate::enrichment_tools::enrichment_tools() + crate::server::sandbox_tools::sandbox_tools()))]
impl ServerHandler for BastionGateway {}
