//! Resource and capacity constraints for a provider instance.

use serde::{Deserialize, Serialize};

use crate::shared::DomainError;

/// Resource and capacity constraints for a provider instance.
///
/// Controls how many sandboxes can run and what resource limits apply.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InstanceConstraints {
    /// Maximum number of sandboxes that can run concurrently.
    /// None means no limit.
    #[serde(default)]
    max_sandboxes: Option<usize>,
    /// Maximum memory per sandbox in megabytes.
    /// None means no limit.
    #[serde(default)]
    max_memory_mb: Option<u64>,
    /// Maximum number of CPUs available.
    /// None means no limit.
    #[serde(default)]
    max_cpu_count: Option<u32>,
}

impl InstanceConstraints {
    /// Create a new InstanceConstraints with all limits disabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create constraints with validated maximum sandbox count.
    ///
    /// Returns `Err(DomainError::Validation)` if `max` is 0.
    pub fn with_max_sandboxes(max: usize) -> Result<Self, DomainError> {
        if max == 0 {
            return Err(DomainError::Validation(
                "max_sandboxes must be at least 1".into(),
            ));
        }
        Ok(Self {
            max_sandboxes: Some(max),
            ..Default::default()
        })
    }

    /// Create constraints with validated maximum memory limit.
    ///
    /// Returns `Err(DomainError::Validation)` if `max` is 0.
    pub fn with_max_memory_mb(max: u64) -> Result<Self, DomainError> {
        if max == 0 {
            return Err(DomainError::Validation(
                "max_memory_mb must be at least 1".into(),
            ));
        }
        Ok(Self {
            max_memory_mb: Some(max),
            ..Default::default()
        })
    }

    /// Create constraints with validated maximum CPU count.
    ///
    /// Returns `Err(DomainError::Validation)` if `max` is 0.
    pub fn with_max_cpu_count(max: u32) -> Result<Self, DomainError> {
        if max == 0 {
            return Err(DomainError::Validation(
                "max_cpu_count must be at least 1".into(),
            ));
        }
        Ok(Self {
            max_cpu_count: Some(max),
            ..Default::default()
        })
    }

    /// Create constraints with all limits explicitly set.
    ///
    /// Returns `Err(DomainError::Validation)` if any value is 0.
    pub fn create(
        max_sandboxes: Option<usize>,
        max_memory_mb: Option<u64>,
        max_cpu_count: Option<u32>,
    ) -> Result<Self, DomainError> {
        if let Some(v) = max_sandboxes {
            if v == 0 {
                return Err(DomainError::Validation(
                    "max_sandboxes must be at least 1".into(),
                ));
            }
        }
        if let Some(v) = max_memory_mb {
            if v == 0 {
                return Err(DomainError::Validation(
                    "max_memory_mb must be at least 1".into(),
                ));
            }
        }
        if let Some(v) = max_cpu_count {
            if v == 0 {
                return Err(DomainError::Validation(
                    "max_cpu_count must be at least 1".into(),
                ));
            }
        }
        Ok(Self {
            max_sandboxes,
            max_memory_mb,
            max_cpu_count,
        })
    }

    /// Accessor: maximum number of sandboxes.
    pub fn max_sandboxes(&self) -> Option<usize> {
        self.max_sandboxes
    }

    /// Accessor: maximum memory per sandbox in megabytes.
    pub fn max_memory_mb(&self) -> Option<u64> {
        self.max_memory_mb
    }

    /// Accessor: maximum CPU count.
    pub fn max_cpu_count(&self) -> Option<u32> {
        self.max_cpu_count
    }

    /// Check if the max_sandboxes value is valid (None or >= 1).
    pub fn is_valid_max_sandboxes(&self) -> bool {
        self.max_sandboxes.is_none_or(|v| v >= 1)
    }

    /// Check if the max_memory_mb value is valid (None or >= 1).
    pub fn is_valid_max_memory_mb(&self) -> bool {
        self.max_memory_mb.is_none_or(|v| v >= 1)
    }

    /// Check if the max_cpu_count value is valid (None or >= 1).
    pub fn is_valid_max_cpu_count(&self) -> bool {
        self.max_cpu_count.is_none_or(|v| v >= 1)
    }

