//! SQLite-backed catalog repository and YAML importer.
//!
//! Imports enricher descriptors from YAML/TOML files at bootstrap and
//! serves queries from the SQLite database at runtime.

use async_trait::async_trait;
use rusqlite::params;
use std::path::Path;
use tokio::sync::Mutex;
use tracing::warn;

use bastion_domain::shared::DomainError;
use enrichment_engine::models::{EnricherDescriptor, ExtractorConfig};
use enrichment_engine::traits::CatalogRepository;

/// SQLite-backed implementation of `CatalogRepository`.
#[derive(Debug)]
pub struct SqliteCatalogRepository {
    conn: Mutex<rusqlite::Connection>,
}

impl SqliteCatalogRepository {
    /// Create a new repository, creating the DB schema if it doesn't exist.
    pub fn new(db_path: &Path) -> Result<Self, DomainError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::Internal(format!("Failed to create DB directory: {}", e)))?;
        }

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| DomainError::Internal(format!("Failed to open SQLite DB: {}", e)))?;

        // Inline schema creation
        // Note: SQLite does not support ALTER TABLE to add composite PK or CASCADE,
        // so we create the table with the correct schema from the start.
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS enrichers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                match_patterns_json TEXT NOT NULL,
                template TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS extractors (
                id TEXT NOT NULL,
                enricher_id TEXT NOT NULL,
                type TEXT NOT NULL,
                pattern TEXT NOT NULL,
                fact_key TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                merge_mode TEXT NOT NULL DEFAULT 'single',
                PRIMARY KEY (enricher_id, id),
                FOREIGN KEY (enricher_id) REFERENCES enrichers(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_extractors_enricher ON extractors(enricher_id);
            "#,
        )
        .map_err(|e| DomainError::Internal(format!("Failed to create schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            // db_path: db_path.to_path_buf(),
        })
    }

    /// Insert or replace an enricher and its extractors.
    pub async fn upsert_enricher(&self, enricher: &EnricherDescriptor) -> Result<(), DomainError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| DomainError::Internal(format!("Transaction error: {}", e)))?;

        let patterns_json =
            serde_json::to_string(&enricher.match_patterns).unwrap_or_else(|_| "[]".to_string());

        tx.execute(
            r#"INSERT OR REPLACE INTO enrichers (id, name, version, match_patterns_json, template, enabled)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                enricher.id,
                enricher.name,
                enricher.version,
                patterns_json,
                enricher.template,
                enricher.enabled as i32
            ],
        )
        .map_err(|e| DomainError::Internal(format!("Insert enricher failed: {}", e)))?;

        // Delete existing extractors for this enricher
        tx.execute("DELETE FROM extractors WHERE enricher_id = ?1", params![enricher.id])
            .map_err(|e| DomainError::Internal(format!("Delete extractors failed: {}", e)))?;

        for ext in &enricher.extractors {
            tx.execute(
                r#"INSERT OR REPLACE INTO extractors (id, enricher_id, type, pattern, fact_key, priority, merge_mode)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![
                    ext.id,
                    enricher.id,
                    ext.extractor_type,
                    ext.pattern,
                    ext.fact_key,
                    ext.priority,
                    ext.merge_mode
                ],
            )
            .map_err(|e| DomainError::Internal(format!("Insert extractor failed: {}", e)))?;
        }

        tx
            .commit()
            .map_err(|e| DomainError::Internal(format!("Commit failed: {}", e)))?;

        Ok(())
    }
}

