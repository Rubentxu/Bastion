//! Catalog infrastructure module.
//!
//! Contains TOML-based parsers for assertions, doctors, and advice;
//! SQLite-backed experience store.

pub mod sqlite_experience_store;
pub mod toml_advice_parser;
pub mod toml_assertion_parser;
pub mod toml_doctor_parser;
