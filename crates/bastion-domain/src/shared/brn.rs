//! Bastion Resource Name (BRN) types and capability-based authorization.
//!
//! BRN format: `brn:{namespace}:{type}[/{sub-type}]:{resource-id}`
//!
//! Example BRNs:
//! - `brn:sandbox:sandbox:sandbox_abc123`
//! - `brn:provider:instance:inst_uuid-here`
//! - `brn:catalog:doctor:firecracker-readiness`
//! - `brn:infra:pool:default`

use bitflags::bitflags;
use thiserror::Error;

// =============================================================================
// BrnNamespace
// =============================================================================

/// Closed enum for BRN namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrnNamespace {
    Sandbox,
    Project,
    Provider,
    Template,
    Catalog,
    Infra,
}

impl BrnNamespace {
    /// Parse a namespace from a string.
    pub fn parse(s: &str) -> Result<Self, BrnError> {
        match s {
            "sandbox" => Ok(BrnNamespace::Sandbox),
            "project" => Ok(BrnNamespace::Project),
            "provider" => Ok(BrnNamespace::Provider),
            "template" => Ok(BrnNamespace::Template),
            "catalog" => Ok(BrnNamespace::Catalog),
            "infra" => Ok(BrnNamespace::Infra),
            _ => Err(BrnError::UnknownNamespace(s.to_string())),
        }
    }

    /// Return the string representation of the namespace.
    pub fn as_str(&self) -> &'static str {
        match self {
            BrnNamespace::Sandbox => "sandbox",
            BrnNamespace::Project => "project",
            BrnNamespace::Provider => "provider",
            BrnNamespace::Template => "template",
            BrnNamespace::Catalog => "catalog",
            BrnNamespace::Infra => "infra",
        }
    }
}

// =============================================================================
// BrnType
// =============================================================================

/// Closed enum for BRN resource types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrnType {
    // sandbox
    Sandbox,
    // project
    Project,
    // provider
    Provider,
    Instance,
    // template
    Template,
    Artifact,
    // catalog
    Doctor,
    Assertion,
    Advice,
    Experience,
    // infra
    Pool,
    Worker,
    Config,
    Secret,
}

impl BrnType {
    /// Parse a resource type from a string.
    pub fn parse(s: &str) -> Result<Self, BrnError> {
        match s {
            "sandbox" => Ok(BrnType::Sandbox),
            "project" => Ok(BrnType::Project),
            "provider" => Ok(BrnType::Provider),
            "instance" => Ok(BrnType::Instance),
            "template" => Ok(BrnType::Template),
            "artifact" => Ok(BrnType::Artifact),
            "doctor" => Ok(BrnType::Doctor),
            "assertion" => Ok(BrnType::Assertion),
            "advice" => Ok(BrnType::Advice),
            "experience" => Ok(BrnType::Experience),
            "pool" => Ok(BrnType::Pool),
            "worker" => Ok(BrnType::Worker),
            "config" => Ok(BrnType::Config),
            "secret" => Ok(BrnType::Secret),
            _ => Err(BrnError::UnknownType(s.to_string())),
        }
    }

    /// Return the string representation of the resource type.
    pub fn as_str(&self) -> &'static str {
        match self {
            BrnType::Sandbox => "sandbox",
            BrnType::Project => "project",
            BrnType::Provider => "provider",
            BrnType::Instance => "instance",
            BrnType::Template => "template",
            BrnType::Artifact => "artifact",
            BrnType::Doctor => "doctor",
            BrnType::Assertion => "assertion",
            BrnType::Advice => "advice",
            BrnType::Experience => "experience",
            BrnType::Pool => "pool",
            BrnType::Worker => "worker",
            BrnType::Config => "config",
            BrnType::Secret => "secret",
        }
    }
}

// =============================================================================
// Brn
// =============================================================================

/// The main BRN struct representing a Bastion Resource Name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Brn {
    pub namespace: BrnNamespace,
    pub resource_type: BrnType,
    pub resource_id: String,
}

impl Brn {
    // ─── Factory methods ───────────────────────────────────────────────────────

