//! Catalog bounded context — experience records, assertion definitions, and advice.
//!
//! This module contains domain types for capturing structured tool execution
//! evidence, validating it against TOML-based assertion descriptors, and
//! providing context-aware guidance via the advice catalog.

pub mod advice;
pub mod assertion;
pub mod doctor;
pub mod experience;
