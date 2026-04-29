//! gRPC client for sandbox worker communication.
//!
//! This adapter translates domain types to/from gRPC messages.
//! TODO: Implement with tonic generated client from protobuf.

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct GrpcWorkerClient {
    endpoint: String,
}

impl GrpcWorkerClient {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}
