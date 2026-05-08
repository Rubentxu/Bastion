//! Domain errors — infrastructure-agnostic error types.

use thiserror::Error;

/// Core domain error type.
///
/// All domain operations return `Result<T, DomainError>`.
/// Infrastructure adapters map their specific errors into these variants.
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("Sandbox not found: {0}")]
    NotFound(String),

    #[error("Sandbox '{0}' already exists")]
    AlreadyExists(String),

    #[error("Sandbox timeout: {0}")]
    Timeout(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Provider '{0}' is unavailable")]
    ProviderUnavailable(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Command failed with exit code {exit_code}: {stderr}")]
    CommandFailed { exit_code: i32, stderr: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Rate limiter lock poisoned: {0}")]
    PoisonedLock(String),
}

impl From<std::io::Error> for DomainError {
    fn from(e: std::io::Error) -> Self {
        DomainError::Internal(e.to_string())
    }
}
