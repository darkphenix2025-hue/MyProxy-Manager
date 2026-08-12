use crate::models::connection::SecretRef;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretStoreError {
    #[error("secret reference belongs to a different backend")]
    BackendMismatch,
    #[error("secret was not found")]
    NotFound,
    #[error("secret store is unavailable")]
    Unavailable,
    #[error("secret store rejected an invalid namespace")]
    InvalidNamespace,
    #[error("secret is too large for the configured secret store")]
    SecretTooLarge,
    #[error("secret store operation requires recovery cleanup")]
    RecoveryRequired(SecretRef),
}

#[async_trait::async_trait]
pub trait SecretStore: Clone + Send + Sync + 'static {
    async fn create(&self, namespace: &str, secret: &str) -> Result<SecretRef, SecretStoreError>;
    async fn read(&self, secret_ref: &SecretRef) -> Result<String, SecretStoreError>;
    async fn replace(&self, secret_ref: &SecretRef, secret: &str) -> Result<(), SecretStoreError>;
    async fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError>;
}

#[derive(Debug, Clone)]
pub struct KeyringSecretStore {
    service: String,
}

const KEYRING_CHUNK_BYTES: usize = 768;
const MAX_KEYRING_CHUNKS: usize = 128;

#[derive(serde::Serialize, serde::Deserialize)]
struct KeyringManifest {
    version: u8,
    active: KeyringGeneration,
    #[serde(default)]
    retired: Vec<KeyringGeneration>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct KeyringGeneration {
    id: String,
    chunk_count: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KeyringPendingOperation {
    key: String,
    generation: KeyringGeneration,
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new("com.myproxy-manager.credentials")
    }
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn validate_ref(secret_ref: &SecretRef) -> Result<(), SecretStoreError> {
        if secret_ref.backend != "keyring" {
            return Err(SecretStoreError::BackendMismatch);
        }
        if secret_ref.key.trim().is_empty() {
            return Err(SecretStoreError::NotFound);
        }
        Ok(())
    }

    fn validate_namespace(namespace: &str) -> Result<&str, SecretStoreError> {
        let namespace = namespace.trim();
        if namespace.is_empty()
            || !namespace.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(SecretStoreError::InvalidNamespace);
        }
        Ok(namespace)
    }

    fn split_secret(secret: &str) -> Result<Vec<&str>, SecretStoreError> {
        if secret.is_empty() {
            return Ok(vec![""]);
        }
        let mut chunks = Vec::new();
        let mut start = 0;
        while start < secret.len() {
            let mut end = (start + KEYRING_CHUNK_BYTES).min(secret.len());
            while end > start && !secret.is_char_boundary(end) {
                end -= 1;
            }
            if end == start {
                return Err(SecretStoreError::SecretTooLarge);
            }
            chunks.push(&secret[start..end]);
            start = end;
        }
        if chunks.len() > MAX_KEYRING_CHUNKS {
            return Err(SecretStoreError::SecretTooLarge);
        }
        Ok(chunks)
    }

    fn entry(service: &str, key: &str) -> Result<keyring::Entry, SecretStoreError> {
        keyring::Entry::new(service, key).map_err(|_| SecretStoreError::Unavailable)
    }

    fn map_keyring_error(error: keyring::Error) -> SecretStoreError {
        match error {
            keyring::Error::NoEntry => SecretStoreError::NotFound,
            _ => SecretStoreError::Unavailable,
        }
    }

    fn read_manifest(service: &str, key: &str) -> Result<KeyringManifest, SecretStoreError> {
        let encoded = Self::entry(service, key)?
            .get_password()
            .map_err(Self::map_keyring_error)?;
        let manifest: KeyringManifest =
            serde_json::from_str(&encoded).map_err(|_| SecretStoreError::Unavailable)?;
        if manifest.version != 1
            || !Self::valid_generation(&manifest.active)
            || manifest
                .retired
                .iter()
                .any(|item| !Self::valid_generation(item))
        {
            return Err(SecretStoreError::Unavailable);
        }
        Ok(manifest)
    }

    fn valid_generation(generation: &KeyringGeneration) -> bool {
        generation.chunk_count > 0
            && generation.chunk_count <= MAX_KEYRING_CHUNKS
            && uuid::Uuid::parse_str(&generation.id).is_ok()
    }

    fn chunk_key(key: &str, generation: &str, index: usize) -> String {
        format!("{key}:{generation}:{index}")
    }

