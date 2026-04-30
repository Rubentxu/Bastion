//! TOML configuration loader.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct GatewayConfig {
    pub server: ServerConfig,
    pub default_provider: String,
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub pool: PoolConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub transport: String,
    #[serde(default = "default_http_addr")]
    pub http_addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8080".to_string()
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    #[serde(rename = "podman")]
    Podman(PodmanConfig),
    #[serde(rename = "firecracker")]
    Firecracker(FirecrackerConfig),
    #[serde(rename = "gvisor")]
    GVisor(GVisorConfig),
}

#[derive(Debug, Deserialize, Clone)]
pub struct PodmanConfig {
    pub socket_path: String,
    pub default_image: String,
    #[serde(default = "default_network_mode")]
    pub network_mode: String,
    #[serde(default)]
    pub rootless: bool,
    #[serde(default = "default_pool_size")]
    pub hot_pool_size: usize,
}

fn default_network_mode() -> String {
    "bridge".to_string()
}
fn default_pool_size() -> usize {
    3
}

#[derive(Debug, Deserialize, Clone)]
pub struct FirecrackerConfig {
    pub kernel_path: String,
    pub rootfs_path: String,
    pub firecracker_bin: String,
    #[serde(default)]
    pub jailer_bin: Option<String>,
    #[serde(default = "default_pool_size")]
    pub hot_pool_size: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GVisorConfig {
    pub runsc_bin: String,
    pub default_image: String,
    #[serde(default = "default_pool_size")]
    pub hot_pool_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct PoolConfig {
    #[serde(default = "default_min_hot")]
    pub min_hot_per_provider: usize,
    #[serde(default = "default_max_hot")]
    pub max_hot_per_provider: usize,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_ms: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_ms: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_hot_per_provider: 1,
            max_hot_per_provider: 10,
            idle_timeout_ms: 300_000,
            cleanup_interval_ms: 60_000,
        }
    }
}

fn default_min_hot() -> usize {
    1
}
fn default_max_hot() -> usize {
    10
}
fn default_idle_timeout() -> u64 {
    300_000
}
fn default_cleanup_interval() -> u64 {
    60_000
}
