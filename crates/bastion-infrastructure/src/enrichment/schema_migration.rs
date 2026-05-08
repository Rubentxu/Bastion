//! Schema migration runner for the enrichment runs database.
//!
//! Manages schema versioning with transactional, idempotent migrations.
//! Ensures backward compatibility while enabling future schema extensions.
//!
//! # Version History
//!
//! - **Version 1**: Initial schema (id, timestamp, command, enricher_id, exit_code,
//!   duration_ms, output_summary_stdout, output_summary_stderr, facts_count,
//!   derived_facts_count, rule_hits_count, diagnostics_count, artifact_count,
//!   confidence_avg, verdict, recommendation_count, error)
//! - **Version 2**: Adds `sandbox_id TEXT` column for multi-sandbox telemetry correlation

use rusqlite::Connection;

use bastion_domain::shared::DomainError;

/// Current schema version.
pub const CURRENT_VERSION: i32 = 2;

/// Schema migration runner.
///
/// Runs pending migrations on an open SQLite connection in a transactional manner.
/// Migrations are idempotent — checking `MAX(version)` before applying ensures
/// already-applied migrations are skipped on subsequent calls.
#[derive(Debug)]
pub struct SchemaMigration;

impl SchemaMigration {
    /// Run all pending migrations on the open connection.
    ///
    /// Idempotent: checks `MAX(version)` before applying any migration.
    /// Transactional: uses `BEGIN IMMEDIATE` to acquire a write lock; on failure,
    /// the transaction is rolled back and the database remains at its prior version.
    ///
    /// # Arguments
    ///
    /// * `conn` - An open SQLite connection (must be writable)
    ///
    /// # Errors
    ///
    /// Returns `DomainError` if migration fails (e.g., SQL error, constraint violation).
    /// On error, the database is rolled back to its pre-migration state.
    pub fn run(conn: &Connection) -> Result<(), DomainError> {
        let current = Self::get_version(conn)?;

        if current >= CURRENT_VERSION {
            tracing::debug!(
                current_version = current,
                "Schema up to date, no migrations needed"
            );
            return Ok(());
        }

        tracing::info!(
            current_version = current,
            target_version = CURRENT_VERSION,
            "Running schema migrations"
        );

        // Apply migrations in order
        if current < 2 {
            Self::migrate_v1_to_v2(conn)?;
        }

        Ok(())
    }

