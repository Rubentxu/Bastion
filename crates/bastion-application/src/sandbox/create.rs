//! Create sandbox use case.

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec};
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::provider::SandboxProvider;
use bastion_domain::shared::id::{SandboxId, ProviderId};
use bastion_domain::shared::DomainError;
use std::collections::HashMap;
use std::sync::Arc;

/// Input for creating a new sandbox.
pub struct CreateSandboxInput {
    pub template_id: String,
    pub provider_id: Option<ProviderId>,
    pub resources: ResourcesSpec,
    pub network: NetworkSpec,
    pub env_vars: HashMap<String, String>,
    pub timeout_ms: u64,
}

/// Use case: Create a new sandbox.
///
/// Orchestrates provider selection, sandbox creation, and persistence.
pub struct CreateSandboxUseCase {
    repository: Arc<dyn SandboxRepository>,
    default_provider_id: ProviderId,
}

impl CreateSandboxUseCase {
    pub fn new(repository: Arc<dyn SandboxRepository>, default_provider_id: ProviderId) -> Self {
        Self { repository, default_provider_id }
    }

    pub async fn execute(
        &self,
        input: CreateSandboxInput,
        provider: &dyn SandboxProvider,
    ) -> Result<Sandbox, DomainError> {
        let _provider_id = input.provider_id
            .unwrap_or_else(|| self.default_provider_id.clone());

        let sandbox_id = SandboxId::generate();

        tracing::info!(
            sandbox_id = %sandbox_id,
            template = %input.template_id,
            provider = %provider.name(),
            "Creating sandbox"
        );

        let sandbox = provider.create(
            &sandbox_id,
            &input.template_id,
            &input.resources,
            &input.network,
            &input.env_vars,
            input.timeout_ms,
        ).await?;

        self.repository.save(&sandbox).await?;

        tracing::info!(sandbox_id = %sandbox_id, "Sandbox created successfully");
        Ok(sandbox)
    }
}
