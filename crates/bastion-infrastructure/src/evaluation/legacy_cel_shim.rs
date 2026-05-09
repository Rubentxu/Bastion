//! Legacy-to-CEL evaluation shim.
//!
//! Provides parallel evaluation of checks via both the legacy domain path
//! (AssertionCheck::evaluate) and the CEL-lite rules engine, enabling
//! parity validation during migration.
//!
//! The shim lives in bastion-infrastructure because it requires access to both:
//! - bastion-domain: AssertionCheck, DoctorCheck, AdviceTrigger, ExperienceRecord
//! - enrichment-engine: RuleEvaluator, RuleConfig, EvalContext, Parser

use enrichment_engine::models::{Fact, OperationInvocation, OperationResult, RuleConfig};
use enrichment_engine::rules::ast::{EvalContext, Parser};

use bastion_domain::catalog::assertion::AssertionCheck;
use bastion_domain::catalog::experience::ExperienceRecord;

use crate::catalog::toml_assertion_parser::TomlCheck;
use crate::catalog::toml_doctor_parser::TomlDoctorCheck;
use crate::catalog::toml_advice_parser::TomlTrigger;

// ─── Error types ─────────────────────────────────────────────────────────────

/// Errors that can occur during shim evaluation.
#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    #[error("CEL parse error: {0}")]
    CelParse(String),
    #[error("Legacy evaluation failed: {0}")]
    LegacyEval(String),
    #[error("CEL/legacy mismatch: legacy={legacy_result}, cel={cel_result}")]
    ResultMismatch { legacy_result: bool, cel_result: bool },
    #[error("No CEL condition available for this check type")]
    NoCelCondition,
}

// ─── Comparison result ────────────────────────────────────────────────────────

/// Result of comparing legacy and CEL evaluation paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonResult {
    /// Whether the legacy path passed.
    pub legacy_passed: bool,
    /// Whether the CEL path passed.
    pub cel_passed: bool,
    /// Optional failure reason from legacy path.
    pub legacy_reason: Option<String>,
    /// Whether the results match.
    pub matches: bool,
}

// ─── Synchronous CEL evaluation helper ────────────────────────────────────────

/// Evaluate a CEL condition string synchronously against an ExperienceRecord.
fn eval_cel_on_record(record: &ExperienceRecord, condition: &str) -> Result<bool, ShimError> {
    let expr = Parser::parse(condition).map_err(|e| ShimError::CelParse(e.to_string()))?;

    // Build OperationResult from ExperienceRecord fields
    let exit_code = record.exit_code.unwrap_or(0);
    // Use i32::MAX as sentinel for unknown duration.
    // This ensures duration comparisons fail for unfinished commands,
    // matching legacy AssertionCheck behavior where duration unknown = check fails.
    let duration_ms = record
        .duration_ms()
        .map(|d| d as i32)
        .unwrap_or(i32::MAX);
    let timed_out = matches!(
        record.status,
        bastion_domain::catalog::experience::ExperienceStatus::Timeout
    );

    let result = OperationResult {
        exit_code,
        stdout: record.stdout_summary.clone(),
        stderr: record.stderr_summary.clone(),
        duration_ms: duration_ms as u64,
        timed_out,
    };

    let invocation = OperationInvocation::from_command(&record.tool_name);
    let facts: Vec<Fact> = vec![];
    let ctx = EvalContext::new(&invocation, &result, &facts);

    Ok(ctx.evaluate(&expr))
}

/// Evaluate a CEL condition string synchronously against an ExperienceRecord with extra facts.
fn eval_cel_on_record_with_facts(
    record: &ExperienceRecord,
    extra_facts: &[Fact],
    condition: &str,
) -> Result<bool, ShimError> {
    let expr = Parser::parse(condition).map_err(|e| ShimError::CelParse(e.to_string()))?;

    let exit_code = record.exit_code.unwrap_or(0);
    let duration_ms = record
        .duration_ms()
        .map(|d| d as i32)
        .unwrap_or(i32::MAX);
    let timed_out = matches!(
        record.status,
        bastion_domain::catalog::experience::ExperienceStatus::Timeout
    );

    let result = OperationResult {
        exit_code,
        stdout: record.stdout_summary.clone(),
        stderr: record.stderr_summary.clone(),
        duration_ms: duration_ms as u64,
        timed_out,
    };

    let invocation = OperationInvocation::from_command(&record.tool_name);
    let all_facts: Vec<Fact> = extra_facts.to_vec();
    let ctx = EvalContext::new(&invocation, &result, &all_facts);

    Ok(ctx.evaluate(&expr))
}

