//! Tool manager abstraction — adapters for installing tools in sandboxes.
//!
//! This module defines the `ToolManagerAdapter` trait and `ToolResolver`
//! that decides which adapter to use for a given capability request.

use std::collections::HashMap;

use crate::shared::DomainError;
use crate::shared::id::SandboxId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Request to prepare a sandbox with a specific capability.
#[derive(Debug, Clone)]
pub struct ToolchainRequest {
    pub sandbox_id: SandboxId,
    pub capability: String,
    pub constraints: HashMap<String, String>,
    pub strategy: ToolchainStrategy,
}

/// Strategy for toolchain resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolchainStrategy {
    /// Let the resolver pick the best approach.
    #[default]
    Auto,
    /// Prefer system package managers (apt, dnf, etc.).
    SystemPackage,
    /// Prefer version managers (asdf, sdkman, etc.).
    VersionManager,
    /// Use pre-packaged artifacts from content-addressed store.
    ContentAddressed,
}

impl ToolchainStrategy {
    /// Check if a given ManagerType matches this strategy.
    pub fn accepts(&self, mt: &ManagerType) -> bool {
        match self {
            ToolchainStrategy::Auto => true,
            ToolchainStrategy::SystemPackage => matches!(mt, ManagerType::Apt | ManagerType::Brew),
            ToolchainStrategy::VersionManager => {
                matches!(mt, ManagerType::Asdf | ManagerType::Sdkman)
            }
            ToolchainStrategy::ContentAddressed => matches!(mt, ManagerType::CaStore),
        }
    }
}

/// A step in a toolchain execution plan.
#[derive(Debug, Clone)]
pub struct ToolchainStep {
    pub description: String,
    pub command: String,
    pub env: HashMap<String, String>,
    pub timeout_ms: u64,
    pub expected_exit_code: i32,
}

/// A verification step to run after toolchain execution.
#[derive(Debug, Clone)]
pub struct ToolVerifyStep {
    pub label: String,
    pub command: String,
    pub expected_output_contains: Option<String>,
    pub expected_exit_code: i32,
}

/// A plan for preparing a toolchain in a sandbox.
#[derive(Debug, Clone)]
pub struct ToolchainPlan {
    pub capability: String,
    pub adapter_used: String,
    pub steps: Vec<ToolchainStep>,
    pub verification: Vec<ToolVerifyStep>,
    pub env: HashMap<String, String>,
    pub path_prefix: Vec<String>,
}

/// Result of a prepared environment.
#[derive(Debug, Clone)]
pub struct PreparedEnvironment {
    pub env_ref: String,
    pub capability: String,
    pub adapter_used: String,
    pub env: HashMap<String, String>,
    pub path_prefix: Vec<String>,
    pub verification_results: Vec<ToolVerifyResult>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ToolVerifyResult {
    pub label: String,
    pub passed: bool,
    pub output: Option<String>,
    pub duration_ms: u64,
}

/// Support level an adapter provides for a toolchain request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportLevel {
    /// Cannot handle this request at all.
    None,
    /// Can handle partially or with degraded quality.
    Partial,
    /// Fully supports this request.
    Full,
}

/// Tool manager type for preference ordering.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagerType {
    #[default]
    CaStore,
    Apt,
    Asdf,
    Sdkman,
    Brew,
    Nix,
}

/// A tool manager adapter — knows how to install specific tools.
///
/// Implementations: AptAdapter, AsdfAdapter, SdkmanAdapter, BrewAdapter, NixAdapter.
#[async_trait]
pub trait ToolManagerAdapter: Send + Sync {
    /// Unique identifier for this adapter.
    fn id(&self) -> &'static str;
    /// Human-readable name.
    fn name(&self) -> &'static str;
    /// Manager type for strategy filtering.
    fn manager_type(&self) -> ManagerType;
    /// Whether this adapter supports the given request.
    fn supports(&self, req: &ToolchainRequest) -> SupportLevel;
    /// Generate an installation plan for this request.
    async fn plan(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError>;
}

/// Resolves toolchain requests to the best adapter.
pub struct ToolResolver {
    adapters: Vec<Box<dyn ToolManagerAdapter>>,
}

impl ToolResolver {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    /// Register an adapter. Order matters — first registered gets priority.
    pub fn register(&mut self, adapter: Box<dyn ToolManagerAdapter>) {
        self.adapters.push(adapter);
    }

    /// Resolve the best adapter for a request and generate a plan.
    /// Respects the strategy field in ToolchainRequest to filter adapters.
    pub async fn resolve(&self, req: &ToolchainRequest) -> Result<ToolchainPlan, DomainError> {
        let strategy = &req.strategy;

        // Try to find an adapter with full support, filtered by strategy
        for adapter in &self.adapters {
            if strategy.accepts(&adapter.manager_type())
                && adapter.supports(req) == SupportLevel::Full
            {
                return adapter.plan(req).await;
            }
        }

        // Fallback: try any adapter with partial support, filtered by strategy
        for adapter in &self.adapters {
            if strategy.accepts(&adapter.manager_type())
                && adapter.supports(req) == SupportLevel::Partial
            {
                return adapter.plan(req).await;
            }
        }

        Err(DomainError::NotFound(format!(
            "No tool manager available for capability '{}' with strategy {:?}",
            req.capability, strategy
        )))
    }

    /// List registered adapter names.
    pub fn list_adapters(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.id()).collect()
    }
}

impl Default for ToolResolver {
    fn default() -> Self {
        Self::new()
    }
}