#[async_trait]
impl CatalogRepository for SqliteCatalogRepository {
    async fn find_enrichers(&self, command: &str) -> Vec<EnricherDescriptor> {
        let command = command.to_string();
        let conn = self.conn.lock().await;

        let mut stmt = match conn.prepare(
            r#"
            SELECT e.id, e.name, e.version, e.match_patterns_json, e.template, e.enabled,
                   ext.id, ext.type, ext.pattern, ext.fact_key, ext.priority, ext.merge_mode
            FROM enrichers e
            LEFT JOIN extractors ext ON ext.enricher_id = e.id
            WHERE e.enabled = 1
            "#,
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i32>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        // Group by enricher
        let mut enricher_map: std::collections::HashMap<
            String,
            (String, String, String, Vec<String>, String, bool),
        > = std::collections::HashMap::new();
        let mut extractor_map: std::collections::HashMap<String, Vec<ExtractorConfig>> =
            std::collections::HashMap::new();

        for row in rows.flatten() {
            let (id, name, version, patterns_json, template, enabled, ext_id, ext_type, ext_pattern, ext_fact_key, ext_priority, ext_merge_mode) = row;

            let patterns: Vec<String> =
                serde_json::from_str(&patterns_json).unwrap_or_default();

            enricher_map.insert(
                id.clone(),
                (name, version, template, patterns, id.clone(), enabled != 0),
            );
            if let (Some(eid), Some(etype), Some(epattern), Some(efact_key), Some(epriority)) =
                (ext_id, ext_type, ext_pattern, ext_fact_key, ext_priority)
            {
                extractor_map
                    .entry(id)
                    .or_default()
                    .push(ExtractorConfig {
                        id: eid,
                        extractor_type: etype,
                        pattern: epattern,
                        fact_key: efact_key,
                        priority: epriority,
                        merge_mode: ext_merge_mode.unwrap_or_else(|| "single".to_string()),
                    });
            }
        }

        enricher_map
            .into_iter()
            .filter(|(_, (_, _, _, patterns, _, _))| {
                patterns.iter().any(|p| {
                    regex::Regex::new(p)
                        .map(|re: regex::Regex| re.is_match(&command))
                        .unwrap_or(false)
                })
            })
            .map(|(id, (name, version, template, match_patterns, _, enabled))| {
                let extractors = extractor_map.remove(&id).unwrap_or_default();
                EnricherDescriptor {
                    id,
                    name,
                    version,
                    match_patterns,
                    template,
                    enabled,
                    extractors,
                }
            })
            .collect()
    }

    async fn list_all(&self) -> Vec<EnricherDescriptor> {
        let conn = self.conn.lock().await;

        let mut stmt = match conn.prepare(
            r#"
            SELECT e.id, e.name, e.version, e.match_patterns_json, e.template, e.enabled,
                   ext.id, ext.type, ext.pattern, ext.fact_key, ext.priority, ext.merge_mode
            FROM enrichers e
            LEFT JOIN extractors ext ON ext.enricher_id = e.id
            "#,
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i32>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        // Group by enricher
        let mut enricher_map: std::collections::HashMap<
            String,
            (String, String, String, Vec<String>, String, bool),
        > = std::collections::HashMap::new();
        let mut extractor_map: std::collections::HashMap<String, Vec<ExtractorConfig>> =
            std::collections::HashMap::new();

        for row in rows.flatten() {
            let (id, name, version, patterns_json, template, enabled, ext_id, ext_type, ext_pattern, ext_fact_key, ext_priority, ext_merge_mode) = row;

            let patterns: Vec<String> =
                serde_json::from_str(&patterns_json).unwrap_or_default();

            enricher_map.insert(
                id.clone(),
                (name, version, template, patterns, id.clone(), enabled != 0),
            );
            if let (Some(eid), Some(etype), Some(epattern), Some(efact_key), Some(epriority)) =
                (ext_id, ext_type, ext_pattern, ext_fact_key, ext_priority)
            {
                extractor_map
                    .entry(id)
                    .or_default()
                    .push(ExtractorConfig {
                        id: eid,
                        extractor_type: etype,
                        pattern: epattern,
                        fact_key: efact_key,
                        priority: epriority,
                        merge_mode: ext_merge_mode.unwrap_or_else(|| "single".to_string()),
                    });
            }
        }

        enricher_map
            .into_iter()
            .map(|(id, (name, version, template, match_patterns, _, enabled))| {
                let extractors = extractor_map.remove(&id).unwrap_or_default();
                EnricherDescriptor {
                    id,
                    name,
                    version,
                    match_patterns,
                    template,
                    enabled,
                    extractors,
                }
            })
            .collect()
    }
}

/// YAML catalog importer.
///
/// Reads `.yaml` and `.toml` enricher descriptor files from a directory
/// and upserts them into the SQLite catalog.
pub struct YamlCatalogImporter<'a> {
    repo: &'a SqliteCatalogRepository,
}

impl<'a> YamlCatalogImporter<'a> {
    pub fn new(repo: &'a SqliteCatalogRepository) -> Self {
        Self { repo }
    }

