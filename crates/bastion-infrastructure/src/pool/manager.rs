//! Sandbox pool manager implementation.
//!
//! Pre-creates containers and keeps them "warm" so that `sandbox_create`
//! can return in <200ms instead of ~1.5s.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{oneshot, Semaphore};
use tokio::time;

use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::shared::id::SandboxId;
use bastion_domain::shared::DomainError;

/// Configuration for the pool manager.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Minimum warm containers per template.
    pub min_idle: usize,
    /// Maximum warm containers per template.
    pub max_idle: usize,
    /// Maximum total sandboxes across all templates.
    pub max_total: usize,
    /// Kill containers idle too long (milliseconds).
    pub idle_timeout_ms: u64,
    /// How often to check and refill (milliseconds).
    pub refill_interval_ms: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_idle: 2,
            max_idle: 5,
            max_total: 50,
            idle_timeout_ms: 600_000,  // 10 minutes
            refill_interval_ms: 5_000,  // 5 seconds
        }
    }
}

/// A warm sandbox entry in the pool.
#[derive(Debug, Clone)]
struct PoolEntry {
    sandbox: Sandbox,
    #[allow(dead_code)]
    created_at: Instant,
    last_used_at: Instant,
}

impl PoolEntry {
    fn new(sandbox: Sandbox) -> Self {
        let now = Instant::now();
        Self {
            sandbox,
            created_at: now,
            last_used_at: now,
        }
    }

    fn is_idle_timeout(&self, timeout_ms: u64) -> bool {
        self.last_used_at.elapsed() > Duration::from_millis(timeout_ms)
    }
}

/// Queue of warm sandboxes for a specific template.
#[derive(Debug, Clone)]
pub struct PoolQueue {
    entries: VecDeque<PoolEntry>,
    min_idle: usize,
    max_idle: usize,
    idle_timeout_ms: u64,
}

impl PoolQueue {
    fn new(min_idle: usize, max_idle: usize, idle_timeout_ms: u64) -> Self {
        Self {
            entries: VecDeque::new(),
            min_idle,
            max_idle,
            idle_timeout_ms,
        }
    }

    /// Try to checkout a warm sandbox from the pool.
    fn checkout(&mut self) -> Option<Sandbox> {
        self.entries.pop_front().map(|entry| entry.sandbox)
    }

    /// Return a sandbox to the pool.
    fn checkin(&mut self, sandbox: Sandbox) -> bool {
        if self.entries.len() >= self.max_idle {
            return false;
        }
        self.entries.push_back(PoolEntry::new(sandbox));
        true
    }

    /// Get the number of idle sandboxes.
    fn idle_count(&self) -> usize {
        self.entries.len()
    }

    /// Remove sandboxes that have been idle too long.
    fn evict_idle(&mut self) -> Vec<Sandbox> {
        let mut evicted = Vec::new();
        let mut to_remove = Vec::new();

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.is_idle_timeout(self.idle_timeout_ms) {
                to_remove.push(i);
            }
        }

        // Remove in reverse order to maintain indices
        for i in to_remove.into_iter().rev() {
            if let Some(entry) = self.entries.remove(i) {
                evicted.push(entry.sandbox);
            }
        }

        evicted
    }

    /// Ensure we have at least min_idle entries by creating new ones if needed.
    fn needs_refill(&self) -> bool {
        self.entries.len() < self.min_idle
    }
}

/// Pool statistics for monitoring.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    /// Per-template statistics.
    pub templates: Vec<TemplateStats>,
    /// Total active (checked out) sandboxes.
    pub active: usize,
    /// Total idle (in pool) sandboxes.
    pub idle: usize,
    /// Total sandboxes across all pools.
    pub total: usize,
}

/// Statistics for a single template pool.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TemplateStats {
    pub template: String,
    pub idle: usize,
    pub min_idle: usize,
    pub max_idle: usize,
}

/// Shared state for the pool background task.
struct PoolState {
    pools: Arc<DashMap<String, PoolQueue>>,
    total_idle: Arc<std::sync::atomic::AtomicUsize>,
    total_active: Arc<std::sync::atomic::AtomicUsize>,
    semaphore: Arc<Semaphore>,
    provider: Arc<dyn SandboxProvider>,
    repository: Arc<dyn SandboxRepository>,
    config: PoolConfig,
}

/// Sandbox pool manager that pre-creates and maintains warm containers.
pub struct SandboxPoolManager {
    provider: Arc<dyn SandboxProvider>,
    repository: Arc<dyn SandboxRepository>,
    pools: Arc<DashMap<String, PoolQueue>>,
    config: PoolConfig,
    total_active: Arc<std::sync::atomic::AtomicUsize>,
    total_idle: Arc<std::sync::atomic::AtomicUsize>,
    semaphore: Arc<Semaphore>,
    shutdown_tx: tokio::sync::Mutex<Option<oneshot::Sender<()>>>,
    refill_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for SandboxPoolManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxPoolManager")
            .field("config", &self.config)
            .finish()
    }
}

