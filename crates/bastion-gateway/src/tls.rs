//! TLS configuration for the worker registry server.
//!
//! TLS is optional. By default, the registry uses plaintext (HTTP/2 without TLS).
//! To enable TLS, provide --tls-cert and --tls-key arguments.

use std::path::Path;
use anyhow::Result;

/// TLS configuration
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

impl TlsConfig {
    /// Load TLS identity from PEM files
    pub fn load_identity(&self) -> Result<tonic::transport::Identity> {
        let cert = std::fs::read_to_string(&self.cert_path)?;
        let key = std::fs::read_to_string(&self.key_path)?;
        Ok(tonic::transport::Identity::from_pem(cert, key))
    }

    /// Check if TLS files exist
    pub fn files_exist(&self) -> bool {
        Path::new(&self.cert_path).exists() && Path::new(&self.key_path).exists()
    }
}