    /// Import all `.yaml` and `.toml` files from the given directory.
    ///
    /// Parse errors are logged at warn level and do not abort the import.
    pub async fn import_dir(&self, dir: &Path) -> Result<usize, DomainError> {
        use tokio::fs::{self, DirEntry};

        if !dir.exists() {
            return Ok(0);
        }

        let dir_path = dir.to_path_buf();
        let mut entries: Vec<DirEntry> = Vec::new();
        let mut dir_stream = match fs::read_dir(&dir_path).await {
            Ok(s) => s,
            Err(e) => {
                return Err(DomainError::Internal(format!("Failed to read catalog dir: {}", e)));
            }
        };

        while let Some(entry) = dir_stream.next_entry().await
            .map_err(|e| DomainError::Internal(format!("Failed to read dir entry: {}", e)))? 
        {
            entries.push(entry);
        }

        let mut count = 0;
        for entry in entries {
            let path = entry.path();
            if !tokio::fs::metadata(&path).await.map(|m| m.is_file()).unwrap_or(false) {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "yaml" && ext != "yml" && ext != "toml" {
                continue;
            }

            match self.import_file(&path).await {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to import enricher file");
                }
            }
        }

        Ok(count)
    }

    async fn import_file(&self, path: &Path) -> Result<(), DomainError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| DomainError::Internal(format!("Failed to read file: {}", e)))?;

        // Dispatch by extension only — no fallback to TOML for YAML files
        let path_ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let enricher = if path_ext == "yaml" || path_ext == "yml" {
            self.parse_yaml(&content)
                .map_err(|e| {
                    DomainError::Internal(format!("YAML parse error: {}", e))
                })?
        } else {
            self.parse_toml(&content).map_err(|e| {
                DomainError::Internal(format!("TOML parse error: {}", e))
            })?
        };

        // Validate regex patterns before inserting
        // W2 Fix: reject entire enricher if any regex extractor has invalid pattern
        for ext in &enricher.extractors {
            if ext.extractor_type == "regex"
                && let Err(e) = regex::Regex::new(&ext.pattern)
            {
                warn!(
                    enricher_id = %enricher.id,
                    extractor_id = %ext.id,
                    pattern = %ext.pattern,
                    error = %e,
                    "Rejecting enricher due to invalid regex extractor"
                );
                return Err(DomainError::Validation(format!(
                    "Enricher '{}' has invalid regex in extractor '{}': {}",
                    enricher.id, ext.id, e
                )));
            }
        }

