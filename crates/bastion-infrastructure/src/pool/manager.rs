//! Sandbox pool manager implementation.
//!
//! Pre-creates containers and keeps them "warm" so that `sandbox_create`
//! can return in <200ms instead of ~1.5s.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{Semaphore, oneshot};
use tokio::time;

use bastion_domain::provider::port::SandboxProvider;
use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::sandbox::value_objects::{
    NetworkSpec, ResourcesSpec, SandboxFilter, SandboxStatus,
};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::SandboxId;

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
            min_idle: 5,
            max_idle: 20,
            max_total: 200,
            idle_timeout_ms: 600_000,  // 10 minutes
            refill_interval_ms: 5_000, // 5 seconds
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
    ///
    /// Uses single-pass `VecDeque::retain` for O(n) complexity, avoiding
    /// index invalidation issues with concurrent access patterns via DashMap.
    fn evict_idle(&mut self) -> Vec<Sandbox> {
        let timeout_ms = self.idle_timeout_ms;
        let mut evicted = Vec::new();

        self.entries.retain(|entry| {
            if entry.is_idle_timeout(timeout_ms) {
                evicted.push(entry.sandbox.clone());
                false // evict: don't retain
            } else {
                true // keep: retain
            }
        });

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

/// Result of a pool recovery operation.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RecoveryResult {
    /// Number of sandboxes successfully reintegrated into the pool.
    pub reintegrated: usize,
    /// Number of sandboxes skipped because the pool is at max_idle capacity.
    pub skipped_pool_full: usize,
    /// Number of sandboxes skipped because their template is not registered.
    pub skipped_not_registered: usize,
    /// Number of sandboxes in repository marked as terminated (orphaned).
    pub orphaned_terminated: usize,
    /// Number of sandboxes that failed to recover.
    pub failed: usize,
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

    /// Recover active sandboxes from the provider on startup.
    ///
    /// This reconciles the provider's view of running sandboxes with the
    /// repository. Sandboxes that are running in the provider but exist
    /// in the repository are reintegrated into the pool if their template
    /// is registered. Sandboxes that are marked active in the repository
    /// but are not running in the provider are marked as terminated.
    ///
    /// This is a "best effort" operation — failures on individual sandboxes
    /// do not stop the recovery of remaining sandboxes.
    pub async fn recover_active_sandboxes(&self) -> Result<RecoveryResult, DomainError> {
        let filter = SandboxFilter {
            status: Some(SandboxStatus::Running),
            ..Default::default()
        };

        let provider_sandboxes = match self.provider.list_sandboxes(&filter).await {
            Ok(sandboxes) => sandboxes,
            Err(e) => {
                tracing::warn!(error = %e, "Provider list_sandboxes failed during recovery, starting with empty pool");
                return Ok(RecoveryResult::default());
            }
        };

        let mut result = RecoveryResult::default();
        let provider_sandbox_ids: std::collections::HashSet<_> =
            provider_sandboxes.iter().map(|s| s.id.clone()).collect();

        // Phase 1: Reintegrate running sandboxes from provider
        for provider_sandbox in &provider_sandboxes {
            let template = provider_sandbox.template_id.to_string();

            // Check if template is registered in pool
            if !self.pools.contains_key(&template) {
                tracing::debug!(
                    sandbox_id = %provider_sandbox.id,
                    template = %template,
                    "Skipping recovery: template not registered in pool"
                );
                result.skipped_not_registered += 1;
                continue;
            }

            // Check if sandbox exists in repository
            match self.repository.find_by_id(&provider_sandbox.id).await {
                Ok(Some(repo_sandbox)) => {
                    // Sandbox exists in repository
                    if repo_sandbox.status != SandboxStatus::Running {
                        // Sandbox was marked as not running in repo, update it
                        tracing::info!(
                            sandbox_id = %provider_sandbox.id,
                            repo_status = ?repo_sandbox.status,
                            "Updating sandbox status to Running in repository"
                        );
                        let mut updated = repo_sandbox.clone();
                        if updated.mark_running().is_err() {
                            tracing::warn!(
                                sandbox_id = %provider_sandbox.id,
                                "Failed to mark recovered sandbox as running"
                            );
                        } else if self.repository.update(&updated).await.is_err() {
                            tracing::warn!(
                                sandbox_id = %provider_sandbox.id,
                                "Failed to update recovered sandbox in repository"
                            );
                        }
                    }

                    // Reintegrate into pool if there's capacity
                    if let Some(mut queue) = self.pools.get_mut(&template) {
                        if queue.value().idle_count() >= queue.value().max_idle {
                            tracing::debug!(
                                sandbox_id = %provider_sandbox.id,
                                template = %template,
                                idle_count = queue.value().idle_count(),
                                max_idle = queue.value().max_idle,
                                "Skipping recovery: pool at max_idle capacity"
                            );
                            result.skipped_pool_full += 1;
                            continue;
                        }

                        if queue.value_mut().checkin(provider_sandbox.clone()) {
                            result.reintegrated += 1;
                            self.total_idle
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            tracing::info!(
                                sandbox_id = %provider_sandbox.id,
                                template = %template,
                                "Reintegrated active sandbox into pool"
                            );
                        } else {
                            result.skipped_pool_full += 1;
                            tracing::debug!(
                                sandbox_id = %provider_sandbox.id,
                                "Failed to reintegrate sandbox into pool"
                            );
                        }
                    }
                }
                Ok(None) => {
                    // Sandbox not in repository - create a minimal entry
                    tracing::info!(
                        sandbox_id = %provider_sandbox.id,
                        template = %template,
                        "Recovered sandbox not in repository, saving it"
                    );

                    if let Err(e) = self.repository.save(provider_sandbox).await {
                        tracing::warn!(
                            sandbox_id = %provider_sandbox.id,
                            error = %e,
                            "Failed to save recovered sandbox to repository"
                        );
                        result.failed += 1;
                    } else {
                        // Try to reintegrate into pool
                        if let Some(mut queue) = self.pools.get_mut(&template) {
                            if queue.value_mut().checkin(provider_sandbox.clone()) {
                                result.reintegrated += 1;
                                self.total_idle
                                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            } else {
                                result.skipped_pool_full += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        sandbox_id = %provider_sandbox.id,
                        error = %e,
                        "Failed to check repository for sandbox"
                    );
                    result.failed += 1;
                }
            }
        }

        // Phase 2: Mark orphaned sandboxes as terminated
        // These are sandboxes in the repository that are marked as active
        // but are not in the provider's list of running sandboxes
        if let Ok(active_in_repo) = self.repository.find_active().await {
            for repo_sandbox in active_in_repo {
                if !provider_sandbox_ids.contains(&repo_sandbox.id) {
                    tracing::info!(
                        sandbox_id = %repo_sandbox.id,
                        repo_status = ?repo_sandbox.status,
                        "Marking sandbox as terminated: not in provider's active list"
                    );
                    let mut updated = repo_sandbox.clone();
                    if updated.terminate().is_ok() {
                        if let Err(e) = self.repository.update(&updated).await {
                            tracing::warn!(
                                sandbox_id = %repo_sandbox.id,
                                error = %e,
                                "Failed to update orphaned sandbox status"
                            );
                        } else {
                            result.orphaned_terminated += 1;
                        }
                    }
                }
            }
        }

        tracing::info!(
            reintegrated = result.reintegrated,
            skipped_pool_full = result.skipped_pool_full,
            skipped_not_registered = result.skipped_not_registered,
            orphaned_terminated = result.orphaned_terminated,
            failed = result.failed,
            "Pool recovery completed"
        );

        Ok(result)
    }

    /// Start the background refill task.
    pub async fn start(&self) -> Result<(), DomainError> {
        // Recovery phase: reintegrate active sandboxes from provider
        tracing::info!("Starting pool recovery...");
        let recovery_result = self.recover_active_sandboxes().await?;
        tracing::info!(
            reintegrated = recovery_result.reintegrated,
            skipped_pool_full = recovery_result.skipped_pool_full,
            orphaned_terminated = recovery_result.orphaned_terminated,
            "Pool recovery completed"
        );

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
        state
            .total_idle
            .fetch_sub(total_evicted, std::sync::atomic::Ordering::SeqCst);

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
                        state
                            .total_idle
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
                &id, template, &resources, &network, &env_vars,
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

        self.total_idle
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.total_active
            .store(0, std::sync::atomic::Ordering::SeqCst);

        tracing::info!(terminated = total_terminated, "SandboxPoolManager stopped");

        Ok(())
    }

    /// Checkout a sandbox from the pool (or create a new one if pool is empty).
    pub async fn checkout(&self, template: &str, timeout_ms: u64) -> Result<Sandbox, DomainError> {
        // Try to get from pool first
        if let Some(mut queue) = self.pools.get_mut(template)
            && let Some(sandbox) = queue.value_mut().checkout()
        {
            self.total_idle
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.total_active
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

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

        let permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DomainError::ResourceExhausted("No permits available".to_string()))?;

        let id = SandboxId::generate();
        let resources = ResourcesSpec::default();
        let network = NetworkSpec::default();
        let env_vars = std::collections::HashMap::new();

        let sandbox = self
            .provider
            .create(&id, template, &resources, &network, &env_vars, timeout_ms)
            .await?;

        self.repository.save(&sandbox).await?;
        self.total_active
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Release permit on drop will count toward max_total
        drop(permit);

        Ok(sandbox)
    }

    /// Return a sandbox to the pool (resets state and returns to pool).
    pub async fn checkin(&self, sandbox_id: &SandboxId) -> Result<(), DomainError> {
        // Find the sandbox in the repository
        let sandbox = self
            .repository
            .find_by_id(sandbox_id)
            .await?
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
            self.total_active
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.total_idle
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            tracing::debug!(
                sandbox_id = %sandbox_id,
                template = %template,
                "Checked in sandbox to pool"
            );
            return Ok(());
        }

        // Pool queue doesn't exist or is full, just terminate
        self.total_active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

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
    use futures::Stream;

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.min_idle, 5);
        assert_eq!(config.max_idle, 20);
        assert_eq!(config.max_total, 200);
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
            None,
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
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        let sandbox2 = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );
        let sandbox3 = Sandbox::new(
            SandboxId::generate(),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        assert!(queue.checkin(sandbox1));
        assert!(queue.checkin(sandbox2));
        assert!(!queue.checkin(sandbox3)); // Should fail - max_idle is 2

        assert_eq!(queue.idle_count(), 2);
    }

    #[test]
    fn test_evict_idle_single_pass_retain() {
        // Test: evict_idle uses single-pass VecDeque::retain pattern
        // Verifies the implementation doesn't use index-based removal
        let mut queue = PoolQueue::new(0, 10, 100); // idle_timeout = 100ms

        // Create three pool entries
        let entry1 = PoolEntry::new(Sandbox::new(
            SandboxId::new("entry-1"),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        ));
        let entry2 = PoolEntry::new(Sandbox::new(
            SandboxId::new("entry-2"),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        ));
        let entry3 = PoolEntry::new(Sandbox::new(
            SandboxId::new("entry-3"),
            bastion_domain::shared::id::TemplateId::new("test"),
            bastion_domain::shared::id::ProviderId::new("podman"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        ));

        queue.entries.push_back(entry1);
        queue.entries.push_back(entry2);
        queue.entries.push_back(entry3);

        // evict_idle with very short timeout (1ms) on entries just created
        // will not evict (they haven't exceeded timeout yet)
        let evicted = queue.evict_idle();
        assert_eq!(
            evicted.len(),
            0,
            "No entries should be evicted with recent creation time"
        );
        assert_eq!(queue.idle_count(), 3, "All 3 entries should remain");

        // Calling again should produce same result (idempotent)
        let evicted2 = queue.evict_idle();
        assert_eq!(evicted2.len(), 0, "Second call should also evict nothing");
        assert_eq!(queue.idle_count(), 3, "Queue unchanged after second call");
    }

    #[tokio::test]
    async fn test_evict_idle_concurrent_safety() {
        // Test concurrent eviction safety: multiple tokio tasks calling
        // evict_idle on the same PoolQueue via Mutex should not cause
        // index-out-of-bounds, lost entries, or double-eviction
        use std::sync::Arc;

        // Create a PoolQueue
        let queue = PoolQueue::new(0, 10, 100_000); // long timeout - entries won't expire

        // Add 5 entries
        let mut queue = queue;
        for i in 0..5 {
            queue.entries.push_back(PoolEntry::new(Sandbox::new(
                SandboxId::new(format!("concurrent-{}", i)),
                bastion_domain::shared::id::TemplateId::new("test"),
                bastion_domain::shared::id::ProviderId::new("podman"),
                None,
                ResourcesSpec::default(),
                NetworkSpec::default(),
            )));
        }

        // Use Arc<Mutex<PoolQueue>> to simulate concurrent access
        let queue_arc = Arc::new(tokio::sync::Mutex::new(queue));
        let mut handles = vec![];

        // Spawn 10 concurrent tasks all calling evict_idle
        for _ in 0..10 {
            let queue_clone = queue_arc.clone();
            let handle = tokio::spawn(async move {
                let mut queue = queue_clone.lock().await;
                let evicted = queue.evict_idle();
                evicted.len()
            });
            handles.push(handle);
        }

        // Collect all results
        let mut total_evicted = 0;
        for handle in handles {
            let count = handle.await.expect("Task should not panic");
            total_evicted += count;
        }

        // All 10 tasks should get 0 evicted (entries haven't expired)
        assert_eq!(
            total_evicted, 0,
            "No entries should be evicted with long timeout"
        );

        // Verify queue still has all 5 entries
        let queue = queue_arc.lock().await;
        assert_eq!(
            queue.idle_count(),
            5,
            "All 5 entries should remain after concurrent evict calls"
        );
    }

    #[tokio::test]
    async fn test_recover_active_sandboxes() {
        use bastion_domain::execution::command::CommandSpec;
        use bastion_domain::execution::stream::CommandChunk;
        use bastion_domain::file_ops::FileEntry;
        use bastion_domain::provider::capabilities::ProviderCapabilities;
        use bastion_domain::provider::executor::TaskExecutor;
        use bastion_domain::provider::lifecycle::SandboxLifecycle;
        use bastion_domain::provider::port::SandboxProvider;
        use bastion_domain::sandbox::snapshot::SnapshotInfo;
        use std::collections::HashMap;
        use std::pin::Pin;

        // Create mock provider
        let provider_sandbox = Sandbox::new(
            SandboxId::new("test-sandbox-1"),
            bastion_domain::shared::id::TemplateId::new("test-template"),
            bastion_domain::shared::id::ProviderId::new("mock"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        let provider_sandbox2 = Sandbox::new(
            SandboxId::new("test-sandbox-2"),
            bastion_domain::shared::id::TemplateId::new("test-template"),
            bastion_domain::shared::id::ProviderId::new("mock"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        // Track what methods are called
        let list_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let list_called_clone = list_called.clone();

        // Create in-memory repository with the sandbox already saved
        let repo_sandbox = provider_sandbox.clone();
        let repo_sandbox2 = provider_sandbox2.clone();
        let repo_sandboxes = std::sync::Arc::new(tokio::sync::Mutex::new(vec![
            repo_sandbox.clone(),
            repo_sandbox2.clone(),
        ]));

        // Mock provider implementation
        #[derive(Debug)]
        struct MockProvider {
            sandboxes: Vec<Sandbox>,
            list_called: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait::async_trait]
        impl SandboxLifecycle for MockProvider {
            async fn create(
                &self,
                _id: &SandboxId,
                _template: &str,
                _resources: &ResourcesSpec,
                _network: &NetworkSpec,
                _env_vars: &HashMap<String, String>,
                _timeout_ms: u64,
            ) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
            async fn terminate(&self, _id: &SandboxId) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
                unimplemented!()
            }
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities::try_new(
                    false,
                    true,
                    false,
                    86_400_000,
                    16_384,
                    16,
                    true,
                    false,
                    1500,
                )
                .expect("known valid values")
            }
            fn name(&self) -> &str {
                "mock"
            }
            async fn list_sandboxes(
                &self,
                filter: &SandboxFilter,
            ) -> Result<Vec<Sandbox>, DomainError> {
                self.list_called
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                if filter.status == Some(SandboxStatus::Running) {
                    Ok(self.sandboxes.clone())
                } else {
                    Ok(vec![])
                }
            }
            async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
            async fn set_timeout(
                &self,
                _id: &SandboxId,
                _timeout_ms: u64,
            ) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn create_snapshot(
                &self,
                _id: &SandboxId,
                _name: &str,
            ) -> Result<SnapshotInfo, DomainError> {
                unimplemented!()
            }
            async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl TaskExecutor for MockProvider {
            async fn run_command(
                &self,
                _id: &SandboxId,
                _command: &CommandSpec,
            ) -> Result<bastion_domain::execution::command::CommandResult, DomainError>
            {
                unimplemented!()
            }
            async fn run_command_stream(
                &self,
                _id: &SandboxId,
                _command: &CommandSpec,
            ) -> Result<
                Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>,
                DomainError,
            > {
                unimplemented!()
            }
            async fn write_file(
                &self,
                _id: &SandboxId,
                _path: &str,
                _content: &[u8],
            ) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn read_file(
                &self,
                _id: &SandboxId,
                _path: &str,
            ) -> Result<Vec<u8>, DomainError> {
                unimplemented!()
            }
            async fn list_files(
                &self,
                _id: &SandboxId,
                _dir: &str,
            ) -> Result<Vec<FileEntry>, DomainError> {
                unimplemented!()
            }
        }
        #[derive(Debug)]
        struct MockRepository {
            sandboxes: std::sync::Arc<tokio::sync::Mutex<Vec<Sandbox>>>,
        }

        #[async_trait::async_trait]
        impl SandboxRepository for MockRepository {
            async fn save(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
                let mut sb = self.sandboxes.lock().await;
                if !sb.iter().any(|s| s.id == sandbox.id) {
                    sb.push(sandbox.clone());
                }
                Ok(())
            }
            async fn find_by_id(&self, id: &SandboxId) -> Result<Option<Sandbox>, DomainError> {
                let sb = self.sandboxes.lock().await;
                Ok(sb.iter().find(|s| s.id == *id).cloned())
            }
            async fn update(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
                let mut sb = self.sandboxes.lock().await;
                if let Some(idx) = sb.iter().position(|s| s.id == sandbox.id) {
                    sb[idx] = sandbox.clone();
                }
                Ok(())
            }
            async fn delete(&self, _id: &SandboxId) -> Result<(), DomainError> {
                Ok(())
            }
            async fn find_active(&self) -> Result<Vec<Sandbox>, DomainError> {
                let sb = self.sandboxes.lock().await;
                Ok(sb.iter().filter(|s| s.is_active()).cloned().collect())
            }
            async fn find_expired(&self) -> Result<Vec<Sandbox>, DomainError> {
                let sb = self.sandboxes.lock().await;
                let now = chrono::Utc::now();
                Ok(sb
                    .iter()
                    .filter(|s| s.expires_at.map(|exp| exp < now).unwrap_or(false))
                    .cloned()
                    .collect())
            }
        }

        let provider = Arc::new(MockProvider {
            sandboxes: vec![provider_sandbox.clone(), provider_sandbox2.clone()],
            list_called,
        });

        let repository = Arc::new(MockRepository {
            sandboxes: repo_sandboxes.clone(),
        });

        let config = PoolConfig {
            min_idle: 1,
            max_idle: 3,
            max_total: 10,
            idle_timeout_ms: 60000,
            refill_interval_ms: 5000,
        };

        let manager = SandboxPoolManager::new(provider.clone(), repository.clone(), config);
        manager.register_template("test-template");

        // Run recovery
        let result = manager.recover_active_sandboxes().await.unwrap();

        // Verify list_sandboxes was called
        assert!(list_called_clone.load(std::sync::atomic::Ordering::SeqCst));

        // Verify the sandboxes were reintegrated
        assert_eq!(result.reintegrated, 2);
        assert_eq!(result.skipped_not_registered, 0);
        assert_eq!(result.skipped_pool_full, 0);
        assert_eq!(result.failed, 0);

        // Check the pool now has 2 sandboxes
        let pool = manager.pools.get("test-template").unwrap();
        assert_eq!(pool.idle_count(), 2);
    }

    #[tokio::test]
    async fn test_recover_active_sandboxes_skips_unregistered_templates() {
        use bastion_domain::execution::command::CommandSpec;
        use bastion_domain::execution::stream::CommandChunk;
        use bastion_domain::file_ops::FileEntry;
        use bastion_domain::provider::capabilities::ProviderCapabilities;
        use bastion_domain::provider::executor::TaskExecutor;
        use bastion_domain::provider::lifecycle::SandboxLifecycle;
        use bastion_domain::provider::port::SandboxProvider;
        use bastion_domain::sandbox::snapshot::SnapshotInfo;
        use std::collections::HashMap;
        use std::pin::Pin;

        // Sandbox with unregistered template
        let provider_sandbox = Sandbox::new(
            SandboxId::new("unregistered-template-sandbox"),
            bastion_domain::shared::id::TemplateId::new("unregistered-template"),
            bastion_domain::shared::id::ProviderId::new("mock"),
            None,
            ResourcesSpec::default(),
            NetworkSpec::default(),
        );

        // Mock provider
        #[derive(Debug)]
        struct MockProvider {
            sandboxes: Vec<Sandbox>,
        }

        #[async_trait::async_trait]
        impl SandboxLifecycle for MockProvider {
            async fn create(
                &self,
                _id: &SandboxId,
                _template: &str,
                _resources: &ResourcesSpec,
                _network: &NetworkSpec,
                _env_vars: &HashMap<String, String>,
                _timeout_ms: u64,
            ) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
            async fn terminate(&self, _id: &SandboxId) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn is_alive(&self, _id: &SandboxId) -> Result<bool, DomainError> {
                unimplemented!()
            }
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities::try_new(
                    false,
                    true,
                    false,
                    86_400_000,
                    16_384,
                    16,
                    true,
                    false,
                    1500,
                )
                .expect("known valid values")
            }
            fn name(&self) -> &str {
                "mock"
            }
            async fn list_sandboxes(
                &self,
                filter: &SandboxFilter,
            ) -> Result<Vec<Sandbox>, DomainError> {
                if filter.status == Some(SandboxStatus::Running) {
                    Ok(self.sandboxes.clone())
                } else {
                    Ok(vec![])
                }
            }
            async fn get_info(&self, _id: &SandboxId) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
            async fn set_timeout(
                &self,
                _id: &SandboxId,
                _timeout_ms: u64,
            ) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn create_snapshot(
                &self,
                _id: &SandboxId,
                _name: &str,
            ) -> Result<SnapshotInfo, DomainError> {
                unimplemented!()
            }
            async fn restore_snapshot(&self, _snapshot_id: &str) -> Result<Sandbox, DomainError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl TaskExecutor for MockProvider {
            async fn run_command(
                &self,
                _id: &SandboxId,
                _command: &CommandSpec,
            ) -> Result<bastion_domain::execution::command::CommandResult, DomainError>
            {
                unimplemented!()
            }
            async fn run_command_stream(
                &self,
                _id: &SandboxId,
                _command: &CommandSpec,
            ) -> Result<
                Pin<Box<dyn Stream<Item = Result<CommandChunk, DomainError>> + Send>>,
                DomainError,
            > {
                unimplemented!()
            }
            async fn write_file(
                &self,
                _id: &SandboxId,
                _path: &str,
                _content: &[u8],
            ) -> Result<(), DomainError> {
                unimplemented!()
            }
            async fn read_file(
                &self,
                _id: &SandboxId,
                _path: &str,
            ) -> Result<Vec<u8>, DomainError> {
                unimplemented!()
            }
            async fn list_files(
                &self,
                _id: &SandboxId,
                _dir: &str,
            ) -> Result<Vec<FileEntry>, DomainError> {
                unimplemented!()
            }
        }

        // Mock repository
        #[derive(Debug)]
        struct MockRepository;

        #[async_trait::async_trait]
        impl SandboxRepository for MockRepository {
            async fn save(&self, _sandbox: &Sandbox) -> Result<(), DomainError> {
                Ok(())
            }
            async fn find_by_id(&self, _id: &SandboxId) -> Result<Option<Sandbox>, DomainError> {
                Ok(None)
            }
            async fn update(&self, _sandbox: &Sandbox) -> Result<(), DomainError> {
                Ok(())
            }
            async fn delete(&self, _id: &SandboxId) -> Result<(), DomainError> {
                Ok(())
            }
            async fn find_active(&self) -> Result<Vec<Sandbox>, DomainError> {
                Ok(vec![])
            }
            async fn find_expired(&self) -> Result<Vec<Sandbox>, DomainError> {
                Ok(vec![])
            }
        }

        let provider = Arc::new(MockProvider {
            sandboxes: vec![provider_sandbox.clone()],
        });
        let repository = Arc::new(MockRepository);

        let config = PoolConfig::default();
        let manager = SandboxPoolManager::new(provider, repository, config);
        // Note: NOT registering "unregistered-template"

        let result = manager.recover_active_sandboxes().await.unwrap();

        // Sandbox should be skipped because template is not registered
        assert_eq!(result.reintegrated, 0);
        assert_eq!(result.skipped_not_registered, 1);
    }
}
