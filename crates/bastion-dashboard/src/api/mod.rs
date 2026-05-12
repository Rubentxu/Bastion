//! API module for dashboard communication.
//!
//! This module provides the HTTP client for communicating with the Bastion
//! gateway API, including request/response types and SSE event handling.

pub mod client;
pub mod events;
pub mod types;

pub use client::{DashboardApiClient, DashboardApiPort};
pub use events::{parse_sse_stream, SseEvent};
pub use types::{
    ApiErrorResponse, MetricsResponse, ProjectDetailResponse,
    SandboxDetailResponse, SandboxInfo, SandboxListResponse, SandboxResourcesResponse,
    ResourceUsage, CreateSandboxRequest,
};
