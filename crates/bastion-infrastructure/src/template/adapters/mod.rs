//! Tool manager adapter implementations.
//!
//! Each adapter knows how to install tools using a specific package manager
//! or version manager (apt, asdf, sdkman, etc.).

mod apt;
mod asdf;
mod sdkman;

pub use apt::AptAdapter;
pub use asdf::AsdfAdapter;
pub use sdkman::SdkmanAdapter;