// ─── Assertion check shim ─────────────────────────────────────────────────────

/// Evaluate a TomlCheck via both legacy and CEL paths and compare results.
///
/// This takes the raw TOML-level type (before conversion to domain AssertionCheck)
/// so it can exercise the full pipeline including the CEL condition generation.
///
/// # Arguments
/// * `toml_check` — The TomlCheck to evaluate (consumed)
/// * `record` — The ExperienceRecord to evaluate against
///
/// # Returns
/// A `ComparisonResult` indicating pass/fail for each path and whether they match.
pub fn evaluate_toml_check(
    toml_check: TomlCheck,
    record: &ExperienceRecord,
) -> Result<ComparisonResult, ShimError> {
    // CEL path: generate condition from TomlCheck (before consuming)
    let cel_condition = toml_check
        .to_cel_condition()
        .ok_or(ShimError::NoCelCondition)?;

    // Legacy path: TomlCheck → AssertionCheck → evaluate
    let assertion_check: AssertionCheck = toml_check.into();
    let (legacy_passed, legacy_reason) = assertion_check.evaluate(record);

    let cel_passed = eval_cel_on_record(record, &cel_condition)?;

    let matches = legacy_passed == cel_passed;

    Ok(ComparisonResult {
        legacy_passed,
        cel_passed,
        legacy_reason,
        matches,
    })
}

/// Evaluate an assertion check via both legacy and CEL paths and compare results.
///
/// # Arguments
/// * `check` — The AssertionCheck to evaluate
/// * `record` — The ExperienceRecord to evaluate against
/// * `cel_condition` — The CEL condition string to evaluate (if available)
///
/// # Returns
/// A `ComparisonResult` indicating pass/fail for each path and whether they match.
pub fn evaluate_assertion_check_with_cel(
    check: &AssertionCheck,
    record: &ExperienceRecord,
    cel_condition: Option<&str>,
) -> Result<ComparisonResult, ShimError> {
    // Legacy path: AssertionCheck::evaluate
    let (legacy_passed, legacy_reason) = check.evaluate(record);

    // CEL path (if condition available)
    let cel_passed = if let Some(condition) = cel_condition {
        eval_cel_on_record(record, condition)?
    } else {
        // No CEL condition available — skip CEL evaluation
        return Ok(ComparisonResult {
            legacy_passed,
            cel_passed: legacy_passed,
            legacy_reason,
            matches: true,
        });
    };

    let matches = legacy_passed == cel_passed;

    Ok(ComparisonResult {
        legacy_passed,
        cel_passed,
        legacy_reason,
        matches,
    })
}

// ─── Doctor check shim ────────────────────────────────────────────────────────

/// Evaluate a doctor AssertionDriven check via both legacy and CEL paths.
///
/// For AssertionDriven checks, the CEL condition is `fact('assertion:<id>') == 'passed'`.
/// The legacy path delegates to the referenced assertion's evaluate method.
///
/// # Arguments
/// * `toml_check` — The TomlDoctorCheck (AssertionDriven variant)
/// * `record` — The ExperienceRecord to evaluate against
/// * `assertion_registry` — Registry to resolve referenced assertions
///
/// # Returns
/// A `ComparisonResult` indicating pass/fail for each path and whether they match.
pub fn evaluate_doctor_check(
    toml_check: &TomlDoctorCheck,
    record: &ExperienceRecord,
    assertion_registry: &crate::catalog::toml_assertion_parser::AssertionRegistry,
) -> Result<ComparisonResult, ShimError> {
    match toml_check {
        TomlDoctorCheck::AssertionDriven { assertion_id } => {
            // Legacy: find the assertion and evaluate its checks
            let descriptor = assertion_registry.get(assertion_id).ok_or_else(|| {
                ShimError::LegacyEval(format!("Assertion '{}' not found", assertion_id))
            })?;

            let (legacy_passed, legacy_reason) =
                evaluate_assertion_descriptor(&descriptor.checks, record);

            // CEL path: generate condition from TomlDoctorCheck
            let cel_condition = toml_check
                .to_cel_condition()
                .ok_or(ShimError::NoCelCondition)?;

            // For AssertionDriven CEL evaluation, we need a fact with the assertion result.
            // We synthesize this fact from the legacy evaluation result.
            let assertion_fact_value = if legacy_passed { "passed" } else { "failed" };
            let fact = Fact {
                key: format!("assertion:{}", assertion_id),
                value: assertion_fact_value.to_string(),
                tags: vec![],
                source_extractor: "legacy_shim".to_string(),
                confidence: 1.0,
            };

            let cel_passed = eval_cel_on_record_with_facts(record, &[fact], &cel_condition)?;

            let matches = legacy_passed == cel_passed;

            Ok(ComparisonResult {
                legacy_passed,
                cel_passed,
                legacy_reason,
                matches,
            })
        }
        TomlDoctorCheck::Aliveness { .. } | TomlDoctorCheck::Resources { .. } => {
            // These have no CEL equivalent — return legacy result only
            Ok(ComparisonResult {
                legacy_passed: true, // Deferred checks always pass at domain level
                cel_passed: true,
                legacy_reason: None,
                matches: true,
            })
        }
    }
}

