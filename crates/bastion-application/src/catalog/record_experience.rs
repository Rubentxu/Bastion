//! RecordExperience use case.
//!
//! Persists a structured experience record after a tool execution completes.

use std::sync::Arc;

use bastion_domain::catalog::experience::{ExperienceRecord, ExperienceStore};
use bastion_domain::shared::DomainError;

/// Use case for recording an experience after tool execution.
#[derive(Debug)]
pub struct RecordExperienceUseCase<S: ExperienceStore> {
    store: Arc<S>,
}

impl<S: ExperienceStore> RecordExperienceUseCase<S> {
    /// Create a new use case with the given store.
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    /// Execute the use case: persist the experience record.
    pub async fn execute(&self, record: ExperienceRecord) -> Result<(), DomainError> {
        tracing::debug!(
            experience_id = %record.id(),
            tool_name = %record.tool_name(),
            trace_id = ?record.trace_id(),
            "Recording experience"
        );
        self.store.save(&record).await
    }
}
