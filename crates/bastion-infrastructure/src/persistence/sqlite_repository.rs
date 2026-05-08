//! SQLite-backed sandbox repository.
//!
//! Provides persistent storage for sandboxes using rusqlite.

use std::path::Path;

use async_trait::async_trait;
use rusqlite::params;
use tokio::sync::Mutex;

use bastion_domain::sandbox::entity::Sandbox;
use bastion_domain::sandbox::repository::SandboxRepository;
use bastion_domain::sandbox::value_objects::{NetworkSpec, ResourcesSpec, SandboxStatus};
use bastion_domain::shared::DomainError;
use bastion_domain::shared::id::{ProviderId, SandboxId, TemplateId};

/// SQLite-backed implementation of `SandboxRepository`.
#[derive(Debug)]
pub struct SqliteSandboxRepository {
    conn: Mutex<rusqlite::Connection>,
    #[allow(dead_code)]
    db_path: std::path::PathBuf,
}

impl SqliteSandboxRepository {
    /// Create a new repository, creating the DB schema if it doesn't exist.
    pub fn new(db_path: &Path) -> Result<Self, DomainError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DomainError::Internal(format!("Failed to create DB directory: {}", e))
            })?;
        }

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sandboxes (
                id TEXT PRIMARY KEY,
                template_id TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL,
                expires_at TEXT,
                resources TEXT NOT NULL,
                network TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}'
            );
            "#,
        )
        .map_err(|e| DomainError::Internal(format!("Failed to create schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    /// Reconcile sandboxes from a provider with the local database.
    ///
    /// - Sandboxes present in the provider but absent in DB are inserted with status Running.
    /// - Sandboxes in DB but absent from the provider are marked as Stopped.
    pub async fn sync_from_provider(
        &self,
        provider: &dyn bastion_domain::provider::port::SandboxProvider,
    ) -> Result<(), DomainError> {
        use bastion_domain::sandbox::value_objects::SandboxFilter;

        // List sandboxes from the provider
        let provider_sandboxes = provider
            .list_sandboxes(&SandboxFilter::default())
            .await
            .map_err(|e| DomainError::Internal(format!("Provider list_sandboxes failed: {}", e)))?;

        let provider_ids: std::collections::HashSet<String> = provider_sandboxes
            .iter()
            .map(|s| s.id.to_string())
            .collect();

        // Lock the connection for the entire transaction
        let mut conn = self.conn.lock().await;

        // Mark sandboxes in DB that are not in the provider as Stopped
        // Collect IDs of running sandboxes that need to be marked as stopped
        // (doing this before the update to avoid borrow conflicts)
        let stopped_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id, status FROM sandboxes")
                .map_err(|e| {
                    DomainError::Internal(format!("Failed to prepare statement: {}", e))
                })?;

            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| DomainError::Internal(format!("Failed to query sandboxes: {}", e)))?;

            let mut ids = Vec::new();
            for row in rows {
                let (id, status): (String, String) =
                    row.map_err(|e| DomainError::Internal(format!("Failed to read row: {}", e)))?;
                if !provider_ids.contains(&id) && status == "running" {
                    ids.push(id);
                }
            }
            ids
        }; // stmt is dropped here, releasing the immutable borrow

        // Mark stopped sandboxes
        for id in stopped_ids {
            let mut update_stmt = conn
                .prepare("UPDATE sandboxes SET status = 'stopped' WHERE id = ?1")
                .map_err(|e| DomainError::Internal(format!("Failed to prepare update: {}", e)))?;
            update_stmt
                .execute(params![id])
                .map_err(|e| DomainError::Internal(format!("Failed to update sandbox: {}", e)))?;
        }

        // Insert provider sandboxes that are not in DB
        for sandbox in &provider_sandboxes {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sandboxes WHERE id = ?1",
                    params![sandbox.id.to_string()],
                    |_| Ok(true),
                )
                .unwrap_or(false);

            if !exists {
                Self::insert_sandbox(&mut conn, sandbox)?;
            }
        }

        Ok(())
    }

    fn insert_sandbox(
        conn: &mut rusqlite::Connection,
        sandbox: &Sandbox,
    ) -> Result<(), DomainError> {
        let resources_json = serde_json::to_string(&sandbox.resources)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize resources: {}", e)))?;
        let network_json = serde_json::to_string(&sandbox.network)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize network: {}", e)))?;
        let metadata_json = serde_json::to_string(&sandbox.metadata)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize metadata: {}", e)))?;
        let expires_at_str = sandbox.expires_at.map(|dt| dt.to_rfc3339());

        conn.execute(
            r#"INSERT INTO sandboxes
               (id, template_id, provider_id, status, created_at, expires_at, resources, network, metadata)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            params![
                sandbox.id.to_string(),
                sandbox.template_id.to_string(),
                sandbox.provider_id.to_string(),
                sandbox.status.to_string(),
                sandbox.created_at.to_rfc3339(),
                expires_at_str,
                resources_json,
                network_json,
                metadata_json,
            ],
        )
        .map_err(|e| DomainError::Internal(format!("Failed to insert sandbox: {}", e)))?;

        Ok(())
    }
}