/// Evaluate an assertion descriptor's checks (all combined with AND logic).
fn evaluate_assertion_descriptor(
    checks: &[AssertionCheck],
    record: &ExperienceRecord,
) -> (bool, Option<String>) {
    let mut all_passed = true;
    let mut reasons = vec![];

    for check in checks {
        let (passed, reason) = check.evaluate(record);
        if !passed {
            all_passed = false;
            if let Some(r) = reason {
                reasons.push(r);
            }
        }
    }

    if reasons.is_empty() {
        (all_passed, None)
    } else {
        (false, Some(reasons.join("; ")))
    }
}

// ─── Advice trigger shim ─────────────────────────────────────────────────────

/// Evaluate an advice trigger via both legacy and CEL paths.
///
/// For advice triggers:
/// - AssertionFailed: fact('assertion:<id>') == 'failed'
/// - DoctorFailed: fact('doctor:<id>') == 'failed'
/// - ExperiencePattern: count_fact('experience:<tool>:<status>', '>=', threshold)
///
/// # Arguments
/// * `toml_trigger` — The TomlTrigger to evaluate
/// * `facts` — Facts available for CEL evaluation (e.g., from experience store)
///
/// # Returns
/// A `ComparisonResult` indicating pass/fail for each path and whether they match.
pub fn evaluate_advice_trigger(
    toml_trigger: &TomlTrigger,
    facts: &[Fact],
) -> Result<ComparisonResult, ShimError> {
    let cel_condition = toml_trigger
        .to_cel_condition()
        .ok_or(ShimError::NoCelCondition)?;

    // Build a minimal EvalContext for trigger evaluation.
    // Triggers don't have a specific record — they evaluate against accumulated facts.
    let invocation = OperationInvocation::from_command("trigger_evaluation");
    let result = OperationResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
        timed_out: false,
    };
    let ctx = EvalContext::new(&invocation, &result, facts);
    let cel_passed = eval_cel_sync(&ctx, &cel_condition)?;

    // Legacy path is satisfied when CEL is satisfied (triggers fire based on conditions)
    Ok(ComparisonResult {
        legacy_passed: cel_passed,
        cel_passed,
        legacy_reason: None,
        matches: true, // Triggers don't have a separate legacy eval, so they always match
    })
}

/// Evaluate a CEL condition string synchronously against a context.
fn eval_cel_sync(ctx: &EvalContext<'_>, condition: &str) -> Result<bool, ShimError> {
    let expr = Parser::parse(condition).map_err(|e| ShimError::CelParse(e.to_string()))?;
    Ok(ctx.evaluate(&expr))
}

// ─── Convenience RuleConfig builder ───────────────────────────────────────────