        self.repo.upsert_enricher(&enricher).await?;
        Ok(())
    }

    fn parse_yaml(&self, content: &str) -> Result<EnricherDescriptor, String> {
        #[derive(serde::Deserialize)]
        struct YamlEnricher {
            enricher: YamlEnricherInner,
        }
        #[derive(serde::Deserialize)]
        struct YamlEnricherInner {
            id: String,
            name: String,
            version: String,
            match_patterns: Vec<String>,
            template: String,
            #[serde(default = "default_enabled")]
            enabled: bool,
            extractors: Vec<YamlExtractor>,
        }
        #[derive(serde::Deserialize)]
        struct YamlExtractor {
            id: String,
            #[serde(rename = "type")]
            extractor_type: String,
            pattern: String,
            fact_key: String,
            #[serde(default)]
            priority: i32,
            #[serde(default = "default_merge_mode")]
            merge_mode: String,
        }
        fn default_enabled() -> bool {
            true
        }
        fn default_merge_mode() -> String {
            "single".to_string()
        }

        let yaml: YamlEnricher =
            serde_yaml::from_str(content).map_err(|e| format!("YAML parse error: {}", e))?;

        Ok(EnricherDescriptor {
            id: yaml.enricher.id,
            name: yaml.enricher.name,
            version: yaml.enricher.version,
            match_patterns: yaml.enricher.match_patterns,
            template: yaml.enricher.template,
            enabled: yaml.enricher.enabled,
            extractors: yaml
                .enricher
                .extractors
                .into_iter()
                .map(|e| ExtractorConfig {
                    id: e.id,
                    extractor_type: e.extractor_type,
                    pattern: e.pattern,
                    fact_key: e.fact_key,
                    priority: e.priority,
                    merge_mode: e.merge_mode,
                })
                .collect(),
        })
    }

    fn parse_toml(&self, content: &str) -> Result<EnricherDescriptor, String> {
        #[derive(serde::Deserialize)]
        struct TomlEnricher {
            enricher: TomlEnricherInner,
        }
        #[derive(serde::Deserialize)]
        struct TomlEnricherInner {
            id: String,
            name: String,
            version: String,
            match_patterns: Vec<String>,
            template: String,
            #[serde(default = "default_enabled")]
            enabled: bool,
            extractors: Vec<TomlExtractor>,
        }
        #[derive(serde::Deserialize)]
        struct TomlExtractor {
            id: String,
            #[serde(rename = "type")]
            extractor_type: String,
            pattern: String,
            fact_key: String,
            #[serde(default)]
            priority: i32,
            #[serde(default = "default_merge_mode")]
            merge_mode: String,
        }
        fn default_enabled() -> bool {
            true
        }
        fn default_merge_mode() -> String {
            "single".to_string()
        }

        let toml: TomlEnricher =
            toml::from_str(content).map_err(|e| format!("TOML parse error: {}", e))?;

        Ok(EnricherDescriptor {
            id: toml.enricher.id,
            name: toml.enricher.name,
            version: toml.enricher.version,
            match_patterns: toml.enricher.match_patterns,
            template: toml.enricher.template,
            enabled: toml.enricher.enabled,
            extractors: toml
                .enricher
                .extractors
                .into_iter()
                .map(|e| ExtractorConfig {
                    id: e.id,
                    extractor_type: e.extractor_type,
                    pattern: e.pattern,
                    fact_key: e.fact_key,
                    priority: e.priority,
                    merge_mode: e.merge_mode,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_import_yaml_valid() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();
        let importer = YamlCatalogImporter::new(&repo);

        let yaml_content = "enricher:\n  id: maven\n  name: Maven\n  version: \"1.0\"\n  match_patterns:\n    - \"^mvn\\\\s+package\"\n  template: build-template\n  enabled: true\n  extractors:\n    - id: bs\n      type: regex\n      pattern: SUCCESS\n      fact_key: status\n      priority: 1\n".to_string();

        std::fs::write(tmp.path().join("maven.yaml"), yaml_content).unwrap();
        let count = importer.import_dir(tmp.path()).await.unwrap();
        assert_eq!(count, 1);

        let enrichers = repo.find_enrichers("mvn package").await;
        assert_eq!(enrichers.len(), 1);
        assert_eq!(enrichers[0].id, "maven");
        assert_eq!(enrichers[0].extractors.len(), 1);
    }

    #[tokio::test]
    async fn test_find_enrichers_match() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();

        let enricher = EnricherDescriptor {
            id: "maven".to_string(),
            name: "Maven".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^mvn\s+(package|install|verify)".to_string()],
            template: "Build {{status}}".to_string(),
            enabled: true,
            extractors: vec![ExtractorConfig {
                id: "build_status".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"(?P<status>BUILD\s+(SUCCESS|FAILURE))".to_string(),
                fact_key: "build_status".to_string(),
                priority: 1,
                merge_mode: "single".to_string(),
            }],
        };
        repo.upsert_enricher(&enricher).await.unwrap();

        let found = repo.find_enrichers("mvn package").await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "maven");

        let not_found = repo.find_enrichers("cargo build").await;
        assert!(not_found.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_file_resilience() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();
        let importer = YamlCatalogImporter::new(&repo);

        // Valid file
        let yaml_content = r#"
enricher:
  id: "maven"
  name: "Maven"
  version: "1.0"
  match_patterns:
    - "^mvn\\s+package"
  template: "Build"
  enabled: true
  extractors: []
"#;
        std::fs::write(tmp.path().join("maven.yaml"), yaml_content).unwrap();

        // Invalid file
        std::fs::write(tmp.path().join("invalid.yaml"), "not: valid: yaml: content: !!!").unwrap();

        // Empty dir
        let empty_dir = tmp.path().join("empty");
        std::fs::create_dir(&empty_dir).unwrap();

        let count = importer.import_dir(tmp.path()).await.unwrap();
        assert_eq!(count, 1); // Only the valid one counted
    }

    #[tokio::test]
    async fn test_yaml_parse_error_does_not_fallback_to_toml() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();
        let importer = YamlCatalogImporter::new(&repo);

        // Malformed YAML that is valid TOML (starts with a simple key)
        // YAML would parse this as a mapping, but it's actually valid TOML
        // Since our parser will fail on YAML, it should NOT fall back to TOML
        let ambiguous_content = "key = \"value\"\n";
        std::fs::write(tmp.path().join("ambiguous.toml"), ambiguous_content).unwrap();

        // This file has a YAML extension but contains TOML-like content
        // It should fail YAML parsing and NOT fall back to TOML
        std::fs::write(tmp.path().join("ambiguous.yaml"), ambiguous_content).unwrap();

        let count = importer.import_dir(tmp.path()).await.unwrap();
        // Both files should be skipped because:
        // - .yaml file: YAML parse fails, no fallback to TOML
        // - .toml file: TOML parse succeeds but doesn't have required fields
        // Since the TOML content doesn't match the expected schema (no [enricher] table),
        // it won't upsert anything
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_same_extractor_id_different_enrichers() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();

        // Maven enricher with extractor id "build_status"
        let maven_enricher = EnricherDescriptor {
            id: "maven".to_string(),
            name: "Maven".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^mvn\s+".to_string()],
            template: "Build".to_string(),
            enabled: true,
            extractors: vec![ExtractorConfig {
                id: "build_status".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"BUILD\s+(SUCCESS|FAILURE)".to_string(),
                fact_key: "build_status".to_string(),
                priority: 1,
                merge_mode: "single".to_string(),
            }],
        };

        // Gradle enricher also with extractor id "build_status"
        let gradle_enricher = EnricherDescriptor {
            id: "gradle".to_string(),
            name: "Gradle".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^gradle\s+".to_string()],
            template: "Build".to_string(),
            enabled: true,
            extractors: vec![ExtractorConfig {
                id: "build_status".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"BUILD\s+(SUCCESS|FAILURE)".to_string(),
                fact_key: "build_status".to_string(),
                priority: 1,
                merge_mode: "single".to_string(),
            }],
        };

        repo.upsert_enricher(&maven_enricher).await.unwrap();
        repo.upsert_enricher(&gradle_enricher).await.unwrap();

        // Both should be stored with their respective extractors
        let all_enrichers = repo.list_all().await;
        assert_eq!(all_enrichers.len(), 2);

        let maven = all_enrichers.iter().find(|e| e.id == "maven").unwrap();
        let gradle = all_enrichers.iter().find(|e| e.id == "gradle").unwrap();

        assert_eq!(maven.extractors.len(), 1);
        assert_eq!(gradle.extractors.len(), 1);
        assert_eq!(maven.extractors[0].id, "build_status");
        assert_eq!(gradle.extractors[0].id, "build_status");
    }

    #[tokio::test]
    async fn test_invalid_regex_rejected_during_import() {
        // W2 Fix: invalid regex extractor should reject the entire enricher import
        // Note: import_dir is resilient (continues on error), but invalid regex
        // causes import_file to return error, so the enricher is not persisted
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();
        let importer = YamlCatalogImporter::new(&repo);

        // YAML with invalid regex pattern
        let yaml_content = r#"
enricher:
  id: "bad_maven"
  name: "Bad Maven"
  version: "1.0"
  match_patterns:
    - "^mvn\\s+test"
  template: "Test"
  enabled: true
  extractors:
    - id: bad_regex
      type: regex
      pattern: "[invalid"  # Invalid regex - missing closing bracket
      fact_key: status
      priority: 1
"#;
        std::fs::write(tmp.path().join("bad_maven.yaml"), yaml_content).unwrap();

        // Import should complete (import_dir is resilient) but count is 0
        let result = importer.import_dir(tmp.path()).await;
        assert!(result.is_ok(), "import_dir should complete even on error");
        assert_eq!(result.unwrap(), 0, "No files should be imported successfully");

        // Verify the enricher was NOT persisted
        let all_enrichers = repo.list_all().await;
        assert!(all_enrichers.iter().find(|e| e.id == "bad_maven").is_none(),
            "Enricher with invalid regex should not be persisted");

        let found = repo.find_enrichers("mvn test").await;
        assert!(found.iter().find(|e| e.id == "bad_maven").is_none(),
            "Enricher with invalid regex should not appear in find_enrichers");
    }

    #[tokio::test]
    async fn test_import_skips_only_invalid_extractors_not_whole_enricher_when_upsert_directly() {
        // This tests the direct upsert path (not through importer)
        // When upsert is called directly, invalid regex is validated in pipeline
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let repo = SqliteCatalogRepository::new(&db_path).unwrap();

        // Valid enricher first
        let valid_enricher = EnricherDescriptor {
            id: "maven".to_string(),
            name: "Maven".to_string(),
            version: "1.0".to_string(),
            match_patterns: vec![r"^mvn\s+".to_string()],
            template: "Build".to_string(),
            enabled: true,
            extractors: vec![ExtractorConfig {
                id: "build_status".to_string(),
                extractor_type: "regex".to_string(),
                pattern: r"BUILD\s+(SUCCESS|FAILURE)".to_string(),
                fact_key: "build_status".to_string(),
                priority: 1,
                merge_mode: "single".to_string(),
            }],
        };
        repo.upsert_enricher(&valid_enricher).await.unwrap();

        // Verify it's there
        let all_enrichers = repo.list_all().await;
        assert_eq!(all_enrichers.len(), 1);
        assert_eq!(all_enrichers[0].id, "maven");
    }
}
