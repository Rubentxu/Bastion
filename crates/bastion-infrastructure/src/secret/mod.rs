//! Secret resolver adapters.
//!
//! Provides infrastructure implementations of the domain `SecretResolver` port.

pub mod env_resolver;
pub mod noop_resolver;

pub use env_resolver::EnvSecretResolver;
pub use noop_resolver::NoopSecretResolver;