    /// Create a sandbox BRN.
    pub fn sandbox(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Sandbox,
            resource_type: BrnType::Sandbox,
            resource_id: id.into(),
        }
    }

    /// Create a project BRN.
    pub fn project(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Project,
            resource_type: BrnType::Project,
            resource_id: id.into(),
        }
    }

    /// Create a provider BRN.
    pub fn provider(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Provider,
            resource_type: BrnType::Provider,
            resource_id: id.into(),
        }
    }

    /// Create a provider instance BRN.
    pub fn provider_instance(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Provider,
            resource_type: BrnType::Instance,
            resource_id: id.into(),
        }
    }

    /// Create a template BRN.
    pub fn template(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Template,
            resource_type: BrnType::Template,
            resource_id: id.into(),
        }
    }

    /// Create an artifact BRN.
    pub fn artifact(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Template,
            resource_type: BrnType::Artifact,
            resource_id: id.into(),
        }
    }

    /// Create a doctor BRN.
    pub fn doctor(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Catalog,
            resource_type: BrnType::Doctor,
            resource_id: id.into(),
        }
    }

    /// Create an assertion BRN.
    pub fn assertion(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Catalog,
            resource_type: BrnType::Assertion,
            resource_id: id.into(),
        }
    }

    /// Create an advice BRN.
    pub fn advice(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Catalog,
            resource_type: BrnType::Advice,
            resource_id: id.into(),
        }
    }

    /// Create an experience BRN.
    pub fn experience(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Catalog,
            resource_type: BrnType::Experience,
            resource_id: id.into(),
        }
    }

    /// Create the singleton infra pool BRN.
    pub fn pool() -> Self {
        Brn {
            namespace: BrnNamespace::Infra,
            resource_type: BrnType::Pool,
            resource_id: "default".to_string(),
        }
    }

    /// Create a worker BRN.
    pub fn worker(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Infra,
            resource_type: BrnType::Worker,
            resource_id: id.into(),
        }
    }

    /// Create the singleton infra config BRN.
    pub fn config() -> Self {
        Brn {
            namespace: BrnNamespace::Infra,
            resource_type: BrnType::Config,
            resource_id: "default".to_string(),
        }
    }

    /// Create a secret BRN.
    pub fn secret(id: impl Into<String>) -> Self {
        Brn {
            namespace: BrnNamespace::Infra,
            resource_type: BrnType::Secret,
            resource_id: id.into(),
        }
    }

    // ─── Parse/Display ────────────────────────────────────────────────────────

    /// Parse a BRN from a string.
    ///
    /// Format: `brn:{namespace}:{type}[/{sub-type}]:{resource-id}`
    ///
    /// The sub-type is currently unused but reserved for future extensibility.
    pub fn parse(s: &str) -> Result<Self, BrnError> {
        let s = s.trim();
        if !s.starts_with("brn:") {
            return Err(BrnError::InvalidFormat);
        }

        let rest = &s[4..]; // Remove "brn:" prefix

        // Split by ':' - format is ns:type:id or ns:type/subtype:id
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() != 3 {
            return Err(BrnError::InvalidFormat);
        }

        let namespace = BrnNamespace::parse(parts[0])?;
        let resource_type = BrnType::parse(parts[1])?;
        let resource_id = parts[2].to_string();

        if resource_id.is_empty() {
            return Err(BrnError::MissingResourceId);
        }

        Ok(Brn {
            namespace,
            resource_type,
            resource_id,
        })
    }

    /// Return the string representation of the BRN.
    pub fn to_string(&self) -> String {
        format!(
            "brn:{}:{}:{}",
            self.namespace.as_str(),
            self.resource_type.as_str(),
            self.resource_id
        )
    }

    // ─── Accessors ────────────────────────────────────────────────────────────

    /// Return the namespace.
    pub fn namespace(&self) -> BrnNamespace {
        self.namespace
    }

    /// Return the resource type.
    pub fn resource_type(&self) -> BrnType {
        self.resource_type
    }

    /// Return the resource ID.
    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    // ─── Query helpers ─────────────────────────────────────────────────────────

    /// Returns true if this is a sandbox BRN.
    pub fn is_sandbox(&self) -> bool {
        self.namespace == BrnNamespace::Sandbox
    }

    /// Returns true if this is a provider instance BRN.
    pub fn is_provider_instance(&self) -> bool {
        self.namespace == BrnNamespace::Provider && self.resource_type == BrnType::Instance
    }

    /// Returns true if this is an infra BRN (pool, worker, config, secret).
    pub fn is_infra(&self) -> bool {
        self.namespace == BrnNamespace::Infra
    }
}

impl std::fmt::Display for Brn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

// =============================================================================
// BrnError
// =============================================================================

