use crate::models::connection::SecretRef;
use crate::modules::secret_store::{KeyringSecretStore, SecretStore, SecretStoreError};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const AUTH_FILE_BACKEND: &str = "auth_file";
const MAX_AUTH_FILE_BYTES: usize = 256 * 1024;

/// File-backed secret storage used by provider auth-files.
///
/// The file contents are intentionally opaque to this module. Provider adapters
/// own the JSON schema; this store only enforces safe names, permissions, size
/// limits, and atomic writes.
#[derive(Debug, Clone)]
pub struct AuthFileStore {
    root: PathBuf,
}

impl AuthFileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for_ref(&self, secret_ref: &SecretRef) -> Result<PathBuf, SecretStoreError> {
        let (namespace, key) = self.parse_ref(secret_ref)?;
        Ok(self.root.join(namespace).join(format!("{key}.json")))
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

    fn parse_ref<'a>(
        &self,
        secret_ref: &'a SecretRef,
    ) -> Result<(&'a str, &'a str), SecretStoreError> {
        if secret_ref.backend != AUTH_FILE_BACKEND {
            return Err(SecretStoreError::BackendMismatch);
        }
        let (namespace, key) = secret_ref
            .key
            .split_once(':')
            .ok_or(SecretStoreError::NotFound)?;
        let namespace = Self::validate_namespace(namespace)?;
        if uuid::Uuid::parse_str(key).is_err() {
            return Err(SecretStoreError::InvalidNamespace);
        }
        Ok((namespace, key))
    }

    fn write_atomic(&self, path: &Path, secret: &str) -> Result<(), SecretStoreError> {
        if secret.len() > MAX_AUTH_FILE_BYTES {
            return Err(SecretStoreError::SecretTooLarge);
        }
        let parent = path.parent().ok_or(SecretStoreError::Unavailable)?;
        std::fs::create_dir_all(parent).map_err(|_| SecretStoreError::Unavailable)?;
        set_private_permissions(parent, 0o700)?;

        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| SecretStoreError::Unavailable)?;
        temporary
            .write_all(secret.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| SecretStoreError::Unavailable)?;
        set_private_permissions(temporary.path(), 0o600)?;
        temporary
            .persist(path)
            .map_err(|_| SecretStoreError::Unavailable)?;

        #[cfg(unix)]
        if std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            tracing::warn!("auth-file directory sync failed after commit");
        }
        Ok(())
    }

    fn read_file(&self, path: &Path) -> Result<String, SecretStoreError> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SecretStoreError::NotFound
            } else {
                SecretStoreError::Unavailable
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SecretStoreError::Unavailable);
        }
        let bytes = std::fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SecretStoreError::NotFound
            } else {
                SecretStoreError::Unavailable
            }
        })?;
        if bytes.len() > MAX_AUTH_FILE_BYTES {
            return Err(SecretStoreError::SecretTooLarge);
        }
        String::from_utf8(bytes).map_err(|_| SecretStoreError::Unavailable)
    }
}

#[async_trait::async_trait]
impl SecretStore for AuthFileStore {
    fn planned_ref(&self, namespace: &str, key: &str) -> Result<SecretRef, SecretStoreError> {
        let namespace = Self::validate_namespace(namespace)?;
        let key = uuid::Uuid::parse_str(key).map_err(|_| SecretStoreError::InvalidNamespace)?;
        Ok(SecretRef::new(
            AUTH_FILE_BACKEND,
            format!("{namespace}:{key}"),
        ))
    }

    async fn create_named(
        &self,
        namespace: &str,
        key: &str,
        secret: &str,
    ) -> Result<SecretRef, SecretStoreError> {
        let secret_ref = self.planned_ref(namespace, key)?;
        let path = self.path_for_ref(&secret_ref)?;
        self.write_atomic(&path, secret)?;
        Ok(secret_ref)
    }

    async fn read(&self, secret_ref: &SecretRef) -> Result<String, SecretStoreError> {
        let path = self.path_for_ref(secret_ref)?;
        self.read_file(&path)
    }

    async fn replace(&self, secret_ref: &SecretRef, secret: &str) -> Result<(), SecretStoreError> {
        let path = self.path_for_ref(secret_ref)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SecretStoreError::NotFound
            } else {
                SecretStoreError::Unavailable
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SecretStoreError::Unavailable);
        }
        self.write_atomic(&path, secret)
    }

    async fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError> {
        let path = self.path_for_ref(secret_ref)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SecretStoreError::Unavailable),
        }
    }
}

