//! Error types for the bastion-dashboard crate.
//!
//! This module provides unified error types for project management,
//! API client operations, and dashboard-specific errors.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use thiserror::Error;

use crate::api::types::ApiErrorResponse;

/// Errors that can occur when managing projects.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    #[error("Invalid project path: {0}")]
    InvalidProjectPath(String),

    #[error("Failed to initialize project: {0}")]
    ProjectInitFailed(String),

    #[error("Failed to parse project configuration: {0}")]
    ConfigParseError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Errors that can occur when interacting with the dashboard API.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Internal server error: {0}")]
    InternalError(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BadRequest"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "NotFound"),
            ApiError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "InternalError"),
            ApiError::NetworkError(_) => (StatusCode::BAD_GATEWAY, "NetworkError"),
            ApiError::SerializationError(_) => (StatusCode::BAD_REQUEST, "SerializationError"),
            ApiError::InvalidResponse(_) => (StatusCode::BAD_GATEWAY, "InvalidResponse"),
        };

        let body = Json(ApiErrorResponse::new(error_type, self.to_string()));
        (status, body).into_response()
    }
}

/// General dashboard errors that don't fit into project or API categories.
#[derive(Debug, Error)]
pub enum DashboardError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl IntoResponse for DashboardError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            DashboardError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BadRequest"),
            DashboardError::NotFound(_) => (StatusCode::NOT_FOUND, "NotFound"),
            DashboardError::InternalError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "InternalError"),
        };

        let body = Json(ApiErrorResponse::new(error_type, self.to_string()));
        (status, body).into_response()
    }
}

/// Errors that can occur during SSE event parsing.
#[derive(Debug, Error)]
pub enum SseError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Invalid event type: {0}")]
    InvalidEventType(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}
