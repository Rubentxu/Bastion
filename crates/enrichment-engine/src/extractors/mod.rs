//! Extractors module.
//!
//! Provides implementations of the [`Extractor`] trait for different
//! pattern-based extraction strategies.

pub use command_extractor::CommandExtractor;
pub use glob_extractor::GlobExtractor;
pub use regex_extractor::RegexExtractor;

mod command_extractor;
mod glob_extractor;
mod regex_extractor;
