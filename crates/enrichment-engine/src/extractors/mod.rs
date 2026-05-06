//! Extractors module.
//!
//! Provides implementations of the [`Extractor`] trait for different
//! pattern-based extraction strategies.

pub use glob_extractor::GlobExtractor;
pub use regex_extractor::RegexExtractor;

mod glob_extractor;
mod regex_extractor;