    fn pending_key() -> &'static str {
        "__myproxy_pending_operation_v1"
    }

    fn read_pending(service: &str) -> Result<Option<KeyringPendingOperation>, SecretStoreError> {
        match Self::entry(service, Self::pending_key())?.get_password() {
            Ok(encoded) => {
                let pending: KeyringPendingOperation =
                    serde_json::from_str(&encoded).map_err(|_| SecretStoreError::Unavailable)?;
                if pending.key.trim().is_empty() || !Self::valid_generation(&pending.generation) {
                    return Err(SecretStoreError::Unavailable);
                }
                Ok(Some(pending))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Self::map_keyring_error(error)),
        }
    }

    fn delete_pending(service: &str) -> Result<(), SecretStoreError> {
        match Self::entry(service, Self::pending_key())?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Self::map_keyring_error(error)),
        }
    }

    fn recover_pending(service: &str) -> Result<(), SecretStoreError> {
        if let Some(pending) = Self::read_pending(service)? {
            match Self::read_manifest(service, &pending.key) {
                Ok(manifest) if manifest.active.id == pending.generation.id => {
                    for retired in &manifest.retired {
                        Self::delete_generation(service, &pending.key, retired)?;
                    }
                    Self::write_manifest(
                        service,
                        &pending.key,
                        &KeyringManifest {
                            version: 1,
                            active: manifest.active,
                            retired: Vec::new(),
                        },
                    )?;
                }
                Ok(_) | Err(SecretStoreError::NotFound) => {
                    Self::delete_generation(service, &pending.key, &pending.generation)?;
                }
                Err(_) => {
                    return Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                        "keyring",
                        pending.key,
                    )));
                }
            }
            Self::delete_pending(service)?;
        }
        Ok(())
    }

    fn delete_generation(
        service: &str,
        key: &str,
        generation: &KeyringGeneration,
    ) -> Result<(), SecretStoreError> {
        let mut failed = false;
        for index in 0..generation.chunk_count {
            match Self::entry(service, &Self::chunk_key(key, &generation.id, index)) {
                Ok(entry) => match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(_) => failed = true,
                },
                Err(_) => failed = true,
            }
        }
        if failed {
            Err(SecretStoreError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn write_secret(
        service: &str,
        key: &str,
        secret: &str,
    ) -> Result<KeyringGeneration, SecretStoreError> {
        if Self::read_pending(service)?.is_some() {
            return Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                "keyring", key,
            )));
        }
        let chunks = Self::split_secret(secret)?;
        let generation = KeyringGeneration {
            id: uuid::Uuid::new_v4().to_string(),
            chunk_count: chunks.len(),
        };
        let pending = serde_json::to_string(&KeyringPendingOperation {
            key: key.to_string(),
            generation: generation.clone(),
        })
        .map_err(|_| SecretStoreError::Unavailable)?;
        Self::entry(service, Self::pending_key())?
            .set_password(&pending)
            .map_err(Self::map_keyring_error)?;
        for (index, chunk) in chunks.iter().enumerate() {
            let result = Self::entry(service, &Self::chunk_key(key, &generation.id, index))?
                .set_password(chunk)
                .map_err(Self::map_keyring_error);
            if let Err(error) = result {
                return match Self::delete_generation(service, key, &generation) {
                    Ok(()) => {
                        let _ = Self::delete_pending(service);
                        Err(error)
                    }
                    Err(_) => Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                        "keyring", key,
                    ))),
                };
            }
        }
        Ok(generation)
    }

    fn write_manifest(
        service: &str,
        key: &str,
        manifest: &KeyringManifest,
    ) -> Result<(), SecretStoreError> {
        let encoded =
            serde_json::to_string(&manifest).map_err(|_| SecretStoreError::Unavailable)?;
        Self::entry(service, key)?
            .set_password(&encoded)
            .map_err(Self::map_keyring_error)
    }

    fn service_lock(&self) -> std::sync::Arc<tokio::sync::RwLock<()>> {
        let digest = {
            use sha2::Digest;
            let mut digest = sha2::Sha256::new();
            digest.update(self.service.as_bytes());
            base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                digest.finalize(),
            )
        };
        keyring_locks()
            .entry(digest)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::RwLock::new(())))
            .clone()
    }
}