impl SandboxPoolManager {
    /// Create a new pool manager.
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        repository: Arc<dyn SandboxRepository>,
        config: PoolConfig,
    ) -> Self {
        let max_permits = config.max_total;
        Self {
            provider,
            repository,
            pools: Arc::new(DashMap::new()),
            config,
            total_active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            total_idle: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            semaphore: Arc::new(Semaphore::new(max_permits)),
            shutdown_tx: tokio::sync::Mutex::new(None),
            refill_handle: tokio::sync::Mutex::new(None),
        }
    }

    /// Start the background refill task.
    pub async fn start(&self) -> Result<(), DomainError> {
        let (tx, rx) = oneshot::channel();
        *self.shutdown_tx.lock().await = Some(tx);

        let state = PoolState {
            pools: self.pools.clone(),
            total_idle: self.total_idle.clone(),
            total_active: self.total_active.clone(),
            semaphore: self.semaphore.clone(),
            provider: self.provider.clone(),
            repository: self.repository.clone(),
            config: self.config.clone(),
        };

        let handle = tokio::spawn(async move {
            Self::refill_loop(state, rx).await;
        });

        *self.refill_handle.lock().await = Some(handle);

        tracing::info!(
            min_idle = %self.config.min_idle,
            max_idle = %self.config.max_idle,
            max_total = %self.config.max_total,
            "SandboxPoolManager started"
        );

        Ok(())
    }

    async fn refill_loop(state: PoolState, mut rx: oneshot::Receiver<()>) {
        let mut interval = time::interval(Duration::from_millis(state.config.refill_interval_ms));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    Self::refill_and_evict(&state).await;
                }
                _ = &mut rx => {
                    tracing::info!("Pool manager received shutdown signal");
                    break;
                }
            }
        }
    }

    async fn refill_and_evict(state: &PoolState) {
        // First, evict idle sandboxes
        let mut total_evicted = 0;
        for mut entry in state.pools.iter_mut() {
            let evicted = entry.value_mut().evict_idle();
            for sandbox in evicted {
                if let Err(e) = state.provider.terminate(&sandbox.id).await {
                    tracing::warn!(sandbox_id = %sandbox.id, error = %e, "Failed to terminate evicted sandbox");
                }
                let _ = state.repository.delete(&sandbox.id).await;
                total_evicted += 1;
            }
        }
        state.total_idle.fetch_sub(total_evicted, std::sync::atomic::Ordering::SeqCst);

        // Then, refill pools that need it
        for mut entry in state.pools.iter_mut() {
            let template = entry.key().clone();

            while entry.value_mut().needs_refill() {
                // Check if we can acquire a permit
                match state.semaphore.try_acquire() {
                    Ok(_permit) => {
                        let sandbox = match Self::create_sandbox(
                            &state.provider,
                            &state.repository,
                            &template,
                        )
                        .await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::warn!(template = %template, error = %e, "Failed to create sandbox for pool");
                                break;
                            }
                        };

                        if !entry.value_mut().checkin(sandbox) {
                            // Pool is full, terminate the sandbox we just created
                            if let Err(e) = state.provider.terminate(&SandboxId::generate()).await {
                                tracing::warn!(error = %e, "Failed to terminate excess sandbox");
                            }
                            break;
                        }
                        state.total_idle.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    Err(_) => {
                        // No permits available, skip refill
                        tracing::debug!("No permits available for pool refill");
                        break;
                    }
                }
            }
        }

        // Log stats periodically
        tracing::debug!(
            active = %state.total_active.load(std::sync::atomic::Ordering::SeqCst),
            idle = %state.total_idle.load(std::sync::atomic::Ordering::SeqCst),
            "Pool stats"
        );
    }

    async fn create_sandbox(
        provider: &Arc<dyn SandboxProvider>,
        repository: &Arc<dyn SandboxRepository>,
        template: &str,
    ) -> Result<Sandbox, DomainError> {
        let id = SandboxId::generate();
        let resources = ResourcesSpec::default();
        let network = NetworkSpec::default();
        let env_vars = std::collections::HashMap::new();

        let sandbox = provider
            .create(
                &id,
                template,
                &resources,
                &network,
                &env_vars,
                3_600_000, // 1 hour timeout for pooled sandboxes
            )
            .await?;

        repository.save(&sandbox).await?;

        Ok(sandbox)
    }

    /// Stop the pool manager and terminate all pooled sandboxes.
    pub async fn stop(&self) -> Result<(), DomainError> {
        // Signal shutdown
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }

        // Wait for the background task to finish
        if let Some(handle) = self.refill_handle.lock().await.take() {
            let _ = handle.await;
        }

        // Terminate all pooled sandboxes
        let mut total_terminated = 0;
        for mut entry in self.pools.iter_mut() {
            let evicted: Vec<_> = entry.value_mut().evict_idle();
            for sandbox in evicted {
                if let Err(e) = self.provider.terminate(&sandbox.id).await {
                    tracing::warn!(sandbox_id = %sandbox.id, error = %e, "Failed to terminate sandbox during stop");
                }
                let _ = self.repository.delete(&sandbox.id).await;
                total_terminated += 1;
            }
        }

        self.total_idle.store(0, std::sync::atomic::Ordering::SeqCst);
        self.total_active.store(0, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(terminated = total_terminated, "SandboxPoolManager stopped");

        Ok(())
    }

    /// Checkout a sandbox from the pool (or create a new one if pool is empty).
    pub async fn checkout(
        &self,
        template: &str,
        timeout_ms: u64,
    ) -> Result<Sandbox, DomainError> {
        // Try to get from pool first
        if let Some(mut queue) = self.pools.get_mut(template)
            && let Some(sandbox) = queue.value_mut().checkout()
        {
            self.total_idle.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.total_active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            tracing::debug!(
                sandbox_id = %sandbox.id,
                template = %template,
                "Checked out sandbox from pool"
            );

            // Update repository with active status
            let _ = self.repository.update(&sandbox).await;

            return Ok(sandbox);
        }

        // Pool is empty or doesn't exist, fall back to direct creation
        tracing::debug!(template = %template, "Pool empty, creating sandbox directly");

        let permit = self.semaphore.acquire().await
            .map_err(|_| DomainError::ResourceExhausted("No permits available".to_string()))?;

        let id = SandboxId::generate();
        let resources = ResourcesSpec::default();
        let network = NetworkSpec::default();
        let env_vars = std::collections::HashMap::new();

        let sandbox = self
            .provider
            .create(
                &id,
                template,
                &resources,
                &network,
                &env_vars,
                timeout_ms,
            )
            .await?;

        self.repository.save(&sandbox).await?;
        self.total_active.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Release permit on drop will count toward max_total
        drop(permit);

        Ok(sandbox)
    }

    /// Return a sandbox to the pool (resets state and returns to pool).
    pub async fn checkin(&self, sandbox_id: &SandboxId) -> Result<(), DomainError> {
        // Find the sandbox in the repository
        let sandbox = self.repository.find_by_id(sandbox_id).await?
            .ok_or_else(|| DomainError::NotFound(sandbox_id.to_string()))?;

        let template = sandbox.template_id.to_string();

        // Reset the container by terminating and recreating
        // For now, just terminate - pool will create fresh on next checkout
        if let Err(e) = self.provider.terminate(sandbox_id).await {
            tracing::warn!(sandbox_id = %sandbox_id, error = %e, "Failed to terminate sandbox during checkin");
        }

        // Try to create a new warm sandbox for the pool
        if let Some(mut queue) = self.pools.get_mut(&template)
            && queue.value_mut().checkin(sandbox)
        {
            self.total_active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.total_idle.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            tracing::debug!(
                sandbox_id = %sandbox_id,
                template = %template,
                "Checked in sandbox to pool"
            );
            return Ok(());
        }

        // Pool queue doesn't exist or is full, just terminate
        self.total_active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

        tracing::debug!(
            sandbox_id = %sandbox_id,
            template = %template,
            "Pool full, sandbox terminated"
        );

        Ok(())
    }

    /// Get current pool statistics.
    pub async fn stats(&self) -> PoolStats {
        let mut templates = Vec::new();
        let mut total_idle = 0usize;

        for entry in self.pools.iter() {
            let idle = entry.value().idle_count();
            total_idle += idle;

            templates.push(TemplateStats {
                template: entry.key().clone(),
                idle,
                min_idle: entry.value().min_idle,
                max_idle: entry.value().max_idle,
            });
        }

        PoolStats {
            templates,
            active: self.total_active.load(std::sync::atomic::Ordering::SeqCst),
            idle: total_idle,
            total: total_idle + self.total_active.load(std::sync::atomic::Ordering::SeqCst),
        }
    }

    /// Register a template with the pool.
    pub fn register_template(&self, template: &str) {
        let queue = PoolQueue::new(
            self.config.min_idle,
            self.config.max_idle,
            self.config.idle_timeout_ms,
        );
        self.pools.insert(template.to_string(), queue);
        tracing::debug!(template = %template, "Template registered with pool manager");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.min_idle, 2);
        assert_eq!(config.max_idle, 5);
        assert_eq!(config.max_total, 50);
        assert_eq!(config.idle_timeout_ms, 600_000);
        assert_eq!(config.refill_interval_ms, 5_000);
    }

    #[test]
    fn test_pool_queue_checkout_checkin() {
        let mut queue = PoolQueue::new(1, 3, 60000);

        let sandbox = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        assert!(queue.checkout().is_none());

        queue.checkin(sandbox.clone());
        assert_eq!(queue.idle_count(), 1);

        let checked = queue.checkout().unwrap();
        assert_eq!(checked.id, sandbox.id);
        assert_eq!(queue.idle_count(), 0);
    }

    #[test]
    fn test_pool_queue_max_idle() {
        let mut queue = PoolQueue::new(1, 2, 60000);

        let sandbox1 = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        let sandbox2 = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        let sandbox3 = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        assert!(queue.checkin(sandbox1));
        assert!(queue.checkin(sandbox2));
        assert!(!queue.checkin(sandbox3)); // Should fail - max_idle is 2

        assert_eq!(queue.idle_count(), 2);
    }
}