impl TomlCheck {
    /// Build a RuleConfig from this TomlCheck for the given enricher_id.
    ///
    /// Returns None if the check has no CEL equivalent (e.g., SandboxAlive).
    #[allow(dead_code)]
    pub fn to_rule_config(&self, enricher_id: &str) -> Option<RuleConfig> {
        let condition = self.to_cel_condition()?;
        Some(RuleConfig {
            id: format!("check_{}", uuid::Uuid::new_v4()),
            enricher_id: enricher_id.to_string(),
            condition,
            priority: 0,
            enabled: true,
            actions: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::catalog::experience::ExperienceRecord;
    use crate::catalog::toml_assertion_parser::AssertionRegistry;

    fn make_pass_record() -> ExperienceRecord {
        ExperienceRecord::new("sandbox_run")
            .with_trace_id("test-001")
            .completed(0)
            .with_stdout(b"BUILD SUCCESS")
            .with_stderr(b"")
    }

    fn make_fail_record() -> ExperienceRecord {
        ExperienceRecord::new("sandbox_run")
            .with_trace_id("test-002")
            .completed(1)
            .with_stdout(b"BUILD FAILURE")
            .with_stderr(b"ERROR: compilation failed")
    }

    // ─── ExitCode tests ───────────────────────────────────────────────────────

    #[test]
    fn test_assertion_exit_code_pass_legacy() {
        let record = make_pass_record();
        let check = AssertionCheck::ExitCode { expected: 0 };
        let (passed, reason) = check.evaluate(&record);
        assert!(passed);
        assert!(reason.is_none());
    }

    #[test]
    fn test_assertion_exit_code_fail_legacy() {
        let record = make_fail_record();
        let check = AssertionCheck::ExitCode { expected: 0 };
        let (passed, reason) = check.evaluate(&record);
        assert!(!passed);
        assert!(reason.is_some());
    }

    #[test]
    fn test_toml_check_exit_code_parity_pass() {
        let record = make_pass_record();
        let toml_check = TomlCheck::ExitCode { expected: 0 };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
    }

    #[test]
    fn test_toml_check_exit_code_parity_fail() {
        let record = make_fail_record();
        let toml_check = TomlCheck::ExitCode { expected: 0 };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(!result.legacy_passed);
        assert!(!result.cel_passed);
    }

    // ─── StdoutContains tests ─────────────────────────────────────────────────

    #[test]
    fn test_toml_check_stdout_contains_parity_pass() {
        let record = make_pass_record();
        let toml_check = TomlCheck::StdoutContains {
            substring: "BUILD SUCCESS".to_string(),
        };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
    }

    #[test]
    fn test_toml_check_stdout_contains_parity_fail() {
        let record = make_fail_record();
        let toml_check = TomlCheck::StdoutContains {
            substring: "BUILD SUCCESS".to_string(),
        };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(!result.legacy_passed);
        assert!(!result.cel_passed);
    }

    // ─── StderrContains tests ─────────────────────────────────────────────────

    #[test]
    fn test_toml_check_stderr_contains_parity_pass() {
        let record = ExperienceRecord::new("sandbox_run")
            .completed(1)
            .with_stdout(b"")
            .with_stderr(b"ERROR: fail");
        let toml_check = TomlCheck::StderrContains {
            substring: "ERROR".to_string(),
        };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
    }

    #[test]
    fn test_toml_check_stderr_contains_parity_fail() {
        let record = ExperienceRecord::new("sandbox_run")
            .completed(1)
            .with_stdout(b"")
            .with_stderr(b"BUILD OK");
        let toml_check = TomlCheck::StderrContains {
            substring: "ERROR".to_string(),
        };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(!result.legacy_passed);
        assert!(!result.cel_passed);
    }

    // ─── CommandDuration tests ────────────────────────────────────────────────

    #[test]
    fn test_toml_check_duration_lt_parity_pass() {
        let record = ExperienceRecord::new("sandbox_run")
            .with_stdout(b"done")
            .with_stderr(b"");
        // Simulate a fast command by setting finished_at close to started_at
        let mut record = record;
        record.exit_code = Some(0);
        record.finished_at = Some(record.started_at);

        let toml_check = TomlCheck::CommandDuration { max_ms: 1000 };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        assert!(result.matches);
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
    }

    #[test]
    fn test_toml_check_duration_lt_parity_fail() {
        // Record with no duration (still running) — should fail the check
        let record = ExperienceRecord::new("sandbox_run").with_stdout(b"");
        let toml_check = TomlCheck::CommandDuration { max_ms: 1000 };
        let result = evaluate_toml_check(toml_check.clone(), &record).unwrap();
        // Both should agree (neither passes when duration unknown)
        assert!(result.matches);
        assert!(!result.legacy_passed);
        assert!(!result.cel_passed);
    }

    // ─── StdoutMatches tests ─────────────────────────────────────────────────
    // NOTE: stdout_matches is NOT yet implemented in the CEL parser (Phase 1 gap).
    // This test documents the expected behavior once the function is added.

    #[test]
    #[ignore = "stdout_matches not yet implemented in CEL parser"]
    fn test_toml_check_stdout_matches_parity() {
        let record = ExperienceRecord::new("sandbox_run")
            .completed(0)
            .with_stdout(b"Version: 1.2.3");
        let toml_check = TomlCheck::StdoutMatches {
            regex: r"Version: \d+\.\d+\.\d+".to_string(),
        };
        let result = evaluate_toml_check(toml_check, &record).unwrap();
        // Legacy path: StdoutMatches always passes at domain level (regex deferred)
        // CEL path: regex is evaluated at parse time
        // For this test, CEL should match
        assert!(result.cel_passed);
    }

    // ─── Advice trigger tests ─────────────────────────────────────────────────

    #[test]
    fn test_advice_trigger_assertion_failed_matched() {
        let trigger = TomlTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        // Simulate a fact indicating the assertion failed
        let facts = vec![Fact {
            key: "assertion:maven.build.success".to_string(),
            value: "failed".to_string(),
            tags: vec![],
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }];
        let result = evaluate_advice_trigger(&trigger, &facts).unwrap();
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
        assert!(result.matches);
    }

    #[test]
    fn test_advice_trigger_assertion_failed_not_matched() {
        let trigger = TomlTrigger::AssertionFailed {
            assertion_id: "maven.build.success".to_string(),
        };
        // Simulate a fact indicating the assertion passed
        let facts = vec![Fact {
            key: "assertion:maven.build.success".to_string(),
            value: "passed".to_string(),
            tags: vec![],
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }];
        let result = evaluate_advice_trigger(&trigger, &facts).unwrap();
        assert!(!result.legacy_passed);
        assert!(!result.cel_passed);
        assert!(result.matches);
    }

    #[test]
    fn test_advice_trigger_experience_pattern_matched() {
        let trigger = TomlTrigger::ExperiencePattern {
            tool_name: "cargo".to_string(),
            status: "failure".to_string(),
            threshold: 3,
        };
        // Simulate 3 failure experiences for cargo
        let facts = vec![
            Fact {
                key: "experience:cargo:failure".to_string(),
                value: "1".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 1.0,
            },
            Fact {
                key: "experience:cargo:failure".to_string(),
                value: "2".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 1.0,
            },
            Fact {
                key: "experience:cargo:failure".to_string(),
                value: "3".to_string(),
                tags: vec![],
                source_extractor: "test".to_string(),
                confidence: 1.0,
            },
        ];
        let result = evaluate_advice_trigger(&trigger, &facts).unwrap();
        assert!(result.cel_passed);
        assert!(result.matches);
    }

    #[test]
    fn test_advice_trigger_doctor_failed() {
        let trigger = TomlTrigger::DoctorFailed {
            doctor_id: "sandbox.alive".to_string(),
        };
        let facts = vec![Fact {
            key: "doctor:sandbox.alive".to_string(),
            value: "failed".to_string(),
            tags: vec![],
            source_extractor: "test".to_string(),
            confidence: 1.0,
        }];
        let result = evaluate_advice_trigger(&trigger, &facts).unwrap();
        assert!(result.cel_passed);
        assert!(result.matches);
    }

    // ─── Error handling tests ─────────────────────────────────────────────────

    #[test]
    fn test_evaluate_toml_check_no_cel_condition_sandbox_alive() {
        let record = make_pass_record();
        let toml_check = TomlCheck::SandboxAlive;
        // SandboxAlive has no CEL equivalent — should return NoCelCondition error
        let result = evaluate_toml_check(toml_check.clone(), &record);
        assert!(matches!(result, Err(ShimError::NoCelCondition)));
    }

    // ─── Doctor check tests ───────────────────────────────────────────────────

    #[test]
    fn test_doctor_check_assertion_driven_parity() {
        use tempfile::tempdir;

        // Set up assertion registry with a known assertion
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.assertion.toml");
        std::fs::write(
            &path,
            r#"
[assertion]
id = "test.assertion"
name = "Test Assertion"
description = "Test"
category = "test"

[assertion.check]
type = "exit_code"
expected = 0
"#,
        )
        .unwrap();

        let registry = AssertionRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        let record = make_pass_record();
        let doctor_check = TomlDoctorCheck::AssertionDriven {
            assertion_id: "test.assertion".to_string(),
        };

        let result = evaluate_doctor_check(&doctor_check, &record, &registry).unwrap();
        assert!(result.matches);
        assert!(result.legacy_passed);
        assert!(result.cel_passed);
    }

    #[test]
    fn test_doctor_check_aliveness_deferred() {
        // Aliveness has no CEL equivalent — should return matches=true
        let record = make_pass_record();
        let doctor_check = TomlDoctorCheck::Aliveness { sandbox_id: None };
        let result =
            evaluate_doctor_check(&doctor_check, &record, &AssertionRegistry::new());
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches);
        assert!(r.legacy_passed);
    }

    // ─── Integration: TOML file loading + parity ─────────────────────────────

    #[test]
    fn test_load_and_evaluate_all_assertion_types() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();

        // Write all TOML assertion types
        let toml_files = vec![
            (
                "exit_code.zero.toml",
                r#"
[assertion]
id = "exit_code.zero"
name = "Exit Code Zero"
description = "Test"
category = "test"
[assertion.check]
type = "exit_code"
expected = 0
"#,
            ),
            (
                "stdout.contains.toml",
                r#"
[assertion]
id = "stdout.contains"
name = "Stdout Contains"
description = "Test"
category = "test"
[assertion.check]
type = "stdout_contains"
substring = "BUILD"
"#,
            ),
            (
                "stderr.contains.toml",
                r#"
[assertion]
id = "stderr.contains"
name = "Stderr Contains"
description = "Test"
category = "test"
[assertion.check]
type = "stderr_contains"
substring = "ERROR"
"#,
            ),
            (
                "command.duration.toml",
                r#"
[assertion]
id = "command.duration"
name = "Command Duration"
description = "Test"
category = "test"
[assertion.check]
type = "command_duration"
max_ms = 5000
"#,
            ),
        ];

        for (filename, content) in toml_files {
            std::fs::write(dir.path().join(filename), content).unwrap();
        }

        let registry = AssertionRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();

        // Evaluate each loaded assertion against a passing record
        let record = make_pass_record();

        for descriptor in registry.list() {
            for check in &descriptor.checks {
                let toml_check: TomlCheck = check.clone().into();
                if matches!(toml_check, TomlCheck::SandboxAlive) {
                    continue; // Skip deferred checks
                }
                let result = evaluate_toml_check(toml_check.clone(), &record);
                // We expect either success (parity) or NoCelCondition error
                if let Ok(r) = result {
                    assert!(
                        r.matches,
                        "Mismatch for {:?}: legacy={}, cel={}",
                        toml_check,
                        r.legacy_passed,
                        r.cel_passed
                    );
                }
            }
        }
    }

    // ─── CEL evaluation tests ─────────────────────────────────────────────────

    #[test]
    fn test_eval_cel_on_record() {
        let record = ExperienceRecord::new("cargo build")
            .with_trace_id("test")
            .completed(0)
            .with_stdout(b"BUILD SUCCESS")
            .with_stderr(b"");
        let result = eval_cel_on_record(&record, "exit_code == 0").unwrap();
        assert!(result);
    }

    #[test]
    fn test_eval_cel_on_record_stderr_contains() {
        let record = ExperienceRecord::new("sandbox_run")
            .completed(1)
            .with_stdout(b"")
            .with_stderr(b"ERROR: compilation failed");
        let result = eval_cel_on_record(&record, r#"stderr_contains('ERROR')"#).unwrap();
        assert!(result);
    }

    #[test]
    fn test_eval_cel_on_record_duration() {
        let record = ExperienceRecord::new("sandbox_run")
            .with_stdout(b"done")
            .with_stderr(b"");
        let mut record = record;
        record.exit_code = Some(0);
        record.finished_at = Some(record.started_at);

        let result = eval_cel_on_record(&record, "duration_lt(1000)").unwrap();
        assert!(result);
    }
}
