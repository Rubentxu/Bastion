//! Provider capabilities value object.

use serde::{Deserialize, Serialize};

/// Capabilities reported by a provider backend.
///
/// Used for provider selection and feature gating.
///
/// # Validation
/// All numeric limits (max_timeout_ms, max_memory_mb, max_cpu_count, avg_startup_ms)
/// must be greater than zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Whether snapshots are supported.
    pub(crate) supports_snapshots: bool,
    /// Whether streaming is supported.
    pub(crate) supports_streaming: bool,
    /// Whether pause/resume is supported.
    pub(crate) supports_pause_resume: bool,
    /// Maximum timeout in milliseconds.
    pub(crate) max_timeout_ms: u64,
    /// Maximum memory in megabytes.
    pub(crate) max_memory_mb: u64,
    /// Maximum CPU count.
    pub(crate) max_cpu_count: u32,
    /// Whether networking is supported.
    pub(crate) supports_networking: bool,
    /// Whether KVM is required.
    pub(crate) requires_kvm: bool,
    /// Average startup time in milliseconds.
    pub(crate) avg_startup_ms: u32,
}

impl ProviderCapabilities {
    /// Create a new ProviderCapabilities with default values (no features supported).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a ProviderCapabilities with validation.
    ///
    /// # Errors
    /// Returns an error if any numeric limit is zero.
    pub fn try_new(
        supports_snapshots: bool,
        supports_streaming: bool,
        supports_pause_resume: bool,
        max_timeout_ms: u64,
        max_memory_mb: u64,
        max_cpu_count: u32,
        supports_networking: bool,
        requires_kvm: bool,
        avg_startup_ms: u32,
    ) -> Result<Self, ValidationError> {
        if max_timeout_ms == 0 {
            return Err(ValidationError::ZeroValue("max_timeout_ms".into()));
        }
        if max_memory_mb == 0 {
            return Err(ValidationError::ZeroValue("max_memory_mb".into()));
        }
        if max_cpu_count == 0 {
            return Err(ValidationError::ZeroValue("max_cpu_count".into()));
        }
        if avg_startup_ms == 0 {
            return Err(ValidationError::ZeroValue("avg_startup_ms".into()));
        }
        Ok(Self {
            supports_snapshots,
            supports_streaming,
            supports_pause_resume,
            max_timeout_ms,
            max_memory_mb,
            max_cpu_count,
            supports_networking,
            requires_kvm,
            avg_startup_ms,
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    #[inline]
    pub fn supports_snapshots(&self) -> bool {
        self.supports_snapshots
    }

    #[inline]
    pub fn supports_streaming(&self) -> bool {
        self.supports_streaming
    }

    #[inline]
    pub fn supports_pause_resume(&self) -> bool {
        self.supports_pause_resume
    }

    #[inline]
    pub fn max_timeout_ms(&self) -> u64 {
        self.max_timeout_ms
    }

    #[inline]
    pub fn max_memory_mb(&self) -> u64 {
        self.max_memory_mb
    }

    #[inline]
    pub fn max_cpu_count(&self) -> u32 {
        self.max_cpu_count
    }

    #[inline]
    pub fn supports_networking(&self) -> bool {
        self.supports_networking
    }

    #[inline]
    pub fn requires_kvm(&self) -> bool {
        self.requires_kvm
    }

    #[inline]
    pub fn avg_startup_ms(&self) -> u32 {
        self.avg_startup_ms
    }

    // ── Builder methods ────────────────────────────────────────────────────────

    /// Set snapshot support.
    pub fn with_supports_snapshots(mut self, value: bool) -> Self {
        self.supports_snapshots = value;
        self
    }

    /// Set streaming support.
    pub fn with_supports_streaming(mut self, value: bool) -> Self {
        self.supports_streaming = value;
        self
    }

    /// Set pause/resume support.
    pub fn with_supports_pause_resume(mut self, value: bool) -> Self {
        self.supports_pause_resume = value;
        self
    }

    /// Set maximum timeout in milliseconds.
    pub fn with_max_timeout_ms(mut self, value: u64) -> Self {
        self.max_timeout_ms = value;
        self
    }

    /// Set maximum memory in megabytes.
    pub fn with_max_memory_mb(mut self, value: u64) -> Self {
        self.max_memory_mb = value;
        self
    }

    /// Set maximum CPU count.
    pub fn with_max_cpu_count(mut self, value: u32) -> Self {
        self.max_cpu_count = value;
        self
    }

    /// Set networking support.
    pub fn with_supports_networking(mut self, value: bool) -> Self {
        self.supports_networking = value;
        self
    }

    /// Set KVM requirement.
    pub fn with_requires_kvm(mut self, value: bool) -> Self {
        self.requires_kvm = value;
        self
    }

    /// Set average startup time in milliseconds.
    pub fn with_avg_startup_ms(mut self, value: u32) -> Self {
        self.avg_startup_ms = value;
        self
    }

    // ── Query methods ─────────────────────────────────────────────────────────

    /// Returns true if this provider supports all the required capabilities.
    ///
    /// For boolean capabilities: if not required, returns true; otherwise checks if supported.
    /// For numeric capabilities: compares against required thresholds.
    pub fn supports_all(&self, required: &ProviderCapabilities) -> bool {
        (!required.supports_snapshots || self.supports_snapshots)
            && (!required.supports_streaming || self.supports_streaming)
            && (!required.supports_pause_resume || self.supports_pause_resume)
            && self.max_timeout_ms >= required.max_timeout_ms
            && self.max_memory_mb >= required.max_memory_mb
            && self.max_cpu_count >= required.max_cpu_count
            && (!required.supports_networking || self.supports_networking)
            && (!required.requires_kvm || self.requires_kvm)
            && self.avg_startup_ms <= required.avg_startup_ms
    }
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            supports_snapshots: false,
            supports_streaming: true,
            supports_pause_resume: false,
            max_timeout_ms: 86_400_000,
            max_memory_mb: 16_384,
            max_cpu_count: 16,
            supports_networking: true,
            requires_kvm: false,
            avg_startup_ms: 1500,
        }
    }
}

/// Validation error for ProviderCapabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// A numeric field that must be positive had a zero value.
    ZeroValue(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ZeroValue(field) => {
                write!(f, "{} must be greater than zero", field)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_default_values() {
        let caps = ProviderCapabilities::default();
        assert!(!caps.supports_snapshots());
        assert!(caps.supports_streaming());
        assert!(!caps.supports_pause_resume());
        assert_eq!(caps.max_timeout_ms(), 86_400_000);
        assert_eq!(caps.max_memory_mb(), 16_384);
        assert_eq!(caps.max_cpu_count(), 16);
        assert!(caps.supports_networking());
        assert!(!caps.requires_kvm());
        assert_eq!(caps.avg_startup_ms(), 1500);
    }

    // ── try_new validation tests ──────────────────────────────────────────────

    #[test]
    fn test_try_new_valid() {
        let caps = ProviderCapabilities::try_new(
            true,  // supports_snapshots
            false, // supports_streaming
            true,  // supports_pause_resume
            60_000,        // max_timeout_ms
            8192,          // max_memory_mb
            8,             // max_cpu_count
            true,          // supports_networking
            false,         // requires_kvm
            2000,          // avg_startup_ms
        )
        .unwrap();
        assert!(caps.supports_snapshots());
        assert!(!caps.supports_streaming());
        assert!(caps.supports_pause_resume());
        assert_eq!(caps.max_timeout_ms(), 60_000);
        assert_eq!(caps.max_memory_mb(), 8192);
        assert_eq!(caps.max_cpu_count(), 8);
        assert!(caps.supports_networking());
        assert!(!caps.requires_kvm());
        assert_eq!(caps.avg_startup_ms(), 2000);
    }

    #[test]
    fn test_try_new_zero_timeout_fails() {
        let result = ProviderCapabilities::try_new(
            false, true, false,
            0,     // max_timeout_ms = 0 (invalid)
            8192, 8192, true, false, 1500,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            ValidationError::ZeroValue("max_timeout_ms".into())
        );
    }

    #[test]
    fn test_try_new_zero_memory_fails() {
        let result = ProviderCapabilities::try_new(
            false, true, false,
            60_000, 0, 8, true, false, 1500,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            ValidationError::ZeroValue("max_memory_mb".into())
        );
    }

    #[test]
    fn test_try_new_zero_cpu_count_fails() {
        let result = ProviderCapabilities::try_new(
            false, true, false,
            60_000, 8192, 0, true, false, 1500,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            ValidationError::ZeroValue("max_cpu_count".into())
        );
    }

    #[test]
    fn test_try_new_zero_startup_fails() {
        let result = ProviderCapabilities::try_new(
            false, true, false,
            60_000, 8192, 8, true, false, 0,
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            ValidationError::ZeroValue("avg_startup_ms".into())
        );
    }

    // ── Builder method tests ──────────────────────────────────────────────────

    #[test]
    fn test_builder_supports_snapshots() {
        let caps = ProviderCapabilities::default().with_supports_snapshots(true);
        assert!(caps.supports_snapshots());
    }

    #[test]
    fn test_builder_supports_streaming() {
        let caps = ProviderCapabilities::default().with_supports_streaming(false);
        assert!(!caps.supports_streaming());
    }

    #[test]
    fn test_builder_supports_pause_resume() {
        let caps = ProviderCapabilities::default().with_supports_pause_resume(true);
        assert!(caps.supports_pause_resume());
    }

    #[test]
    fn test_builder_max_timeout_ms() {
        let caps = ProviderCapabilities::default().with_max_timeout_ms(30_000);
        assert_eq!(caps.max_timeout_ms(), 30_000);
    }

    #[test]
    fn test_builder_max_memory_mb() {
        let caps = ProviderCapabilities::default().with_max_memory_mb(4096);
        assert_eq!(caps.max_memory_mb(), 4096);
    }

    #[test]
    fn test_builder_max_cpu_count() {
        let caps = ProviderCapabilities::default().with_max_cpu_count(4);
        assert_eq!(caps.max_cpu_count(), 4);
    }

    #[test]
    fn test_builder_supports_networking() {
        let caps = ProviderCapabilities::default().with_supports_networking(false);
        assert!(!caps.supports_networking());
    }

    #[test]
    fn test_builder_requires_kvm() {
        let caps = ProviderCapabilities::default().with_requires_kvm(true);
        assert!(caps.requires_kvm());
    }

    #[test]
    fn test_builder_avg_startup_ms() {
        let caps = ProviderCapabilities::default().with_avg_startup_ms(3000);
        assert_eq!(caps.avg_startup_ms(), 3000);
    }

    #[test]
    fn test_builder_chained() {
        let caps = ProviderCapabilities::default()
            .with_supports_snapshots(true)
            .with_supports_streaming(false)
            .with_supports_pause_resume(true)
            .with_max_timeout_ms(120_000)
            .with_max_memory_mb(4096)
            .with_max_cpu_count(4)
            .with_supports_networking(false)
            .with_requires_kvm(true)
            .with_avg_startup_ms(2500);
        assert!(caps.supports_snapshots());
        assert!(!caps.supports_streaming());
        assert!(caps.supports_pause_resume());
        assert_eq!(caps.max_timeout_ms(), 120_000);
        assert_eq!(caps.max_memory_mb(), 4096);
        assert_eq!(caps.max_cpu_count(), 4);
        assert!(!caps.supports_networking());
        assert!(caps.requires_kvm());
        assert_eq!(caps.avg_startup_ms(), 2500);
    }

    // ── supports_all tests ────────────────────────────────────────────────────

    #[test]
    fn test_supports_all_empty_required() {
        let caps = ProviderCapabilities::default();
        let required = ProviderCapabilities::default();
        assert!(caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_all_features() {
        let caps = ProviderCapabilities::try_new(
            true, true, true,
            120_000, 16384, 32,
            true, true, 1000,
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            true, true, true,
            60_000, 8192, 16,
            true, true, 2000,
        )
        .unwrap();
        assert!(caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_insufficient_timeout() {
        let caps = ProviderCapabilities::try_new(
            true, true, true,
            30_000, 8192, 16,  // timeout too low
            true, true, 2000,
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            false, false, false,
            60_000, 1, 1,  // requires 60s timeout
            false, false, 1,
        )
        .unwrap();
        assert!(!caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_insufficient_memory() {
        let caps = ProviderCapabilities::try_new(
            true, true, true,
            60_000, 4096, 16,  // memory too low
            true, true, 2000,
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            false, false, false,
            1, 8192, 1,  // requires 8192 MB
            false, false, 1,
        )
        .unwrap();
        assert!(!caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_insufficient_cpu() {
        let caps = ProviderCapabilities::try_new(
            true, true, true,
            60_000, 8192, 4,  // CPU too low
            true, true, 2000,
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            false, false, false,
            1, 1, 16,  // requires 16 CPUs
            false, false, 1,
        )
        .unwrap();
        assert!(!caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_slower_startup() {
        let caps = ProviderCapabilities::try_new(
            true, true, true,
            60_000, 8192, 16,
            true, true, 3000,  // slower than required
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            false, false, false,
            1, 1, 1,
            false, false, 2000,  // requires faster startup
        )
        .unwrap();
        assert!(!caps.supports_all(&required));
    }

    #[test]
    fn test_supports_all_missing_snapshot_support() {
        let caps = ProviderCapabilities::try_new(
            false, true, true,  // no snapshot support
            60_000, 8192, 16,
            true, true, 2000,
        )
        .unwrap();
        let required = ProviderCapabilities::try_new(
            true, false, false,  // requires snapshot
            1, 1, 1,
            false, false, 1,
        )
        .unwrap();
        assert!(!caps.supports_all(&required));
    }

    // ── Serde tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_serde_roundtrip() {
        let caps = ProviderCapabilities::try_new(
            true, false, true,
            60_000, 8192, 8,
            true, false, 2500,
        )
        .unwrap();
        let json = serde_json::to_string(&caps).unwrap();
        let parsed: ProviderCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.supports_snapshots(), caps.supports_snapshots());
        assert_eq!(parsed.supports_streaming(), caps.supports_streaming());
        assert_eq!(parsed.supports_pause_resume(), caps.supports_pause_resume());
        assert_eq!(parsed.max_timeout_ms(), caps.max_timeout_ms());
        assert_eq!(parsed.max_memory_mb(), caps.max_memory_mb());
        assert_eq!(parsed.max_cpu_count(), caps.max_cpu_count());
        assert_eq!(parsed.supports_networking(), caps.supports_networking());
        assert_eq!(parsed.requires_kvm(), caps.requires_kvm());
        assert_eq!(parsed.avg_startup_ms(), caps.avg_startup_ms());
    }

    // ── Clone tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_clone() {
        let caps = ProviderCapabilities::default();
        let cloned = caps.clone();
        assert_eq!(cloned.supports_snapshots(), caps.supports_snapshots());
        assert_eq!(cloned.supports_streaming(), caps.supports_streaming());
        assert_eq!(cloned.supports_pause_resume(), caps.supports_pause_resume());
        assert_eq!(cloned.max_timeout_ms(), caps.max_timeout_ms());
        assert_eq!(cloned.max_memory_mb(), caps.max_memory_mb());
        assert_eq!(cloned.max_cpu_count(), caps.max_cpu_count());
        assert_eq!(cloned.supports_networking(), caps.supports_networking());
        assert_eq!(cloned.requires_kvm(), caps.requires_kvm());
        assert_eq!(cloned.avg_startup_ms(), caps.avg_startup_ms());
    }

    // ── Display impl tests ────────────────────────────────────────────────────

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::ZeroValue("max_timeout_ms".into());
        assert_eq!(err.to_string(), "max_timeout_ms must be greater than zero");
    }
}