#[async_trait]
impl SandboxRepository for SqliteSandboxRepository {
    async fn save(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
        let mut conn = self.conn.lock().await;

        // Check if exists
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sandboxes WHERE id = ?1",
                params![sandbox.id.to_string()],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if exists {
            return Err(DomainError::AlreadyExists(sandbox.id.to_string()));
        }

        Self::insert_sandbox(&mut conn, sandbox)
    }

    async fn find_by_id(&self, id: &SandboxId) -> Result<Option<Sandbox>, DomainError> {
        let conn = self.conn.lock().await;

        let row = match conn.query_row(
            "SELECT id, template_id, provider_id, status, created_at, expires_at, resources, network, metadata
             FROM sandboxes WHERE id = ?1",
            params![id.to_string()],
            |row| {
                Ok(SandboxRow {
                    id: row.get(0)?,
                    template_id: row.get(1)?,
                    provider_id: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    resources: row.get(6)?,
                    network: row.get(7)?,
                    metadata: row.get(8)?,
                })
            },
        ) {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => {
                return Err(DomainError::Internal(format!(
                    "Failed to query sandbox: {}",
                    e
                )))
            }
        };

        let sandbox = row_to_sandbox(row)?;
        Ok(Some(sandbox))
    }

    async fn update(&self, sandbox: &Sandbox) -> Result<(), DomainError> {
        let conn = self.conn.lock().await;

        let resources_json = serde_json::to_string(&sandbox.resources)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize resources: {}", e)))?;
        let network_json = serde_json::to_string(&sandbox.network)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize network: {}", e)))?;
        let metadata_json = serde_json::to_string(&sandbox.metadata)
            .map_err(|e| DomainError::Internal(format!("Failed to serialize metadata: {}", e)))?;
        let expires_at_str = sandbox.expires_at.map(|dt| dt.to_rfc3339());

        let rows_affected = conn
            .execute(
                r#"UPDATE sandboxes SET
                   template_id = ?2, provider_id = ?3, status = ?4, created_at = ?5,
                   expires_at = ?6, resources = ?7, network = ?8, metadata = ?9
                   WHERE id = ?1"#,
                params![
                    sandbox.id.to_string(),
                    sandbox.template_id.to_string(),
                    sandbox.provider_id.to_string(),
                    sandbox.status.to_string(),
                    sandbox.created_at.to_rfc3339(),
                    expires_at_str,
                    resources_json,
                    network_json,
                    metadata_json,
                ],
            )
            .map_err(|e| DomainError::Internal(format!("Failed to update sandbox: {}", e)))?;

        if rows_affected == 0 {
            return Err(DomainError::NotFound(sandbox.id.to_string()));
        }

        Ok(())
    }

    async fn delete(&self, id: &SandboxId) -> Result<(), DomainError> {
        let conn = self.conn.lock().await;

        let rows_affected = conn
            .execute(
                "DELETE FROM sandboxes WHERE id = ?1",
                params![id.to_string()],
            )
            .map_err(|e| DomainError::Internal(format!("Failed to delete sandbox: {}", e)))?;

        if rows_affected == 0 {
            return Err(DomainError::NotFound(id.to_string()));
        }

        Ok(())
    }