fn keyring_locks() -> &'static dashmap::DashMap<String, std::sync::Arc<tokio::sync::RwLock<()>>> {
    static LOCKS: std::sync::OnceLock<
        dashmap::DashMap<String, std::sync::Arc<tokio::sync::RwLock<()>>>,
    > = std::sync::OnceLock::new();
    LOCKS.get_or_init(dashmap::DashMap::new)
}

#[async_trait::async_trait]
impl SecretStore for KeyringSecretStore {
    async fn create(&self, namespace: &str, secret: &str) -> Result<SecretRef, SecretStoreError> {
        let namespace = Self::validate_namespace(namespace)?.to_string();
        let key = format!("{namespace}:{}", uuid::Uuid::new_v4());
        let service = self.service.clone();
        let entry_key = key.clone();
        let secret = zeroize::Zeroizing::new(secret.to_string());
        let lock = self.service_lock();
        let _guard = lock.write().await;
        tokio::task::spawn_blocking(move || {
            Self::recover_pending(&service)?;
            let active = Self::write_secret(&service, &entry_key, secret.as_str())?;
            let manifest = KeyringManifest {
                version: 1,
                active: active.clone(),
                retired: Vec::new(),
            };
            if let Err(error) = Self::write_manifest(&service, &entry_key, &manifest) {
                return match Self::delete_generation(&service, &entry_key, &active) {
                    Ok(()) => {
                        let _ = Self::delete_pending(&service);
                        Err(error)
                    }
                    Err(_) => Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                        "keyring", entry_key,
                    ))),
                };
            }
            Self::delete_pending(&service).map_err(|_| {
                SecretStoreError::RecoveryRequired(SecretRef::new("keyring", entry_key))
            })
        })
        .await
        .map_err(|_| SecretStoreError::Unavailable)??;
        Ok(SecretRef::new("keyring", key))
    }

    async fn read(&self, secret_ref: &SecretRef) -> Result<String, SecretStoreError> {
        Self::validate_ref(secret_ref)?;
        let service = self.service.clone();
        let key = secret_ref.key.clone();
        let lock = self.service_lock();
        let _guard = lock.write().await;
        tokio::task::spawn_blocking(move || {
            Self::recover_pending(&service)?;
            let manifest = Self::read_manifest(&service, &key)?;
            let mut secret = String::new();
            for index in 0..manifest.active.chunk_count {
                let chunk =
                    Self::entry(&service, &Self::chunk_key(&key, &manifest.active.id, index))?
                        .get_password()
                        .map_err(Self::map_keyring_error)?;
                secret.push_str(&chunk);
            }
            Ok(secret)
        })
        .await
        .map_err(|_| SecretStoreError::Unavailable)?
    }

    async fn replace(&self, secret_ref: &SecretRef, secret: &str) -> Result<(), SecretStoreError> {
        Self::validate_ref(secret_ref)?;
        let service = self.service.clone();
        let key = secret_ref.key.clone();
        let secret = zeroize::Zeroizing::new(secret.to_string());
        let lock = self.service_lock();
        let _guard = lock.write().await;
        tokio::task::spawn_blocking(move || {
            Self::recover_pending(&service)?;
            let previous = Self::read_manifest(&service, &key)?;
            let active = Self::write_secret(&service, &key, secret.as_str())?;
            let mut retired = previous.retired.clone();
            retired.push(previous.active.clone());
            let transitional = KeyringManifest {
                version: 1,
                active: active.clone(),
                retired,
            };
            if let Err(error) = Self::write_manifest(&service, &key, &transitional) {
                return match Self::delete_generation(&service, &key, &active) {
                    Ok(()) => {
                        let _ = Self::delete_pending(&service);
                        Err(error)
                    }
                    Err(_) => Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                        "keyring", key,
                    ))),
                };
            }
            for generation in &transitional.retired {
                if Self::delete_generation(&service, &key, generation).is_err() {
                    return Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                        "keyring", key,
                    )));
                }
            }
            if Self::write_manifest(
                &service,
                &key,
                &KeyringManifest {
                    version: 1,
                    active,
                    retired: Vec::new(),
                },
            )
            .is_err()
            {
                return Err(SecretStoreError::RecoveryRequired(SecretRef::new(
                    "keyring", key,
                )));
            }
            Self::delete_pending(&service)
                .map_err(|_| SecretStoreError::RecoveryRequired(SecretRef::new("keyring", key)))
        })
        .await
        .map_err(|_| SecretStoreError::Unavailable)?
    }

    async fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError> {
        Self::validate_ref(secret_ref)?;
        let service = self.service.clone();
        let key = secret_ref.key.clone();
        let lock = self.service_lock();
        let _guard = lock.write().await;
        tokio::task::spawn_blocking(move || {
            let manifest = match Self::read_manifest(&service, &key) {
                Ok(manifest) => Some(manifest),
                Err(SecretStoreError::NotFound) => None,
                Err(error) => return Err(error),
            };
            if let Some(manifest) = manifest.as_ref() {
                Self::delete_generation(&service, &key, &manifest.active)?;
                for generation in &manifest.retired {
                    Self::delete_generation(&service, &key, generation)?;
                }
            }
            Self::recover_pending(&service)?;
            match Self::entry(&service, &key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(Self::map_keyring_error(error)),
            }
        })
        .await
        .map_err(|_| SecretStoreError::Unavailable)?
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct MemorySecretStore {
    values: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl SecretStore for MemorySecretStore {
    async fn create(&self, namespace: &str, secret: &str) -> Result<SecretRef, SecretStoreError> {
        let namespace = KeyringSecretStore::validate_namespace(namespace)?;
        let key = format!("{namespace}:{}", uuid::Uuid::new_v4());
        self.values.lock().insert(key.clone(), secret.to_string());
        Ok(SecretRef::new("memory", key))
    }

    async fn read(&self, secret_ref: &SecretRef) -> Result<String, SecretStoreError> {
        if secret_ref.backend != "memory" {
            return Err(SecretStoreError::BackendMismatch);
        }
        self.values
            .lock()
            .get(&secret_ref.key)
            .cloned()
            .ok_or(SecretStoreError::NotFound)
    }

    async fn replace(&self, secret_ref: &SecretRef, secret: &str) -> Result<(), SecretStoreError> {
        if secret_ref.backend != "memory" {
            return Err(SecretStoreError::BackendMismatch);
        }
        let mut values = self.values.lock();
        let value = values
            .get_mut(&secret_ref.key)
            .ok_or(SecretStoreError::NotFound)?;
        *value = secret.to_string();
        Ok(())
    }

    async fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError> {
        if secret_ref.backend != "memory" {
            return Err(SecretStoreError::BackendMismatch);
        }
        self.values.lock().remove(&secret_ref.key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_roundtrips_and_replaces_secret_without_exposing_it_in_ref() {
        let store = MemorySecretStore::default();
        let secret_ref = store
            .create("codex", "access-canary")
            .await
            .expect("create secret");

        assert_eq!(secret_ref.backend, "memory");
        assert!(!secret_ref.key.contains("access-canary"));
        assert_eq!(store.read(&secret_ref).await.unwrap(), "access-canary");

        store
            .replace(&secret_ref, "refreshed-canary")
            .await
            .unwrap();
        assert_eq!(store.read(&secret_ref).await.unwrap(), "refreshed-canary");
    }

    #[tokio::test]
    async fn store_rejects_foreign_backend_and_delete_is_idempotent() {
        let store = MemorySecretStore::default();
        let foreign = crate::models::connection::SecretRef::new("keyring", "foreign");
        assert!(matches!(
            store.read(&foreign).await,
            Err(SecretStoreError::BackendMismatch)
        ));

        let secret_ref = store.create("codex", "secret").await.unwrap();
        store.delete(&secret_ref).await.unwrap();
        store.delete(&secret_ref).await.unwrap();
        assert!(matches!(
            store.read(&secret_ref).await,
            Err(SecretStoreError::NotFound)
        ));
    }

    #[test]
    fn keyring_chunking_supports_large_utf8_token_sets_with_bounded_entries() {
        let secret = format!("{}{}", "token-".repeat(2_000), "令牌".repeat(500));
        let chunks = KeyringSecretStore::split_secret(&secret).unwrap();

        assert!(chunks.len() > 1);
        assert!(chunks.len() <= MAX_KEYRING_CHUNKS);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.len() <= KEYRING_CHUNK_BYTES));
        assert_eq!(chunks.concat(), secret);
    }

    #[test]
    fn keyring_chunking_rejects_secrets_beyond_manifest_limit() {
        let secret = "x".repeat(KEYRING_CHUNK_BYTES * MAX_KEYRING_CHUNKS + 1);
        assert!(matches!(
            KeyringSecretStore::split_secret(&secret),
            Err(SecretStoreError::SecretTooLarge)
        ));
    }
}