/// Error type for BRN operations.
#[derive(Debug, Clone, Error)]
pub enum BrnError {
    #[error("Invalid BRN format: expected 'brn:ns:type:id'")]
    InvalidFormat,

    #[error("Unknown namespace: '{0}'")]
    UnknownNamespace(String),

    #[error("Unknown resource type: '{0}'")]
    UnknownType(String),

    #[error("Missing resource ID")]
    MissingResourceId,
}

// =============================================================================
// Capability Types
// =============================================================================

bitflags! {
    /// Set of capabilities controlling access per resource.
    ///
    /// | Capability | Scope |
    /// |-----------|-------|
    /// | `EXECUTION` | MCP — sandbox lifecycle, command execution |
    /// | `ADMIN` | REST — configuration, management, mutations |
    /// | `READONLY` | Both — queries, lists, status |
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CapabilitySet: u32 {
        const EXECUTION = 1 << 0;
        const ADMIN     = 1 << 1;
        const READONLY  = 1 << 2;
    }
}

/// Authentication context containing the principal and their capabilities.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub principal: String,
    pub capabilities: CapabilitySet,
}

impl AuthContext {
    /// Create a new authentication context.
    pub fn new(principal: impl Into<String>, capabilities: CapabilitySet) -> Self {
        AuthContext {
            principal: principal.into(),
            capabilities,
        }
    }
}

// =============================================================================
// Action
// =============================================================================

/// Action being performed on a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Execute,
}

// =============================================================================
// AuthError
// =============================================================================

/// Authorization error.
#[derive(Debug, Clone, Error)]
pub enum AuthError {
    #[error("Insufficient capabilities: required {required:?}, actual {actual:?}")]
    InsufficientCapabilities {
        required: CapabilitySet,
        actual: CapabilitySet,
    },
}

// =============================================================================
// CapabilityPolicy
// =============================================================================

/// Policy for capability-based authorization with hardcoded rules per ADR-0015.
///
/// Resource-to-capability mapping:
///
/// | BRN Pattern | EXECUTION | ADMIN | READONLY |
/// |-------------|:---------:|:-----:|:--------:|
/// | `brn:sandbox:*` | ✅ | - | - |
/// | `brn:project:*` | ✅ | ✅ | ✅ |
/// | `brn:provider:*` | - | ✅ | ✅ |
/// | `brn:template:*` | - | ✅ | ✅ |
/// | `brn:catalog:doctor:*` | - | ✅ (run) | ✅ (list) |
/// | `brn:catalog:experience:*` | - | - | ✅ |
/// | `brn:infra:*` | - | ✅ | ✅ |
pub struct CapabilityPolicy {
    _private: (),
}

impl CapabilityPolicy {
    /// Create a new capability policy.
    pub fn new() -> Self {
        CapabilityPolicy { _private: () }
    }