    async fn find_active(&self) -> Result<Vec<Sandbox>, DomainError> {
        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT id, template_id, provider_id, status, created_at, expires_at, resources, network, metadata
                 FROM sandboxes WHERE status IN ('running', 'pending')",
            )
            .map_err(|e| DomainError::Internal(format!("Failed to prepare statement: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(SandboxRow {
                    id: row.get(0)?,
                    template_id: row.get(1)?,
                    provider_id: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    resources: row.get(6)?,
                    network: row.get(7)?,
                    metadata: row.get(8)?,
                })
            })
            .map_err(|e| DomainError::Internal(format!("Failed to query sandboxes: {}", e)))?;

        let mut sandboxes = Vec::new();
        for row in rows {
            let row =
                row.map_err(|e| DomainError::Internal(format!("Failed to read row: {}", e)))?;
            sandboxes.push(row_to_sandbox(row)?);
        }

        Ok(sandboxes)
    }

    async fn find_expired(&self) -> Result<Vec<Sandbox>, DomainError> {
        let conn = self.conn.lock().await;

        let now = chrono::Utc::now().to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT id, template_id, provider_id, status, created_at, expires_at, resources, network, metadata
                 FROM sandboxes WHERE status IN ('running', 'pending') AND expires_at IS NOT NULL AND expires_at < ?",
            )
            .map_err(|e| DomainError::Internal(format!("Failed to prepare statement: {}", e)))?;

        let rows = stmt
            .query_map([&now], |row| {
                Ok(SandboxRow {
                    id: row.get(0)?,
                    template_id: row.get(1)?,
                    provider_id: row.get(2)?,
                    status: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    resources: row.get(6)?,
                    network: row.get(7)?,
                    metadata: row.get(8)?,
                })
            })
            .map_err(|e| {
                DomainError::Internal(format!("Failed to query expired sandboxes: {}", e))
            })?;

        let mut sandboxes = Vec::new();
        for row in rows {
            let row =
                row.map_err(|e| DomainError::Internal(format!("Failed to read row: {}", e)))?;
            sandboxes.push(row_to_sandbox(row)?);
        }

        Ok(sandboxes)
    }
}

/// Helper struct for reading a sandbox row from SQLite.
#[derive(Debug)]
struct SandboxRow {
    id: String,
    template_id: String,
    provider_id: String,
    status: String,
    created_at: String,
    expires_at: Option<String>,
    resources: String,
    network: String,
    metadata: String,
}

fn row_to_sandbox(row: SandboxRow) -> Result<Sandbox, DomainError> {
    let status =
        serde_json::from_str::<SandboxStatus>(&format!("\"{}\"", row.status)).map_err(|e| {
            DomainError::Internal(format!("Failed to parse status '{}': {}", row.status, e))
        })?;
    let resources: ResourcesSpec = serde_json::from_str(&row.resources)
        .map_err(|e| DomainError::Internal(format!("Failed to deserialize resources: {}", e)))?;
    let network: NetworkSpec = serde_json::from_str(&row.network)
        .map_err(|e| DomainError::Internal(format!("Failed to deserialize network: {}", e)))?;
    let metadata: std::collections::HashMap<String, String> =
        serde_json::from_str(&row.metadata)
            .map_err(|e| DomainError::Internal(format!("Failed to deserialize metadata: {}", e)))?;

    let created_at = chrono::DateTime::parse_from_rfc3339(&row.created_at)
        .map_err(|e| DomainError::Internal(format!("Failed to parse created_at: {}", e)))?
        .with_timezone(&chrono::Utc);

    let expires_at = row
        .expires_at
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| DomainError::Internal(format!("Failed to parse expires_at: {}", e)))
        })
        .transpose()?;

    Ok(Sandbox {
        id: SandboxId::new(row.id),
        template_id: TemplateId::new(row.template_id),
        provider_id: ProviderId::new(row.provider_id),
        status,
        created_at,
        expires_at,
        resources,
        network,
        metadata,
    })
}