/// Codex storage used during the transition from the previous keyring-only
/// implementation to provider auth-files. New credentials always go to the
/// auth-file store; existing Codex keyring references remain readable.
#[derive(Debug, Clone)]
pub struct CodexAuthStore {
    auth_files: AuthFileStore,
    legacy_keyring: KeyringSecretStore,
}

impl CodexAuthStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            auth_files: AuthFileStore::new(root),
            legacy_keyring: KeyringSecretStore::default(),
        }
    }

    pub fn auth_files(&self) -> &AuthFileStore {
        &self.auth_files
    }
}

#[async_trait::async_trait]
impl SecretStore for CodexAuthStore {
    fn planned_ref(&self, namespace: &str, key: &str) -> Result<SecretRef, SecretStoreError> {
        self.auth_files.planned_ref(namespace, key)
    }

    async fn create_named(
        &self,
        namespace: &str,
        key: &str,
        secret: &str,
    ) -> Result<SecretRef, SecretStoreError> {
        self.auth_files.create_named(namespace, key, secret).await
    }

    async fn read(&self, secret_ref: &SecretRef) -> Result<String, SecretStoreError> {
        match secret_ref.backend.as_str() {
            AUTH_FILE_BACKEND => self.auth_files.read(secret_ref).await,
            "keyring" => self.legacy_keyring.read(secret_ref).await,
            _ => Err(SecretStoreError::BackendMismatch),
        }
    }

    async fn replace(&self, secret_ref: &SecretRef, secret: &str) -> Result<(), SecretStoreError> {
        match secret_ref.backend.as_str() {
            AUTH_FILE_BACKEND => self.auth_files.replace(secret_ref, secret).await,
            "keyring" => self.legacy_keyring.replace(secret_ref, secret).await,
            _ => Err(SecretStoreError::BackendMismatch),
        }
    }

    async fn delete(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError> {
        match secret_ref.backend.as_str() {
            AUTH_FILE_BACKEND => self.auth_files.delete(secret_ref).await,
            "keyring" => self.legacy_keyring.delete(secret_ref).await,
            _ => Err(SecretStoreError::BackendMismatch),
        }
    }
}

fn set_private_permissions(path: &Path, mode: u32) -> Result<(), SecretStoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|_| SecretStoreError::Unavailable)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::SecretRef;
    use crate::modules::secret_store::SecretStore;

    #[tokio::test]
    async fn stores_auth_material_in_a_private_provider_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = AuthFileStore::new(directory.path());
        let key = uuid::Uuid::new_v4().to_string();

        let secret_ref = store
            .create_named("codex-oauth", &key, "access-canary")
            .await
            .unwrap();

        assert_eq!(secret_ref.backend, AUTH_FILE_BACKEND);
        assert!(!secret_ref.key.contains("access-canary"));
        assert_eq!(store.read(&secret_ref).await.unwrap(), "access-canary");

        let path = store.path_for_ref(&secret_ref).unwrap();
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("json")
        );
        assert!(path.starts_with(directory.path()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn replacement_is_atomic_and_delete_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let store = AuthFileStore::new(directory.path());
        let key = uuid::Uuid::new_v4().to_string();
        let secret_ref = store
            .create_named("codex-oauth", &key, "old-secret")
            .await
            .unwrap();

        store.replace(&secret_ref, "new-secret").await.unwrap();
        assert_eq!(store.read(&secret_ref).await.unwrap(), "new-secret");
        store.delete(&secret_ref).await.unwrap();
        store.delete(&secret_ref).await.unwrap();
        assert!(matches!(
            store.read(&secret_ref).await,
            Err(SecretStoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn codex_store_writes_new_credentials_to_auth_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = CodexAuthStore::new(directory.path());
        let key = uuid::Uuid::new_v4().to_string();

        let secret_ref = store
            .create_named("codex-oauth", &key, "{\"access_token\":\"redacted\"}")
            .await
            .unwrap();

        assert_eq!(secret_ref.backend, AUTH_FILE_BACKEND);
        assert!(store
            .auth_files()
            .path_for_ref(&secret_ref)
            .unwrap()
            .exists());
    }

    #[tokio::test]
    async fn rejects_foreign_backends_and_path_traversal_keys() {
        let directory = tempfile::tempdir().unwrap();
        let store = AuthFileStore::new(directory.path());

        assert!(matches!(
            store.read(&SecretRef::new("keyring", "anything")).await,
            Err(SecretStoreError::BackendMismatch)
        ));
        assert!(matches!(
            store.read(&SecretRef::new("auth_file", "../outside")).await,
            Err(SecretStoreError::InvalidNamespace)
                | Err(SecretStoreError::NotFound)
                | Err(SecretStoreError::Unavailable)
        ));
    }
}