    /// Return the required capabilities for a given BRN and action.
    pub fn required_capabilities(&self, brn: &Brn, action: Action) -> CapabilitySet {
        use CapabilitySet as CS;

        // Special case: catalog:experience is always READONLY
        if brn.namespace == BrnNamespace::Catalog && brn.resource_type == BrnType::Experience {
            return CS::READONLY;
        }

        // sandbox:* → EXECUTION for all actions
        if brn.namespace == BrnNamespace::Sandbox {
            return CS::EXECUTION;
        }

        // project:* → EXECUTION|ADMIN for Write, READONLY for Read, EXECUTION for Execute
        if brn.namespace == BrnNamespace::Project {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::EXECUTION | CS::ADMIN,
                Action::Execute => CS::EXECUTION,
            }
        }
        // provider:* → ADMIN for Write, READONLY for Read
        else if brn.namespace == BrnNamespace::Provider {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::ADMIN,
                Action::Execute => CS::empty(), // No execution capability for provider
            }
        }
        // template:* → ADMIN for Write, READONLY for Read
        else if brn.namespace == BrnNamespace::Template {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::ADMIN,
                Action::Execute => CS::empty(),
            }
        }
        // catalog:doctor:* → ADMIN for Execute/Write, READONLY for Read
        else if brn.namespace == BrnNamespace::Catalog && brn.resource_type == BrnType::Doctor {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::ADMIN,
                Action::Execute => CS::ADMIN,
            }
        }
        // catalog:assertion/advice:* → ADMIN for Write, READONLY for Read
        else if brn.namespace == BrnNamespace::Catalog
            && (brn.resource_type == BrnType::Assertion || brn.resource_type == BrnType::Advice)
        {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::ADMIN,
                Action::Execute => CS::empty(),
            }
        }
        // infra:* → ADMIN for Write, READONLY for Read
        else if brn.namespace == BrnNamespace::Infra {
            match action {
                Action::Read => CS::READONLY,
                Action::Write => CS::ADMIN,
                Action::Execute => CS::empty(),
            }
        }
        // Default: no capabilities required
        else {
            CS::empty()
        }
    }

    /// Authorize an authentication context for a given BRN and action.
    pub fn authorize(&self, auth: &AuthContext, brn: &Brn, action: Action) -> Result<(), AuthError> {
        let required = self.required_capabilities(brn, action);

        if auth.capabilities.contains(required) {
            Ok(())
        } else {
            Err(AuthError::InsufficientCapabilities {
                required,
                actual: auth.capabilities,
            })
        }
    }
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BrnNamespace tests ───────────────────────────────────────────────────

    #[test]
    fn fn_brn_namespace_parse_valid() {
        assert_eq!(BrnNamespace::parse("sandbox").unwrap(), BrnNamespace::Sandbox);
        assert_eq!(BrnNamespace::parse("project").unwrap(), BrnNamespace::Project);
        assert_eq!(BrnNamespace::parse("provider").unwrap(), BrnNamespace::Provider);
        assert_eq!(BrnNamespace::parse("template").unwrap(), BrnNamespace::Template);
        assert_eq!(BrnNamespace::parse("catalog").unwrap(), BrnNamespace::Catalog);
        assert_eq!(BrnNamespace::parse("infra").unwrap(), BrnNamespace::Infra);
    }

    #[test]
    fn fn_brn_namespace_parse_invalid() {
        assert!(matches!(
            BrnNamespace::parse("unknown"),
            Err(BrnError::UnknownNamespace(s)) if s == "unknown"
        ));
        assert!(matches!(
            BrnNamespace::parse("SANDBOX"),
            Err(BrnError::UnknownNamespace(s)) if s == "SANDBOX"
        ));
    }

    #[test]
    fn fn_brn_namespace_as_str() {
        assert_eq!(BrnNamespace::Sandbox.as_str(), "sandbox");
        assert_eq!(BrnNamespace::Project.as_str(), "project");
        assert_eq!(BrnNamespace::Provider.as_str(), "provider");
        assert_eq!(BrnNamespace::Template.as_str(), "template");
        assert_eq!(BrnNamespace::Catalog.as_str(), "catalog");
        assert_eq!(BrnNamespace::Infra.as_str(), "infra");
    }

    // ─── BrnType tests ────────────────────────────────────────────────────────

    #[test]
    fn fn_brn_type_parse_valid() {
        assert_eq!(BrnType::parse("sandbox").unwrap(), BrnType::Sandbox);
        assert_eq!(BrnType::parse("project").unwrap(), BrnType::Project);
        assert_eq!(BrnType::parse("provider").unwrap(), BrnType::Provider);
        assert_eq!(BrnType::parse("instance").unwrap(), BrnType::Instance);
        assert_eq!(BrnType::parse("template").unwrap(), BrnType::Template);
        assert_eq!(BrnType::parse("artifact").unwrap(), BrnType::Artifact);
        assert_eq!(BrnType::parse("doctor").unwrap(), BrnType::Doctor);
        assert_eq!(BrnType::parse("assertion").unwrap(), BrnType::Assertion);
        assert_eq!(BrnType::parse("advice").unwrap(), BrnType::Advice);
        assert_eq!(BrnType::parse("experience").unwrap(), BrnType::Experience);
        assert_eq!(BrnType::parse("pool").unwrap(), BrnType::Pool);
        assert_eq!(BrnType::parse("worker").unwrap(), BrnType::Worker);
        assert_eq!(BrnType::parse("config").unwrap(), BrnType::Config);
        assert_eq!(BrnType::parse("secret").unwrap(), BrnType::Secret);
    }

    #[test]
    fn fn_brn_type_parse_invalid() {
        assert!(matches!(
            BrnType::parse("unknown"),
            Err(BrnError::UnknownType(s)) if s == "unknown"
        ));
    }

    #[test]
    fn fn_brn_type_as_str() {
        assert_eq!(BrnType::Sandbox.as_str(), "sandbox");
        assert_eq!(BrnType::Project.as_str(), "project");
        assert_eq!(BrnType::Provider.as_str(), "provider");
        assert_eq!(BrnType::Instance.as_str(), "instance");
        assert_eq!(BrnType::Template.as_str(), "template");
        assert_eq!(BrnType::Artifact.as_str(), "artifact");
        assert_eq!(BrnType::Doctor.as_str(), "doctor");
        assert_eq!(BrnType::Assertion.as_str(), "assertion");
        assert_eq!(BrnType::Advice.as_str(), "advice");
        assert_eq!(BrnType::Experience.as_str(), "experience");
        assert_eq!(BrnType::Pool.as_str(), "pool");
        assert_eq!(BrnType::Worker.as_str(), "worker");
        assert_eq!(BrnType::Config.as_str(), "config");
        assert_eq!(BrnType::Secret.as_str(), "secret");
    }

    // ─── Brn factory method tests ─────────────────────────────────────────────

    #[test]
    fn fn_brn_factory_sandbox() {
        let brn = Brn::sandbox("abc123");
        assert_eq!(brn.namespace, BrnNamespace::Sandbox);
        assert_eq!(brn.resource_type, BrnType::Sandbox);
        assert_eq!(brn.resource_id, "abc123");
    }

    #[test]
    fn fn_brn_factory_project() {
        let brn = Brn::project("proj1");
        assert_eq!(brn.namespace, BrnNamespace::Project);
        assert_eq!(brn.resource_type, BrnType::Project);
        assert_eq!(brn.resource_id, "proj1");
    }

    #[test]
    fn fn_brn_factory_provider() {
        let brn = Brn::provider("prov1");
        assert_eq!(brn.namespace, BrnNamespace::Provider);
        assert_eq!(brn.resource_type, BrnType::Provider);
        assert_eq!(brn.resource_id, "prov1");
    }

    #[test]
    fn fn_brn_factory_provider_instance() {
        let brn = Brn::provider_instance("inst1");
        assert_eq!(brn.namespace, BrnNamespace::Provider);
        assert_eq!(brn.resource_type, BrnType::Instance);
        assert_eq!(brn.resource_id, "inst1");
    }

    #[test]
    fn fn_brn_factory_template() {
        let brn = Brn::template("tpl1");
        assert_eq!(brn.namespace, BrnNamespace::Template);
        assert_eq!(brn.resource_type, BrnType::Template);
        assert_eq!(brn.resource_id, "tpl1");
    }

    #[test]
    fn fn_brn_factory_artifact() {
        let brn = Brn::artifact("art1");
        assert_eq!(brn.namespace, BrnNamespace::Template);
        assert_eq!(brn.resource_type, BrnType::Artifact);
        assert_eq!(brn.resource_id, "art1");
    }

    #[test]
    fn fn_brn_factory_doctor() {
        let brn = Brn::doctor("doc1");
        assert_eq!(brn.namespace, BrnNamespace::Catalog);
        assert_eq!(brn.resource_type, BrnType::Doctor);
        assert_eq!(brn.resource_id, "doc1");
    }

    #[test]
    fn fn_brn_factory_assertion() {
        let brn = Brn::assertion("assert1");
        assert_eq!(brn.namespace, BrnNamespace::Catalog);
        assert_eq!(brn.resource_type, BrnType::Assertion);
        assert_eq!(brn.resource_id, "assert1");
    }

    #[test]
    fn fn_brn_factory_advice() {
        let brn = Brn::advice("adv1");
        assert_eq!(brn.namespace, BrnNamespace::Catalog);
        assert_eq!(brn.resource_type, BrnType::Advice);
        assert_eq!(brn.resource_id, "adv1");
    }

    #[test]
    fn fn_brn_factory_experience() {
        let brn = Brn::experience("exp1");
        assert_eq!(brn.namespace, BrnNamespace::Catalog);
        assert_eq!(brn.resource_type, BrnType::Experience);
        assert_eq!(brn.resource_id, "exp1");
    }

    #[test]
    fn fn_brn_factory_pool() {
        let brn = Brn::pool();
        assert_eq!(brn.namespace, BrnNamespace::Infra);
        assert_eq!(brn.resource_type, BrnType::Pool);
        assert_eq!(brn.resource_id, "default");
    }

    #[test]
    fn fn_brn_factory_worker() {
        let brn = Brn::worker("w1");
        assert_eq!(brn.namespace, BrnNamespace::Infra);
        assert_eq!(brn.resource_type, BrnType::Worker);
        assert_eq!(brn.resource_id, "w1");
    }

    #[test]
    fn fn_brn_factory_config() {
        let brn = Brn::config();
        assert_eq!(brn.namespace, BrnNamespace::Infra);
        assert_eq!(brn.resource_type, BrnType::Config);
        assert_eq!(brn.resource_id, "default");
    }

    #[test]
    fn fn_brn_factory_secret() {
        let brn = Brn::secret("sec1");
        assert_eq!(brn.namespace, BrnNamespace::Infra);
        assert_eq!(brn.resource_type, BrnType::Secret);
        assert_eq!(brn.resource_id, "sec1");
    }

    // ─── Brn parse/display tests ──────────────────────────────────────────────

    #[test]
    fn fn_brn_parse_valid() {
        let brn = Brn::parse("brn:sandbox:sandbox:sandbox_abc123").unwrap();
        assert_eq!(brn.namespace, BrnNamespace::Sandbox);
        assert_eq!(brn.resource_type, BrnType::Sandbox);
        assert_eq!(brn.resource_id, "sandbox_abc123");

        let brn = Brn::parse("brn:provider:instance:inst_uuid-here").unwrap();
        assert_eq!(brn.namespace, BrnNamespace::Provider);
        assert_eq!(brn.resource_type, BrnType::Instance);
        assert_eq!(brn.resource_id, "inst_uuid-here");

        let brn = Brn::parse("brn:catalog:doctor:firecracker-readiness").unwrap();
        assert_eq!(brn.namespace, BrnNamespace::Catalog);
        assert_eq!(brn.resource_type, BrnType::Doctor);
        assert_eq!(brn.resource_id, "firecracker-readiness");

        let brn = Brn::parse("brn:infra:pool:default").unwrap();
        assert_eq!(brn.namespace, BrnNamespace::Infra);
        assert_eq!(brn.resource_type, BrnType::Pool);
        assert_eq!(brn.resource_id, "default");
    }

    #[test]
    fn fn_brn_parse_invalid_format() {
        assert!(matches!(Brn::parse(""), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("brn"), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("brn:"), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("brn:sandbox"), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("brn:sandbox:sandbox"), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("notbrn:sandbox:sandbox:id"), Err(BrnError::InvalidFormat)));
        assert!(matches!(Brn::parse("BRN:sandbox:sandbox:id"), Err(BrnError::InvalidFormat)));
    }

    #[test]
    fn fn_brn_parse_invalid_namespace() {
        assert!(matches!(
            Brn::parse("brn:unknown:sandbox:id"),
            Err(BrnError::UnknownNamespace(s)) if s == "unknown"
        ));
    }

    #[test]
    fn fn_brn_parse_invalid_type() {
        assert!(matches!(
            Brn::parse("brn:sandbox:unknown:id"),
            Err(BrnError::UnknownType(s)) if s == "unknown"
        ));
    }

    #[test]
    fn fn_brn_parse_missing_resource_id() {
        assert!(matches!(Brn::parse("brn:sandbox:sandbox:"), Err(BrnError::MissingResourceId)));
    }

    #[test]
    fn fn_brn_to_string() {
        let brn = Brn::sandbox("abc123");
        assert_eq!(brn.to_string(), "brn:sandbox:sandbox:abc123");

        let brn = Brn::parse("brn:provider:instance:inst1").unwrap();
        assert_eq!(brn.to_string(), "brn:provider:instance:inst1");
    }

    #[test]
    fn fn_brn_display() {
        let brn = Brn::pool();
        assert_eq!(format!("{}", brn), "brn:infra:pool:default");
    }

    // ─── Brn accessor tests ───────────────────────────────────────────────────

    #[test]
    fn fn_brn_accessors() {
        let brn = Brn::doctor("doc1");
        assert_eq!(brn.namespace(), BrnNamespace::Catalog);
        assert_eq!(brn.resource_type(), BrnType::Doctor);
        assert_eq!(brn.resource_id(), "doc1");
    }

    // ─── Brn query helper tests ───────────────────────────────────────────────

    #[test]
    fn fn_brn_is_sandbox() {
        assert!(Brn::sandbox("x").is_sandbox());
        assert!(!Brn::project("x").is_sandbox());
        assert!(!Brn::provider("x").is_sandbox());
    }

    #[test]
    fn fn_brn_is_provider_instance() {
        assert!(Brn::provider_instance("x").is_provider_instance());
        assert!(!Brn::provider("x").is_provider_instance());
        assert!(!Brn::sandbox("x").is_provider_instance());
    }

    #[test]
    fn fn_brn_is_infra() {
        assert!(Brn::pool().is_infra());
        assert!(Brn::worker("x").is_infra());
        assert!(Brn::config().is_infra());
        assert!(Brn::secret("x").is_infra());
        assert!(!Brn::sandbox("x").is_infra());
    }

    // ─── CapabilitySet tests ──────────────────────────────────────────────────

    #[test]
    fn fn_capability_set_operations() {
        let exec = CapabilitySet::EXECUTION;
        let admin = CapabilitySet::ADMIN;
        let readonly = CapabilitySet::READONLY;

        // Union
        let combined = exec | admin;
        assert!(combined.contains(exec));
        assert!(combined.contains(admin));
        assert!(!combined.contains(readonly));

        // Intersection
        let both = exec | admin;
        let intersected = both & exec;
        assert!(intersected.contains(exec));
        assert!(!intersected.contains(admin));

        // Empty
        let empty = CapabilitySet::empty();
        assert!(!empty.contains(exec));

        // Contains
        assert!(CapabilitySet::EXECUTION.contains(CapabilitySet::EXECUTION));
    }

    // ─── AuthContext tests ────────────────────────────────────────────────────

    #[test]
    fn fn_auth_context_new() {
        let auth = AuthContext::new("user1", CapabilitySet::EXECUTION);
        assert_eq!(auth.principal, "user1");
        assert!(auth.capabilities.contains(CapabilitySet::EXECUTION));
    }

    // ─── CapabilityPolicy tests ───────────────────────────────────────────────

    #[test]
    fn fn_policy_sandbox_requires_execution() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::sandbox("sb1");

        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::EXECUTION);
        assert_eq!(policy.required_capabilities(&brn, Action::Write), CapabilitySet::EXECUTION);
        assert_eq!(policy.required_capabilities(&brn, Action::Execute), CapabilitySet::EXECUTION);
    }

    #[test]
    fn fn_policy_project_rules() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::project("proj1");

        // Read → READONLY
        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::READONLY);
        // Write → EXECUTION|ADMIN
        assert_eq!(
            policy.required_capabilities(&brn, Action::Write),
            CapabilitySet::EXECUTION | CapabilitySet::ADMIN
        );
        // Execute → EXECUTION
        assert_eq!(policy.required_capabilities(&brn, Action::Execute), CapabilitySet::EXECUTION);
    }

    #[test]
    fn fn_policy_provider_rules() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::provider("prov1");

        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&brn, Action::Write), CapabilitySet::ADMIN);
        assert_eq!(policy.required_capabilities(&brn, Action::Execute), CapabilitySet::empty());
    }

    #[test]
    fn fn_policy_template_rules() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::template("tpl1");

        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&brn, Action::Write), CapabilitySet::ADMIN);
    }

    #[test]
    fn fn_policy_catalog_doctor_rules() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::doctor("doc1");

        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&brn, Action::Write), CapabilitySet::ADMIN);
        assert_eq!(policy.required_capabilities(&brn, Action::Execute), CapabilitySet::ADMIN);
    }

    #[test]
    fn fn_policy_catalog_experience_readonly() {
        let policy = CapabilityPolicy::new();
        let brn = Brn::experience("exp1");

        // Experience is always READONLY regardless of action
        assert_eq!(policy.required_capabilities(&brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&brn, Action::Write), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&brn, Action::Execute), CapabilitySet::READONLY);
    }

    #[test]
    fn fn_policy_infra_rules() {
        let policy = CapabilityPolicy::new();

        let pool_brn = Brn::pool();
        assert_eq!(policy.required_capabilities(&pool_brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&pool_brn, Action::Write), CapabilitySet::ADMIN);

        let worker_brn = Brn::worker("w1");
        assert_eq!(policy.required_capabilities(&worker_brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&worker_brn, Action::Write), CapabilitySet::ADMIN);

        let config_brn = Brn::config();
        assert_eq!(policy.required_capabilities(&config_brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&config_brn, Action::Write), CapabilitySet::ADMIN);

        let secret_brn = Brn::secret("sec1");
        assert_eq!(policy.required_capabilities(&secret_brn, Action::Read), CapabilitySet::READONLY);
        assert_eq!(policy.required_capabilities(&secret_brn, Action::Write), CapabilitySet::ADMIN);
    }

    // ─── Policy authorization tests ──────────────────────────────────────────

    #[test]
    fn fn_policy_authorize_success() {
        let policy = CapabilityPolicy::new();
        let auth = AuthContext::new("user1", CapabilitySet::EXECUTION);
        let brn = Brn::sandbox("sb1");

        assert!(policy.authorize(&auth, &brn, Action::Execute).is_ok());
    }

    #[test]
    fn fn_policy_authorize_insufficient_capabilities() {
        let policy = CapabilityPolicy::new();
        let auth = AuthContext::new("user1", CapabilitySet::READONLY);
        let brn = Brn::sandbox("sb1");

        let result = policy.authorize(&auth, &brn, Action::Execute);
        assert!(matches!(
            result,
            Err(AuthError::InsufficientCapabilities { required, actual })
            if required == CapabilitySet::EXECUTION && actual == CapabilitySet::READONLY
        ));
    }

    #[test]
    fn fn_policy_authorize_experience_always_readonly() {
        let policy = CapabilityPolicy::new();
        let auth = AuthContext::new("user1", CapabilitySet::READONLY);
        let brn = Brn::experience("exp1");

        // Should succeed because experience requires only READONLY
        assert!(policy.authorize(&auth, &brn, Action::Read).is_ok());
        assert!(policy.authorize(&auth, &brn, Action::Write).is_ok());
        assert!(policy.authorize(&auth, &brn, Action::Execute).is_ok());

        // ADMIN-only user cannot read experience (ADMIN doesn't include READONLY)
        let admin_auth = AuthContext::new("user2", CapabilitySet::ADMIN);
        let result = policy.authorize(&admin_auth, &brn, Action::Read);
        assert!(result.is_err());
        match result {
            Err(AuthError::InsufficientCapabilities { required, actual }) => {
                assert_eq!(required, CapabilitySet::READONLY);
                assert_eq!(actual, CapabilitySet::ADMIN);
            }
            _ => panic!("Expected InsufficientCapabilities error"),
        }

        // User with READONLY|ADMIN can access experience
        let full_auth = AuthContext::new("user3", CapabilitySet::READONLY | CapabilitySet::ADMIN);
        assert!(policy.authorize(&full_auth, &brn, Action::Read).is_ok());
    }

    // ─── Brn equality tests ───────────────────────────────────────────────────

    #[test]
    fn fn_brn_equality() {
        let brn1 = Brn::sandbox("abc");
        let brn2 = Brn::sandbox("abc");
        let brn3 = Brn::sandbox("xyz");

        assert_eq!(brn1, brn2);
        assert_ne!(brn1, brn3);
    }

    #[test]
    fn fn_brn_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Brn::sandbox("abc"));
        set.insert(Brn::sandbox("abc")); // Duplicate
        set.insert(Brn::sandbox("xyz"));
        assert_eq!(set.len(), 2);
    }

    // ─── Error Clone tests ───────────────────────────────────────────────────

    #[test]
    fn fn_brn_error_clone() {
        let err = BrnError::UnknownNamespace("test".to_string());
        let cloned = err.clone();
        assert_eq!(format!("{}", err), format!("{}", cloned));
    }

    #[test]
    fn fn_auth_error_clone() {
        let err = AuthError::InsufficientCapabilities {
            required: CapabilitySet::EXECUTION,
            actual: CapabilitySet::READONLY,
        };
        let cloned = err.clone();
        assert_eq!(format!("{}", err), format!("{}", cloned));
    }

    // ─── BrnNamespace Eq + Hash ──────────────────────────────────────────────

    #[test]
    fn fn_brn_namespace_eq() {
        assert_eq!(BrnNamespace::Sandbox, BrnNamespace::Sandbox);
        assert_ne!(BrnNamespace::Sandbox, BrnNamespace::Project);
    }

    #[test]
    fn fn_brn_namespace_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(BrnNamespace::Sandbox);
        set.insert(BrnNamespace::Sandbox);
        set.insert(BrnNamespace::Project);
        assert_eq!(set.len(), 2);
    }

    // ─── BrnType Eq + Hash ───────────────────────────────────────────────────

    #[test]
    fn fn_brn_type_eq() {
        assert_eq!(BrnType::Sandbox, BrnType::Sandbox);
        assert_ne!(BrnType::Sandbox, BrnType::Project);
    }

    #[test]
    fn fn_brn_type_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(BrnType::Sandbox);
        set.insert(BrnType::Sandbox);
        set.insert(BrnType::Project);
        assert_eq!(set.len(), 2);
    }
}
