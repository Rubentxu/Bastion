//! Socket reference types for provider backends.
//!
//! Describes how to connect to a provider runtime socket.

use serde::{Deserialize, Serialize};

/// Reference to a provider runtime socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketRef {
    /// Unix domain socket.
    Unix {
        /// Socket file path.
        address: String,
    },
    /// TCP socket.
    Tcp {
        /// Host:port address.
        address: String,
    },
    /// SSH tunnel to remote socket.
    Ssh {
        /// Host:port of the remote endpoint.
        address: String,
        /// Path to SSH private key.
        ssh_key: Option<String>,
        /// SSH username.
        ssh_user: Option<String>,
    },
}

impl SocketRef {
    /// Create a Unix socket reference.
    pub fn unix(address: impl Into<String>) -> Self {
        Self::Unix {
            address: address.into(),
        }
    }

    /// Create a TCP socket reference.
    pub fn tcp(address: impl Into<String>) -> Self {
        Self::Tcp {
            address: address.into(),
        }
    }

    /// Create an SSH socket reference.
    pub fn ssh(address: impl Into<String>) -> Self {
        Self::Ssh {
            address: address.into(),
            ssh_key: None,
            ssh_user: None,
        }
    }

    /// Get the address string for this socket.
    pub fn address(&self) -> &str {
        match self {
            Self::Unix { address } => address,
            Self::Tcp { address } => address,
            Self::Ssh { address, .. } => address,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_ref_unix() {
        let socket = SocketRef::unix("/run/podman/podman.sock");
        assert_eq!(socket.address(), "/run/podman/podman.sock");
    }

    #[test]
    fn test_socket_ref_tcp() {
        let socket = SocketRef::tcp("localhost:2375");
        assert_eq!(socket.address(), "localhost:2375");
    }

    #[test]
    fn test_socket_ref_ssh() {
        let socket = SocketRef::ssh("example.com:2375");
        assert_eq!(socket.address(), "example.com:2375");
    }

    #[test]
    fn test_socket_ref_serde_unix() {
        let socket = SocketRef::unix("/var/run/docker.sock");
        let json = serde_json::to_string(&socket).unwrap();
        assert!(json.contains("\"type\":\"unix\""));
        let parsed: SocketRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.address(), "/var/run/docker.sock");
    }

    #[test]
    fn test_socket_ref_serde_tcp() {
        let socket = SocketRef::tcp("localhost:2375");
        let json = serde_json::to_string(&socket).unwrap();
        assert!(json.contains("\"type\":\"tcp\""));
        let parsed: SocketRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.address(), "localhost:2375");
    }

    #[test]
    fn test_socket_ref_serde_ssh() {
        let socket = SocketRef::ssh("remote.example.com:22");
        let json = serde_json::to_string(&socket).unwrap();
        assert!(json.contains("\"type\":\"ssh\""));
        let parsed: SocketRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.address(), "remote.example.com:22");
    }
}
