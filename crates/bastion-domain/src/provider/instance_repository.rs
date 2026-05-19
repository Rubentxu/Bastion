//! Provider instance repository trait.
//!
//! This trait defines the interface for persisting and querying ProviderInstance entities.
//! Implementations are provided by bastion-infrastructure.

use async_trait::async_trait;
use super::{ProviderInstance, ProviderInstanceId, ProviderTypeId};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Instance not found: {0}")]
    NotFound(ProviderInstanceId),
    #[error("Instance with name already exists: {0}")]
    AlreadyExists(String),
    #[error("Unknown provider type: {0}")]
    UnknownProviderType(String),
    #[error("Configuration validation failed: {0}")]
    ValidationFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Instance not found by name: {0}")]
    InstanceNotFoundByName(String),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("TOML parsing error: {0}")]
    TomlParse(#[from] toml::de::Error),
}

/// Repository trait for ProviderInstance entities.
///
/// Implementations are responsible for persisting instances to storage
/// (TOML files, database, etc.). This trait is implemented by
/// bastion-infrastructure, not bastion-domain.
#[async_trait]
pub trait ProviderInstanceRepository: Send + Sync {
    /// Save an instance (create or update).
    async fn save(&self, instance: &ProviderInstance) -> Result<()>;

    /// Find an instance by its ID.
    async fn find_by_id(&self, id: &ProviderInstanceId) -> Result<Option<ProviderInstance>>;

    /// Find an instance by its name.
    async fn find_by_name(&self, name: &str) -> Result<Option<ProviderInstance>>;

    /// List all instances of a specific provider type.
    async fn list_by_type(&self, type_id: &ProviderTypeId) -> Result<Vec<ProviderInstance>>;

    /// List all instances.
    async fn list_all(&self) -> Result<Vec<ProviderInstance>>;

    /// Delete an instance by ID.
    async fn delete(&self, id: &ProviderInstanceId) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use super::*;

    /// A mock in-memory implementation for testing.
    pub struct MockRepository {
        instances: Mutex<HashMap<ProviderInstanceId, ProviderInstance>>,
        by_name: Mutex<HashMap<String, ProviderInstanceId>>,
    }

    impl MockRepository {
        pub fn new() -> Self {
            Self {
                instances: Mutex::new(HashMap::new()),
                by_name: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl ProviderInstanceRepository for MockRepository {
        async fn save(&self, instance: &ProviderInstance) -> Result<()> {
            let mut by_name = self.by_name.lock().unwrap();
            let mut instances = self.instances.lock().unwrap();

            if let Some(existing) = by_name.get(instance.name()) {
                if existing != instance.id() {
                    return Err(Error::AlreadyExists(instance.name().to_string()));
                }
            }
            by_name.insert(instance.name().to_string(), instance.id().clone());
            instances.insert(instance.id().clone(), instance.clone());
            Ok(())
        }

        async fn find_by_id(&self, id: &ProviderInstanceId) -> Result<Option<ProviderInstance>> {
            let instances = self.instances.lock().unwrap();
            Ok(instances.get(id).cloned())
        }

        async fn find_by_name(&self, name: &str) -> Result<Option<ProviderInstance>> {
            let by_name = self.by_name.lock().unwrap();
            let instances = self.instances.lock().unwrap();
            Ok(by_name.get(name).and_then(|id| instances.get(id).cloned()))
        }

        async fn list_by_type(&self, type_id: &ProviderTypeId) -> Result<Vec<ProviderInstance>> {
            let instances = self.instances.lock().unwrap();
            Ok(instances
                .values()
                .filter(|i| i.type_id() == type_id)
                .cloned()
                .collect())
        }

        async fn list_all(&self) -> Result<Vec<ProviderInstance>> {
            let instances = self.instances.lock().unwrap();
            Ok(instances.values().cloned().collect())
        }

        async fn delete(&self, id: &ProviderInstanceId) -> Result<()> {
            let mut by_name = self.by_name.lock().unwrap();
            let mut instances = self.instances.lock().unwrap();

            if let Some(instance) = instances.remove(id) {
                by_name.remove(instance.name());
            }
            Ok(())
        }
    }

    use crate::provider::instance_config::ProviderInstanceConfig;
    use crate::provider::instance_constraints::InstanceConstraints;

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
    async fn test_save_and_find_by_id() {
        let repo = MockRepository::new();
        let instance = create_test_instance("test-podman", "podman");

        repo.save(&instance).await.unwrap();
        let found = repo.find_by_id(instance.id()).await.unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "test-podman");
    }

    #[tokio::test]
    async fn test_find_by_id_not_found() {
        let repo = MockRepository::new();
        let found = repo.find_by_id(&ProviderInstanceId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_save_and_find_by_name() {
        let repo = MockRepository::new();
        let instance = create_test_instance("test-podman", "podman");

        repo.save(&instance).await.unwrap();
        let found = repo.find_by_name("test-podman").await.unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().id(), instance.id());
    }

    #[tokio::test]
    async fn test_find_by_name_not_found() {
        let repo = MockRepository::new();
        let found = repo.find_by_name("nonexistent").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_save_already_exists_error() {
        let repo = MockRepository::new();
        let instance1 = create_test_instance("test-podman", "podman");
        let instance2_dup_name = create_test_instance("test-podman", "docker");

        repo.save(&instance1).await.unwrap();
        let result = repo.save(&instance2_dup_name).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn test_list_by_type() {
        let repo = MockRepository::new();

        let podman1 = create_test_instance("podman-1", "podman");
        let podman2 = create_test_instance("podman-2", "podman");
        let docker = create_test_instance("docker-1", "docker");

        repo.save(&podman1).await.unwrap();
        repo.save(&podman2).await.unwrap();
        repo.save(&docker).await.unwrap();

        let podman_instances = repo.list_by_type(&ProviderTypeId::new("podman")).await.unwrap();
        assert_eq!(podman_instances.len(), 2);

        let docker_instances = repo.list_by_type(&ProviderTypeId::new("docker")).await.unwrap();
        assert_eq!(docker_instances.len(), 1);

        let wasm_instances = repo.list_by_type(&ProviderTypeId::new("wasm")).await.unwrap();
        assert_eq!(wasm_instances.len(), 0);
    }

    #[tokio::test]
    async fn test_list_all() {
        let repo = MockRepository::new();

        let podman = create_test_instance("podman-1", "podman");
        let docker = create_test_instance("docker-1", "docker");

        repo.save(&podman).await.unwrap();
        repo.save(&docker).await.unwrap();

        let all = repo.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_delete() {
        let repo = MockRepository::new();
        let instance = create_test_instance("test-podman", "podman");
        let id = instance.id().clone();

        repo.save(&instance).await.unwrap();
        assert!(repo.find_by_id(&id).await.unwrap().is_some());

        repo.delete(&id).await.unwrap();
        assert!(repo.find_by_id(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let repo = MockRepository::new();
        // Should not error when deleting non-existent
        repo.delete(&ProviderInstanceId::new()).await.unwrap();
    }

    #[tokio::test]
    async fn test_error_display() {
        let err = Error::NotFound(ProviderInstanceId::new());
        assert!(format!("{}", err).contains("Instance not found"));

        let err = Error::AlreadyExists("test".to_string());
        assert!(format!("{}", err).contains("already exists"));

        let err = Error::UnknownProviderType("wasm".to_string());
        assert!(format!("{}", err).contains("Unknown provider type"));

        let err = Error::ValidationFailed("invalid config".to_string());
        assert!(format!("{}", err).contains("Configuration validation failed"));
    }
}