    /// Check if all constraint values are valid.
    pub fn is_valid(&self) -> bool {
        self.is_valid_max_sandboxes()
            && self.is_valid_max_memory_mb()
            && self.is_valid_max_cpu_count()
    }

    /// Check if this instance can accept more sandboxes.
    pub fn has_capacity(&self, current_count: usize) -> bool {
        self.max_sandboxes.is_none_or(|max| current_count < max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_constraints_default() {
        let constraints = InstanceConstraints::default();
        assert!(constraints.max_sandboxes().is_none());
        assert!(constraints.max_memory_mb().is_none());
        assert!(constraints.max_cpu_count().is_none());
        assert!(constraints.is_valid());
    }

    #[test]
    fn test_instance_constraints_with_max_sandboxes() {
        let constraints = InstanceConstraints::with_max_sandboxes(10).unwrap();
        assert_eq!(constraints.max_sandboxes(), Some(10));
    }

    #[test]
    fn test_instance_constraints_with_max_memory() {
        let constraints = InstanceConstraints::with_max_memory_mb(8192).unwrap();
        assert_eq!(constraints.max_memory_mb(), Some(8192));
    }

    #[test]
    fn test_instance_constraints_with_max_cpu() {
        let constraints = InstanceConstraints::with_max_cpu_count(4).unwrap();
        assert_eq!(constraints.max_cpu_count(), Some(4));
    }

    #[test]
    fn test_instance_constraints_rejects_zero_sandboxes() {
        let err = InstanceConstraints::with_max_sandboxes(0).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_instance_constraints_rejects_zero_memory() {
        let err = InstanceConstraints::with_max_memory_mb(0).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_instance_constraints_rejects_zero_cpu() {
        let err = InstanceConstraints::with_max_cpu_count(0).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_instance_constraints_create_valid() {
        let constraints = InstanceConstraints::create(Some(10), Some(8192), Some(4)).unwrap();
        assert_eq!(constraints.max_sandboxes(), Some(10));
        assert_eq!(constraints.max_memory_mb(), Some(8192));
        assert_eq!(constraints.max_cpu_count(), Some(4));
        assert!(constraints.is_valid());
    }

    #[test]
    fn test_instance_constraints_create_rejects_zero_values() {
        let err = InstanceConstraints::create(Some(0), Some(8192), Some(4)).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));

        let err = InstanceConstraints::create(Some(10), Some(0), Some(4)).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));

        let err = InstanceConstraints::create(Some(10), Some(8192), Some(0)).unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn test_instance_constraints_is_valid_helpers() {
        let constraints = InstanceConstraints::default();
        assert!(constraints.is_valid_max_sandboxes());
        assert!(constraints.is_valid_max_memory_mb()); // None is valid (no limit)
        assert!(constraints.is_valid_max_cpu_count()); // None is valid (no limit)

        let constraints = InstanceConstraints::with_max_sandboxes(10).unwrap();
        assert!(constraints.is_valid_max_sandboxes());
        assert!(constraints.is_valid_max_memory_mb()); // None is valid (is_none_or returns true)
        assert!(constraints.is_valid_max_cpu_count()); // None is valid (is_none_or returns true)
    }

    #[test]
    fn test_instance_constraints_has_capacity_no_limit() {
        let constraints = InstanceConstraints::default();
        assert!(constraints.has_capacity(1000));
    }

    #[test]
    fn test_instance_constraints_has_capacity_with_limit() {
        let constraints = InstanceConstraints::with_max_sandboxes(5).unwrap();
        assert!(constraints.has_capacity(0));
        assert!(constraints.has_capacity(4));
        assert!(!constraints.has_capacity(5));
        assert!(!constraints.has_capacity(100));
    }

    #[test]
    fn test_instance_constraints_serde() {
        let constraints = InstanceConstraints::create(Some(10), Some(8192), Some(4)).unwrap();
        let json = serde_json::to_string(&constraints).unwrap();
        let parsed: InstanceConstraints = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_sandboxes(), Some(10));
        assert_eq!(parsed.max_memory_mb(), Some(8192));
        assert_eq!(parsed.max_cpu_count(), Some(4));
    }
}
