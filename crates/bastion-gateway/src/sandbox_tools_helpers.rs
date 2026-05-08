//! Helper methods for sandbox tools.

use std::sync::Arc;

use rmcp::Peer;
use rmcp::model::{ProgressNotificationParam, ProgressToken};

use crate::server::BastionGateway;

/// Helper methods for BastionGateway sandbox tools.
impl BastionGateway {
    /// Resolve a provider by name, falling back to default
    pub(crate) fn resolve_provider(
        &self,
        name: &str,
    ) -> Arc<dyn bastion_domain::provider::SandboxProvider> {
        let name_lower = name.to_lowercase();
        self.providers.get(&name_lower).cloned().unwrap_or_else(|| {
            tracing::warn!(
                requested = name,
                "Provider not found, falling back to default"
            );
            self.provider.clone()
        })
    }

    /// Send a progress notification to the MCP client.
    /// If sending fails, logs a warning but continues execution.
    pub(crate) async fn send_progress(
        peer: &Peer<rmcp::RoleServer>,
        token: &ProgressToken,
        progress: f64,
        message: Option<&str>,
    ) {
        let params = match message {
            Some(msg) => ProgressNotificationParam::new(token.clone(), progress).with_message(msg),
            None => ProgressNotificationParam::new(token.clone(), progress),
        };
        if let Err(e) = peer.notify_progress(params).await {
            tracing::warn!(error = %e, "Failed to send progress notification");
        }
    }

    /// Build a progress message from current stdout/stderr accumulated output.
    pub(crate) fn build_progress_message(
        stdout_parts: &[String],
        stderr_parts: &[String],
        chunk_count: u32,
    ) -> Option<String> {
        // Show last 200 chars of stdout as preview, truncated for notification size
        let stdout_preview = stdout_parts
            .last()
            .map(|s| {
                if s.len() > 200 {
                    format!("{}...", &s[s.len() - 200..])
                } else {
                    s.clone()
                }
            })
            .filter(|s| !s.is_empty());

        let message = match (stdout_preview, stderr_parts.is_empty()) {
            (Some(preview), true) => format!("[{} chunks] {}", chunk_count, preview),
            (Some(preview), false) => format!("[{} chunks] {} (+stderr)", chunk_count, preview),
            (None, false) => format!("[{} chunks] (stderr: {})", chunk_count, stderr_parts.len()),
            (None, true) => format!("[{} chunks] processing...", chunk_count),
        };

        // Truncate message if too long for notification
        if message.len() > 500 {
            Some(format!("{}...", &message[..500]))
        } else {
            Some(message)
        }
    }
}
