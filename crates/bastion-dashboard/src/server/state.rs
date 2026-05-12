//! Server state management.
//!
//! Provides the shared state structure used by all HTTP handlers.

use std::sync::Arc;

use crate::api::DashboardApiPort;
use crate::project::ProjectManagerPort;

/// Shared state for the dashboard HTTP server.
///
/// This state is cloned and passed to each request handler via Axum's
/// `State` extractor, allowing handlers to access the project manager
/// and API client without direct access to internal Arc structures.
#[derive(Clone)]
pub struct DashboardServerState {
    /// Project management capability.
    pub project_manager: Arc<dyn ProjectManagerPort>,
    /// Dashboard API client for gateway communication.
    pub api_client: Arc<dyn DashboardApiPort>,
    /// Base URL of the Bastion gateway.
    pub gateway_url: String,
}

impl DashboardServerState {
    /// Create a new server state.
    pub fn new(
        project_manager: Arc<dyn ProjectManagerPort>,
        api_client: Arc<dyn DashboardApiPort>,
        gateway_url: String,
    ) -> Self {
        Self {
            project_manager,
            api_client,
            gateway_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_server_state_clone() {
        // DashboardServerState should be Clone to work with Axum's State extractor
        fn assert_clone<T: Clone>() {}
        assert_clone::<DashboardServerState>();
    }
}
