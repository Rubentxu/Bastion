//! Tests for MetricsCollector — latency stats, flakiness, and regression detection.

/// Feature-gated MetricsCollector import — use crate path for internal tests.
#[cfg(feature = "test-metrics")]
use crate::MetricsCollector;

#[cfg(feature = "test-metrics")]
mod latency_tests {
    use super::*;
    use crate::types::MetricsError;

    #[test]
    fn record_test_writes_row() {
        // GIVEN an in-memory metrics collector
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        // WHEN we record a test run
        metrics.record_test(
            "pool_init",
            450,
            "pass",
            "bastion-infrastructure",
            "tests/pool_test.rs",
        );

        // THEN: record was written (check via raw query) and insufficient samples error
        // (per spec MC-004, latency_stats requires ≥3 runs)
        use rusqlite::Connection;
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM test_runs WHERE test_name = 'pool_init'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "Should have exactly 1 row written for pool_init");

        // But latency_stats requires ≥3 samples (per spec MC-004)
        let result = metrics.latency_stats("pool_init");
        assert!(
            result.is_err(),
            "Should return InsufficientSamples for < 3 runs"
        );
    }

    #[test]
    fn latency_stats_p50_p95_p99() {
        // GIVEN a collector with 5 runs: durations [100, 200, 300, 400, 500]ms
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        for duration in [100_u64, 200, 300, 400, 500] {
            metrics.record_test(
                "pool_latency_test",
                duration,
                "pass",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
        }

        // WHEN we query latency stats
        let stats = metrics.latency_stats("pool_latency_test").unwrap();

        // THEN percentiles match expected values
        assert_eq!(stats.p50_ms, 300, "p50 should be median (300)");
        assert_eq!(stats.p95_ms, 500, "p95 should be near max (500)");
        assert_eq!(stats.p99_ms, 500, "p99 should be max (500)");
        assert_eq!(stats.sample_count, 5, "Should have 5 samples");
    }

    #[test]
    fn latency_stats_insufficient_samples() {
        // GIVEN a collector with only 2 runs (less than minimum 3)
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        metrics.record_test(
            "not_enough",
            100,
            "pass",
            "bastion-infrastructure",
            "tests/pool_test.rs",
        );
        metrics.record_test(
            "not_enough",
            200,
            "pass",
            "bastion-infrastructure",
            "tests/pool_test.rs",
        );

        // WHEN we query latency stats
        let result = metrics.latency_stats("not_enough");

        // THEN we get an InsufficientSamples error
        assert!(result.is_err(), "Should return error for < 3 samples");
        let err = result.unwrap_err();
        assert!(
            matches!(err, MetricsError::InsufficientSamples { .. }),
            "Should be InsufficientSamples error"
        );
    }

    #[test]
    fn flakiness_score_calculation() {
        // GIVEN a collector with 10 runs: 7 pass, 3 fail
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        for _ in 0..7 {
            metrics.record_test(
                "e2e_lifecycle",
                500,
                "pass",
                "bastion-gateway",
                "tests/e2e_test.rs",
            );
        }
        for _ in 0..3 {
            metrics.record_test(
                "e2e_lifecycle",
                500,
                "fail",
                "bastion-gateway",
                "tests/e2e_test.rs",
            );
        }

        // WHEN we query flakiness score
        let score = metrics.flakiness_score("e2e_lifecycle").unwrap();

        // THEN score is 0.3 (3 failed / 10 total)
        assert!(
            (score - 0.3).abs() < 0.001,
            "Flakiness should be 0.3, got {}",
            score
        );
    }

    #[test]
    fn flakiness_score_zero_runs() {
        // GIVEN no recorded runs for a test
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        // WHEN we query flakiness for non-existent test
        let score = metrics.flakiness_score("nonexistent_test").unwrap();

        // THEN flakiness is 0.0
        assert!(
            (score - 0.0).abs() < 0.001,
            "Flakiness for no runs should be 0.0"
        );
    }

    #[test]
    fn flakiness_score_all_fail() {
        // GIVEN 5 runs, all fail
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let metrics = MetricsCollector::new(&db_path).unwrap();

        for _ in 0..5 {
            metrics.record_test(
                "flaky_always",
                100,
                "fail",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
        }

        // WHEN we query flakiness score
        let score = metrics.flakiness_score("flaky_always").unwrap();

        // THEN score is 1.0
        assert!(
            (score - 1.0).abs() < 0.001,
            "Flakiness for all-fail should be 1.0"
        );
    }

    #[test]
    fn regression_detect_finds_regression() {
        // GIVEN baseline db with P95=200ms and current db with P95=500ms
        let baseline_dir = tempfile::tempdir().unwrap();
        let current_dir = tempfile::tempdir().unwrap();

        let baseline_db = baseline_dir.path().join("baseline.db");
        let current_db = current_dir.path().join("current.db");

        let baseline_metrics = MetricsCollector::new(&baseline_db).unwrap();
        let current_metrics = MetricsCollector::new(&current_db).unwrap();

        // Baseline: [100, 150, 200, 250, 300] ms -> P95 ≈ 300
        for duration in [100_u64, 150, 200, 250, 300] {
            baseline_metrics.record_test(
                "pool_latency",
                duration,
                "pass",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
        }

        // Current: [300, 400, 500, 600, 700] ms -> P95 ≈ 700 (regression!)
        for duration in [300_u64, 400, 500, 600, 700] {
            current_metrics.record_test(
                "pool_latency",
                duration,
                "pass",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
        }

        // WHEN we run regression detection with 20% threshold
        let regressions = MetricsCollector::regression_detect(&baseline_db, &current_db).unwrap();

        // THEN pool_latency is flagged
        assert!(
            !regressions.is_empty(),
            "Should detect at least one regression"
        );
        let r = &regressions[0];
        assert_eq!(r.test_name, "pool_latency");
        assert!(r.delta_pct > 20.0, "Delta should exceed 20% threshold");
    }

    #[test]
    fn regression_detect_no_regression() {
        // GIVEN two identical databases
        let dir = tempfile::tempdir().unwrap();
        let db1_path = dir.path().join("db1.db");
        let db2_path = dir.path().join("db2.db");

        let metrics1 = MetricsCollector::new(&db1_path).unwrap();
        let metrics2 = MetricsCollector::new(&db2_path).unwrap();

        for duration in [100_u64, 200, 300, 400, 500] {
            metrics1.record_test(
                "stable_test",
                duration,
                "pass",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
            metrics2.record_test(
                "stable_test",
                duration,
                "pass",
                "bastion-infrastructure",
                "tests/pool_test.rs",
            );
        }

        // WHEN we run regression detection
        let regressions = MetricsCollector::regression_detect(&db1_path, &db2_path).unwrap();

        // THEN no regressions found
        assert!(
            regressions.is_empty(),
            "Should not detect regression for identical distributions"
        );
    }
}

#[cfg(not(feature = "test-metrics"))]
mod noop_tests {
    #[test]
    fn noop_metrics_record_test_compiles() {
        // GIVEN no metrics feature
        // WHEN we try to use MetricsCollector::noop()
        // THEN it compiles (no-op stub)
        let metrics = crate::MetricsCollector::noop();
        // Fire-and-forget should be a no-op
        metrics.record_test("test", 100, "pass", "crate", "file.rs");
    }
}
