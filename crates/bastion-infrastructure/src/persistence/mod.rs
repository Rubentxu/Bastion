//! Persistence adapters — sandbox storage implementations.

pub mod in_memory;
pub mod sqlite_repository;

pub use in_memory::InMemorySandboxRepository;
pub use sqlite_repository::SqliteSandboxRepository;
