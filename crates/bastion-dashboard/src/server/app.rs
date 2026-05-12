//! Dashboard HTTP application.
//!
//! Provides the main application builder and server startup.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tracing::info;

use crate::api::DashboardApiPort;
use crate::project::ProjectManagerPort;
use crate::server::router::create_router;
use crate::server::state::DashboardServerState;

/// Default port for the dashboard HTTP server.
pub const DEFAULT_SERVER_PORT: u16 = 3000;

/// Dashboard HTTP application.
///
/// This struct builds and manages the Axum HTTP server for the dashboard.
#[derive(Clone)]
pub struct DashboardApp {
    state: DashboardServerState,
    port: u16,
}

impl DashboardApp {
    /// Create a new DashboardApp with the given state and port.
    pub fn new(
        project_manager: Arc<dyn ProjectManagerPort>,
        api_client: Arc<dyn DashboardApiPort>,
        gateway_url: String,
        port: u16,
    ) -> Self {
        let state = DashboardServerState::new(project_manager, api_client, gateway_url);
        Self { state, port }
    }

    /// Create a new DashboardApp with the default port (3000).
    pub fn with_default_port(
        project_manager: Arc<dyn ProjectManagerPort>,
        api_client: Arc<dyn DashboardApiPort>,
        gateway_url: String,
    ) -> Self {
        Self::new(project_manager, api_client, gateway_url, DEFAULT_SERVER_PORT)
    }

    /// Build the Axum router.
    pub fn router(&self) -> Router {
        create_router(self.state.clone())
    }

    /// Get the socket address for the server.
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], self.port))
    }

    /// Start the HTTP server and listen for requests.
    ///
    /// This starts the server in the foreground using serve_background
    /// and waits for shutdown signal.
    pub async fn serve(self) -> Result<(), ServerError> {
        let handle = self.serve_background().await.map_err(|e| {
            ServerError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
        // Wait for shutdown signal - in a real app this would be triggered by OS signals
        // For now, we just await the handle which can be triggered via shutdown()
        handle.wait().await
    }

    /// Start the HTTP server in the background.
    ///
    /// Returns a handle that can be used to manage the server.
    pub async fn serve_background(self) -> Result<ServerHandle, std::io::Error> {
        let addr = self.socket_addr();
        info!("Starting dashboard server on {} (background)", addr);

        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        info!("Dashboard server listening on {}", local_addr);

        // Create shutdown signal
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Spawn the server
        let task = tokio::spawn(async move {
            axum::serve(listener, self.router())
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                })
                .await
        });

        Ok(ServerHandle {
            shutdown_tx,
            task,
        })
    }
}

/// Handle to a background server instance.
pub struct ServerHandle {
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl ServerHandle {
    /// Signal the server to shutdown gracefully.
    pub fn shutdown(self) -> Result<(), ServerError> {
        self.shutdown_tx
            .send(())
            .map_err(|_| ServerError::AlreadyShutdown)?;
        Ok(())
    }

    /// Wait for the server to finish.
    pub async fn wait(self) -> Result<(), ServerError> {
        self.task
            .await
            .map_err(|_| ServerError::TaskPanic)?
            .map_err(ServerError::IoError)
    }
}

/// Errors that can occur when managing the server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Server is already shutting down")]
    AlreadyShutdown,

    #[error("Server task panicked")]
    TaskPanic,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Start the dashboard HTTP server on the default port.
///
/// This is a convenience function for quick setup.
/// Use `DashboardApp::new()` for more control.
pub async fn serve(
    project_manager: Arc<dyn ProjectManagerPort>,
    api_client: Arc<dyn DashboardApiPort>,
    gateway_url: String,
) -> Result<(), ServerError> {
    let app = DashboardApp::with_default_port(project_manager, api_client, gateway_url);
    app.serve().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::DashboardApiClient;
    use crate::project::ProjectManager;

    fn create_test_app() -> DashboardApp {
        let api_client = DashboardApiClient::default();
        let manager = ProjectManager::new(api_client.clone());
        DashboardApp::new(
            Arc::new(manager),
            Arc::new(api_client),
            "http://localhost:8080".to_string(),
            3001, // Use different port for tests
        )
    }

    #[test]
    fn test_dashboard_app_creation() {
        let app = create_test_app();
        assert_eq!(app.port, 3001);
    }

    #[test]
    fn test_socket_addr() {
        let app = create_test_app();
        let addr = app.socket_addr();
        assert_eq!(addr.port(), 3001);
    }

    #[tokio::test]
    async fn test_serve_background() {
        let app = create_test_app();
        let result = app.serve_background().await;
        assert!(result.is_ok());

        let handle = result.unwrap();
        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Shutdown the server
        let shutdown_result = handle.shutdown();
        assert!(shutdown_result.is_ok());
    }
}
