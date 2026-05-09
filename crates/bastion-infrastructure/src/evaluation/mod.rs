//! Evaluation shim module.
//!
//! Provides legacy-to-CEL evaluation shim for migration parity testing.

pub mod legacy_cel_shim;

pub use legacy_cel_shim::{
    evaluate_advice_trigger, evaluate_assertion_check_with_cel, evaluate_doctor_check,
    evaluate_toml_check, ComparisonResult, ShimError,
};
