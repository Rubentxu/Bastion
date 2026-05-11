//! Orientation bounded context — agent guidance, template recommendations, and config management.
//!
//! This module provides domain types for proactive agent orientation:
//! - Template recommendations based on command patterns
//! - Configuration history and audit trails
//! - Agent-facing orientation data structures

pub mod config_history;
pub mod template_recommender;

pub use config_history::{ConfigChange, ConfigHistory};
pub use template_recommender::{TemplateRecommendation, TemplateRecommender};
