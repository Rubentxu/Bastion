//! Application layer catalog orchestration.
//!
//! Contains use cases for recording experience, evaluating assertions,
//! running doctors, and suggesting advice.

pub mod evaluate_assertion;
pub mod record_experience;
pub mod run_doctor;
pub mod suggest_advice;

pub use evaluate_assertion::EvaluateAssertionUseCase;
pub use record_experience::RecordExperienceUseCase;
pub use run_doctor::RunDoctorUseCase;
pub use suggest_advice::{AdviceConfig, SuggestAdviceContext, SuggestAdviceUseCase};
