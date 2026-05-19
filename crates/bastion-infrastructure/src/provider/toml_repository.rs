//! TOML file-based ProviderInstance repository.
//!
//! Persists ProviderInstance entities to TOML files in a configured directory.
//! Each instance is stored as `{name}.toml`.

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

use bastion_domain::provider::instance::{ProviderInstance, ProviderInstanceId};
use bastion_domain::provider::instance_repository::{Error, ProviderInstanceRepository, Result};
use bastion_domain::provider::ProviderTypeId;

/// Repository that persists ProviderInstance entities to TOML files.
///
/// Each instance is stored as a single TOML file named `{instance_name}.toml`
/// in the configured `base_path` directory.
#[derive(Debug)]
pub struct TomlProviderInstanceRepository {
    base_path: PathBuf,
}

impl TomlProviderInstanceRepository {
    /// Create a new TomlProviderInstanceRepository with the given base directory.
    ///
    /// The base directory will be created if it doesn't exist.
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Returns the path for a TOML file with the given instance name.
    fn instance_path(&self, name: &str) -> PathBuf {
        self.base_path.join(format!("{}.toml", name))
    }
}

#[async_trait]
impl ProviderInstanceRepository for TomlProviderInstanceRepository {
    /// Save an instance to its TOML file.
    ///
    /// Writes the instance as formatted TOML to `{base_path}/{name}.toml`.
    async fn save(&self, instance: &ProviderInstance) -> Result<()> {
        let path = self.instance_path(instance.name());
        let toml_content = toml::to_string_pretty(instance).map_err(Error::TomlSerialize)?;
        fs::write(&path, toml_content).await.map_err(Error::Io)?;
        Ok(())
    }

    /// Find an instance by ID by listing all and filtering.
    ///
    /// Note: This is O(n) where n is the number of instances.
    /// For frequent ID-based lookups, consider a different repository implementation.
    async fn find_by_id(&self, id: &ProviderInstanceId) -> Result<Option<ProviderInstance>> {
        let all = self.list_all().await?;
        Ok(all.into_iter().find(|i| i.id() == id))
    }