    /// Get the current schema version from the database.
    ///
    /// Returns `Ok(0)` if the `schema_version` table does not exist yet (fresh DB).
    fn get_version(conn: &Connection) -> Result<i32, DomainError> {
        // The schema_version table may not exist on fresh databases.
        // We use `CREATE TABLE IF NOT EXISTS` and then query.
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                description TEXT
            )
            "#,
        )
        .map_err(|e| {
            DomainError::Internal(format!("Failed to ensure schema_version table: {}", e))
        })?;

        let version: Option<i32> = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .map_err(|e| DomainError::Internal(format!("Failed to query schema version: {}", e)))?;

        Ok(version.unwrap_or(0))
    }

    /// Migrate from v1 to v2: adds `sandbox_id TEXT` column.
    ///
    /// This migration is non-destructive — uses `ALTER TABLE ... ADD COLUMN` which
    /// preserves all existing data. Old rows get `NULL` for the new column.
    ///
    /// Uses `BEGIN IMMEDIATE` to serialize concurrent access and ensure consistent rollback.
    fn migrate_v1_to_v2(conn: &Connection) -> Result<(), DomainError> {
        tracing::info!("Applying migration v1 -> v2: adding sandbox_id column");

        // Use BEGIN IMMEDIATE to acquire a write lock immediately
        conn.execute("BEGIN IMMEDIATE", []).map_err(|e| {
            DomainError::Internal(format!("Failed to begin migration transaction: {}", e))
        })?;

        let result = (|| {
            // Double-check version inside transaction (another process may have migrated)
            let current: Option<i32> = conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                    row.get(0)
                })
                .map_err(|e| DomainError::Internal(format!("Failed to re-check version: {}", e)))?;

            if current.unwrap_or(0) >= 2 {
                tracing::debug!("Version already at v2 inside transaction, rolling back");
                return Ok(());
            }

            // Add sandbox_id column using ALTER TABLE ADD COLUMN
            // This is safe: existing rows get NULL, no data loss
            conn.execute("ALTER TABLE enrichment_runs ADD COLUMN sandbox_id TEXT", [])
                .map_err(|e| {
                    DomainError::Internal(format!("Failed to add sandbox_id column: {}", e))
                })?;

            // Record the migration in schema_version
            let applied_at = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO schema_version (version, applied_at, description) VALUES (2, ?1, 'add sandbox_id')",
                [applied_at],
            )
            .map_err(|e| DomainError::Internal(format!("Failed to record v2 migration: {}", e)))?;

            tracing::info!("Migration v1->v2 completed successfully");
            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute("COMMIT", []).map_err(|e| {
                    DomainError::Internal(format!("Failed to commit migration: {}", e))
                })?;
                Ok(())
            }
            Err(e) => {
                // Rollback on failure
                let _ = conn.execute("ROLLBACK", []);
                tracing::error!(error = %e, "Migration v1->v2 failed, rolled back");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_v1_schema(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichment_runs (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                output_summary_stdout TEXT,
                output_summary_stderr TEXT,
                facts_count INTEGER DEFAULT 0,
                derived_facts_count INTEGER DEFAULT 0,
                rule_hits_count INTEGER DEFAULT 0,
                diagnostics_count INTEGER DEFAULT 0,
                artifact_count INTEGER DEFAULT 0,
                confidence_avg REAL DEFAULT 0.0,
                verdict TEXT,
                recommendation_count INTEGER DEFAULT 0,
                error TEXT
            );
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                description TEXT
            );
            INSERT INTO schema_version (version, applied_at, description) VALUES (1, '2024-01-01T00:00:00Z', 'initial schema');
            "#,
        )
        .unwrap();
    }

    fn create_fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Create v1 schema directly (simulating existing DB without migration)
        create_v1_schema(&conn);
        conn
    }

    fn create_v2_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichment_runs (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                output_summary_stdout TEXT,
                output_summary_stderr TEXT,
                facts_count INTEGER DEFAULT 0,
                derived_facts_count INTEGER DEFAULT 0,
                rule_hits_count INTEGER DEFAULT 0,
                diagnostics_count INTEGER DEFAULT 0,
                artifact_count INTEGER DEFAULT 0,
                confidence_avg REAL DEFAULT 0.0,
                verdict TEXT,
                recommendation_count INTEGER DEFAULT 0,
                error TEXT,
                sandbox_id TEXT
            );
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                description TEXT
            );
            INSERT INTO schema_version (version, applied_at, description) VALUES (2, '2024-01-01T00:00:00Z', 'add sandbox_id');
            "#,
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_get_version_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn test_get_version_v1() {
        let conn = create_fresh_db();
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_get_version_v2() {
        let conn = create_v2_db();
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn test_run_no_op_when_current() {
        let conn = create_v2_db();
        // Should not error, just return immediately
        SchemaMigration::run(&conn).unwrap();

        // Verify still at v2
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 2);

        // Verify sandbox_id column still exists
        let col_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('enrichment_runs') WHERE name = 'sandbox_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 1);
    }

    #[test]
    fn test_migrate_v1_to_v2() {
        let conn = create_fresh_db();

        // Verify sandbox_id doesn't exist before migration
        let col_before: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('enrichment_runs') WHERE name = 'sandbox_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_before, 0);

        // Run migration
        SchemaMigration::run(&conn).unwrap();

        // Verify sandbox_id now exists
        let col_after: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('enrichment_runs') WHERE name = 'sandbox_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_after, 1);

        // Verify version is now 2
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn test_migration_idempotent() {
        let conn = create_fresh_db();

        // Run migration twice
        SchemaMigration::run(&conn).unwrap();
        SchemaMigration::run(&conn).unwrap();

        // Should still be at v2 with sandbox_id
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 2);

        let col_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('enrichment_runs') WHERE name = 'sandbox_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 1);
    }

    #[test]
    fn test_migration_preserves_existing_data() {
        let conn = create_fresh_db();

        // Insert a record before migration
        conn.execute(
            r#"
            INSERT INTO enrichment_runs (id, timestamp, command, enricher_id, exit_code, duration_ms)
            VALUES ('test-1', '2024-01-01T00:00:00Z', 'mvn package', 'maven', 0, 5000)
            "#,
            [],
        )
        .unwrap();

        // Run migration
        SchemaMigration::run(&conn).unwrap();

        // Verify record still exists and sandbox_id is NULL
        let (id, sandbox_id): (String, Option<String>) = conn
            .query_row(
                "SELECT id, sandbox_id FROM enrichment_runs WHERE id = 'test-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(id, "test-1");
        assert_eq!(sandbox_id, None);
    }

    #[test]
    fn test_run_creates_schema_version_if_missing() {
        let conn = Connection::open_in_memory().unwrap();

        // Create enrichment_runs table but no schema_version
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichment_runs (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                output_summary_stdout TEXT,
                output_summary_stderr TEXT,
                facts_count INTEGER DEFAULT 0,
                derived_facts_count INTEGER DEFAULT 0,
                rule_hits_count INTEGER DEFAULT 0,
                diagnostics_count INTEGER DEFAULT 0,
                artifact_count INTEGER DEFAULT 0,
                confidence_avg REAL DEFAULT 0.0,
                verdict TEXT,
                recommendation_count INTEGER DEFAULT 0,
                error TEXT
            );
            "#,
        )
        .unwrap();

        // Run should create schema_version and migrate
        SchemaMigration::run(&conn).unwrap();

        // Verify version is now 2
        let version = SchemaMigration::get_version(&conn).unwrap();
        assert_eq!(version, 2);

        // Verify sandbox_id column exists
        let col_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('enrichment_runs') WHERE name = 'sandbox_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 1);
    }
}
