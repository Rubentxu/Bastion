//! Configuration adapter — reads TOML config into domain types.

pub mod embedded_defaults;
pub mod loader;

pub use embedded_defaults::*;
pub use loader::GatewayConfig;