    /// Find an instance by name by reading its TOML file directly.
    async fn find_by_name(&self, name: &str) -> Result<Option<ProviderInstance>> {
        let path = self.instance_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path).await.map_err(Error::Io)?;
        let instance: ProviderInstance = toml::from_str(&content).map_err(Error::TomlParse)?;
        Ok(Some(instance))
    }

    /// List all instances of a specific provider type.
    async fn list_by_type(&self, type_id: &ProviderTypeId) -> Result<Vec<ProviderInstance>> {
        let all = self.list_all().await?;
        Ok(all.into_iter().filter(|i| i.type_id() == type_id).collect())
    }

    /// List all instances by reading all TOML files in the base directory.
    async fn list_all(&self) -> Result<Vec<ProviderInstance>> {
        let mut entries = fs::read_dir(&self.base_path).await.map_err(Error::Io)?;
        let mut instances = Vec::new();

        while let Some(entry) = entries.next_entry().await.map_err(Error::Io)? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                match fs::read_to_string(&path).await {
                    Ok(content) => match toml::from_str::<ProviderInstance>(&content) {
                        Ok(instance) => instances.push(instance),
                        // Skip files that aren't valid ProviderInstance TOML
                        Err(_) => {}
                    },
                    // Skip files that can't be read
                    Err(_) => {}
                }
            }
        }

        Ok(instances)
    }

    /// Delete an instance by ID.
    ///
    /// Finds the instance first, then removes its TOML file.
    /// Returns Ok(()) even if the instance doesn't exist.
    async fn delete(&self, id: &ProviderInstanceId) -> Result<()> {
        let all = self.list_all().await?;
        if let Some(instance) = all.into_iter().find(|i| i.id() == id) {
            let path = self.instance_path(instance.name());
            fs::remove_file(path).await.map_err(Error::Io)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bastion_domain::provider::instance_config::ProviderInstanceConfig;
    use bastion_domain::provider::instance_constraints::InstanceConstraints;
    use bastion_domain::provider::ProviderTypeId;
    use tempfile::TempDir;

    fn create_test_instance(name: &str, type_id: &str) -> ProviderInstance {
        let config = ProviderInstanceConfig::podman();
        ProviderInstance::new(
            ProviderInstanceId::new(),
            ProviderTypeId::new(type_id),
            name.to_string(),
            format!("Test {}", name),
            Some("A test instance".to_string()),
            config,
            InstanceConstraints::default(),
        )
        .expect("test instance should be valid")
    }

    #[tokio::test]
    async fn test_save_and_find_by_name() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let instance = create_test_instance("test-podman", "podman");
        repo.save(&instance).await.unwrap();

        let found = repo.find_by_name("test-podman").await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.name(), "test-podman");
        assert_eq!(found.id(), instance.id());
    }

    #[tokio::test]
    async fn test_find_by_name_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let found = repo.find_by_name("nonexistent").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_list_all() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let podman = create_test_instance("podman-1", "podman");
        let docker = create_test_instance("docker-1", "docker");

        repo.save(&podman).await.unwrap();
        repo.save(&docker).await.unwrap();

        let all = repo.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_all_empty() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let all = repo.list_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_list_by_type() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let podman1 = create_test_instance("podman-1", "podman");
        let podman2 = create_test_instance("podman-2", "podman");
        let docker = create_test_instance("docker-1", "docker");

        repo.save(&podman1).await.unwrap();
        repo.save(&podman2).await.unwrap();
        repo.save(&docker).await.unwrap();

        let podman_instances = repo
            .list_by_type(&ProviderTypeId::new("podman"))
            .await
            .unwrap();
        assert_eq!(podman_instances.len(), 2);

        let docker_instances = repo
            .list_by_type(&ProviderTypeId::new("docker"))
            .await
            .unwrap();
        assert_eq!(docker_instances.len(), 1);

        let wasm_instances = repo
            .list_by_type(&ProviderTypeId::new("wasm"))
            .await
            .unwrap();
        assert!(wasm_instances.is_empty());
    }

    #[tokio::test]
    async fn test_find_by_id() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let instance = create_test_instance("test-podman", "podman");
        let id = instance.id().clone();

        repo.save(&instance).await.unwrap();

        let found = repo.find_by_id(&id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "test-podman");
    }

    #[tokio::test]
    async fn test_find_by_id_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let found = repo.find_by_id(&ProviderInstanceId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let instance = create_test_instance("test-podman", "podman");
        let id = instance.id().clone();

        repo.save(&instance).await.unwrap();
        assert!(repo.find_by_id(&id).await.unwrap().is_some());

        repo.delete(&id).await.unwrap();
        assert!(repo.find_by_id(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        // Should not error when deleting non-existent
        repo.delete(&ProviderInstanceId::new()).await.unwrap();
    }

    #[tokio::test]
    async fn test_save_updates_existing() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let mut instance = create_test_instance("test-podman", "podman");
        repo.save(&instance).await.unwrap();

        instance.mark_active();
        repo.save(&instance).await.unwrap();

        let found = repo.find_by_name("test-podman").await.unwrap().unwrap();
        assert!(matches!(found.status(), bastion_domain::provider::instance::ProviderInstanceStatus::Active));
    }

    #[tokio::test]
    async fn test_invalid_toml_in_directory() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        // Write an invalid TOML file directly
        let invalid_path = temp_dir.path().join("invalid.toml");
        fs::write(&invalid_path, "not valid toml {{{{}}").await.unwrap();

        // Should not error, just skip the invalid file
        let all = repo.list_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_roundtrip_serialization() {
        let temp_dir = TempDir::new().unwrap();
        let repo = TomlProviderInstanceRepository::new(temp_dir.path().to_path_buf());

        let original = create_test_instance("roundtrip-test", "podman");
        repo.save(&original).await.unwrap();

        let loaded = repo.find_by_name("roundtrip-test").await.unwrap().unwrap();

        // Verify key fields are preserved
        assert_eq!(loaded.id(), original.id());
        assert_eq!(loaded.name(), original.name());
        assert_eq!(loaded.display_name(), original.display_name());
        assert_eq!(loaded.type_id(), original.type_id());
    }
}