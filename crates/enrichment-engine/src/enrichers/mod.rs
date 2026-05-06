//! Built-in enrichers.
//!
//! Ships with a Maven enricher descriptor.

mod maven;

pub use maven::{all_enrichers, maven_enricher};
