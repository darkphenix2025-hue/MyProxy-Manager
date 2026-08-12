use crate::models::connection::{
    AccountPlatformDocumentV3, Credential, Identity, ProviderConnection,
};
use std::io::Write;

#[derive(serde::Serialize, serde::Deserialize)]
struct PendingOnboarding {
    version: u8,
    backend: String,
    key: String,
    created_at: i64,
}

pub type AccountPlatformParts = (Vec<Identity>, Vec<Credential>, Vec<ProviderConnection>);

#[derive(Debug, thiserror::Error)]
pub enum AccountPlatformStoreError {
    #[error("account platform storage is unavailable")]
    Unavailable,
    #[error("account platform document is invalid")]
    InvalidDocument,
    #[error("account platform record identifiers conflict")]
    Conflict,
    #[error("account platform references are inconsistent")]
    InvalidReference,
}

#[derive(Debug, Clone)]
pub struct AccountPlatformStore {
    path: std::path::PathBuf,
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl AccountPlatformStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let lock = store_locks()
            .entry(path.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        Self { path, lock }
    }

    pub async fn load(&self) -> Result<AccountPlatformParts, AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        self.load_unlocked()
    }

    pub async fn append(
        &self,
        identity: Identity,
        credential: Credential,
        connection: ProviderConnection,
    ) -> Result<(), AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        validate_record(&identity, &credential, &connection)?;
        let (mut identities, mut credentials, mut connections) = self.load_unlocked()?;
        if identities.iter().any(|value| value.id == identity.id)
            || credentials.iter().any(|value| value.id == credential.id)
            || connections.iter().any(|value| value.id == connection.id)
        {
            return Err(AccountPlatformStoreError::Conflict);
        }
        identities.push(identity);
        credentials.push(credential);
        connections.push(connection);
        self.write_unlocked(AccountPlatformDocumentV3::new(
            identities,
            credentials,
            connections,
        ))
    }

    pub async fn begin_onboarding(
        &self,
        secret_ref: &crate::models::connection::SecretRef,
    ) -> Result<(), AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        let pending = PendingOnboarding {
            version: 1,
            backend: secret_ref.backend.clone(),
            key: secret_ref.key.clone(),
            created_at: chrono::Utc::now().timestamp(),
        };
        if self.pending_path().exists() {
            return Err(AccountPlatformStoreError::Conflict);
        }
        self.write_json_atomic(&self.pending_path(), &pending)
    }

    pub async fn pending_onboarding(
        &self,
    ) -> Result<Option<crate::models::connection::SecretRef>, AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        let bytes = match std::fs::read(self.pending_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(AccountPlatformStoreError::Unavailable),
        };
        let pending: PendingOnboarding = serde_json::from_slice(&bytes)
            .map_err(|_| AccountPlatformStoreError::InvalidDocument)?;
        if pending.version != 1
            || pending.backend.is_empty()
            || pending.key.is_empty()
            || pending.created_at <= 0
        {
            return Err(AccountPlatformStoreError::InvalidDocument);
        }
        Ok(Some(crate::models::connection::SecretRef::new(
            pending.backend,
            pending.key,
        )))
    }

    pub async fn stale_onboarding(
        &self,
        now: i64,
        lease_seconds: i64,
    ) -> Result<Option<crate::models::connection::SecretRef>, AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        let bytes = match std::fs::read(self.pending_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(AccountPlatformStoreError::Unavailable),
        };
        let pending: PendingOnboarding = serde_json::from_slice(&bytes)
            .map_err(|_| AccountPlatformStoreError::InvalidDocument)?;
        if pending.version != 1 || pending.created_at <= 0 {
            return Err(AccountPlatformStoreError::InvalidDocument);
        }
        if pending.created_at > now.saturating_sub(lease_seconds.max(0)) {
            return Ok(None);
        }
        Ok(Some(crate::models::connection::SecretRef::new(
            pending.backend,
            pending.key,
        )))
    }

    pub async fn clear_onboarding(
        &self,
        expected: &crate::models::connection::SecretRef,
    ) -> Result<(), AccountPlatformStoreError> {
        let _guard = self.lock.lock().await;
        let _file_lock = self.acquire_file_lock()?;
        let pending = match std::fs::read(self.pending_path()) {
            Ok(bytes) => serde_json::from_slice::<PendingOnboarding>(&bytes)
                .map_err(|_| AccountPlatformStoreError::InvalidDocument)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(AccountPlatformStoreError::Unavailable),
        };
        if pending.backend != expected.backend || pending.key != expected.key {
            return Ok(());
        }
        match std::fs::remove_file(self.pending_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AccountPlatformStoreError::Unavailable),
        }
    }

    fn load_unlocked(&self) -> Result<AccountPlatformParts, AccountPlatformStoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new(), Vec::new()));
            }
            Err(_) => return Err(AccountPlatformStoreError::Unavailable),
        };
        serde_json::from_slice::<AccountPlatformDocumentV3>(&bytes)
            .map(AccountPlatformDocumentV3::into_parts)
            .map_err(|_| AccountPlatformStoreError::InvalidDocument)
    }

    fn acquire_file_lock(&self) -> Result<std::fs::File, AccountPlatformStoreError> {
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(AccountPlatformStoreError::Unavailable)?;
        std::fs::create_dir_all(parent).map_err(|_| AccountPlatformStoreError::Unavailable)?;
        let lock_path = self.path.with_extension("lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(lock_path)
            .map_err(|_| AccountPlatformStoreError::Unavailable)?;
        fs2::FileExt::lock_exclusive(&file).map_err(|_| AccountPlatformStoreError::Unavailable)?;
        Ok(file)
    }

    fn write_unlocked(
        &self,
        document: AccountPlatformDocumentV3,
    ) -> Result<(), AccountPlatformStoreError> {
        self.write_json_atomic(&self.path, &document)
    }

    fn write_json_atomic<T: serde::Serialize>(
        &self,
        path: &std::path::Path,
        value: &T,
    ) -> Result<(), AccountPlatformStoreError> {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(AccountPlatformStoreError::Unavailable)?;
        std::fs::create_dir_all(parent).map_err(|_| AccountPlatformStoreError::Unavailable)?;
        let encoded = serde_json::to_vec_pretty(value)
            .map_err(|_| AccountPlatformStoreError::InvalidDocument)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| AccountPlatformStoreError::Unavailable)?;
        temporary
            .write_all(&encoded)
            .and_then(|_| temporary.write_all(b"\n"))
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| AccountPlatformStoreError::Unavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| AccountPlatformStoreError::Unavailable)?;
        }
        temporary
            .persist(path)
            .map_err(|_| AccountPlatformStoreError::Unavailable)?;
        // The rename is the commit point. Directory sync improves crash durability,
        // but failure after a successful rename must not be reported as an uncommitted
        // write (the caller could otherwise delete the now-referenced secret).
        #[cfg(unix)]
        if std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            tracing::warn!("account metadata directory sync failed after commit");
        }
        Ok(())
    }

    fn pending_path(&self) -> std::path::PathBuf {
        self.path.with_extension("pending.json")
    }
}

fn store_locks(
) -> &'static dashmap::DashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>> {
    static LOCKS: std::sync::OnceLock<
        dashmap::DashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>,
    > = std::sync::OnceLock::new();
    LOCKS.get_or_init(dashmap::DashMap::new)
}

fn validate_record(
    identity: &Identity,
    credential: &Credential,
    connection: &ProviderConnection,
) -> Result<(), AccountPlatformStoreError> {
    if credential.identity_id.as_deref() != Some(identity.id.as_str())
        || connection.identity_id.as_deref() != Some(identity.id.as_str())
        || connection.credential_id != credential.id
    {
        return Err(AccountPlatformStoreError::InvalidReference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::{
        AuthKind, ConnectionStatus, CredentialLifecycle, SecretRef, WireProtocol,
    };

    fn record(suffix: &str) -> (Identity, Credential, ProviderConnection) {
        let identity_id = format!("identity-{suffix}");
        let credential_id = format!("credential-{suffix}");
        let mut identity = Identity::new(&identity_id, "openai");
        identity.subject = Some(format!("sha256:{suffix}"));
        let mut credential = Credential::new(
            &credential_id,
            Some(identity_id.clone()),
            AuthKind::CodexOauthTokenSet,
            SecretRef::new("memory", format!("secret-{suffix}")),
            format!("sha256:{suffix}"),
        );
        credential.lifecycle = CredentialLifecycle::Ready;
        let mut connection = ProviderConnection::new(
            format!("connection-{suffix}"),
            "openai_codex",
            credential_id,
            "https://chatgpt.com/backend-api/codex",
            vec![WireProtocol::CodexResponsesUpstream],
        );
        connection.identity_id = Some(identity_id);
        connection.status = ConnectionStatus::Ready;
        connection.enabled = true;
        (identity, credential, connection)
    }

    #[tokio::test]
    async fn missing_store_loads_as_empty_v3_document() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));

        let (identities, credentials, connections) = store.load().await.unwrap();

        assert!(identities.is_empty());
        assert!(credentials.is_empty());
        assert!(connections.is_empty());
    }

    #[tokio::test]
    async fn concurrent_appends_are_atomic_and_preserve_all_records() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));
        let first = tokio::spawn({
            let store = store.clone();
            async move {
                let (identity, credential, connection) = record("one");
                store.append(identity, credential, connection).await
            }
        });
        let second = tokio::spawn({
            let store = store.clone();
            async move {
                let (identity, credential, connection) = record("two");
                store.append(identity, credential, connection).await
            }
        });
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();

        let (identities, credentials, connections) = store.load().await.unwrap();

        assert_eq!(identities.len(), 2);
        assert_eq!(credentials.len(), 2);
        assert_eq!(connections.len(), 2);
        let raw = std::fs::read_to_string(directory.path().join("accounts-v3.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["schema_version"],
            3
        );
    }

    #[tokio::test]
    async fn append_rejects_inconsistent_references_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts-v3.json");
        let store = AccountPlatformStore::new(&path);
        let (identity, credential, mut connection) = record("broken");
        connection.credential_id = "missing-credential".to_string();

        assert!(matches!(
            store.append(identity, credential, connection).await,
            Err(AccountPlatformStoreError::InvalidReference)
        ));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn onboarding_journal_roundtrips_internal_secret_reference() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));
        let secret_ref = SecretRef::new("memory", "secret-canary");

        store.begin_onboarding(&secret_ref).await.unwrap();
        assert_eq!(
            store.pending_onboarding().await.unwrap(),
            Some(secret_ref.clone())
        );

        store.clear_onboarding(&secret_ref).await.unwrap();
        assert!(store.pending_onboarding().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn compare_and_delete_does_not_remove_a_newer_onboarding() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));
        let first = SecretRef::new("memory", "secret-first");
        let second = SecretRef::new("memory", "secret-second");

        store.begin_onboarding(&first).await.unwrap();
        store.clear_onboarding(&second).await.unwrap();
        assert_eq!(
            store.pending_onboarding().await.unwrap(),
            Some(first.clone())
        );
        store.clear_onboarding(&first).await.unwrap();
        assert!(store.pending_onboarding().await.unwrap().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_document_is_owner_readable_and_writable_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts-v3.json");
        let store = AccountPlatformStore::new(&path);
        let (identity, credential, connection) = record("mode");
        store
            .append(identity, credential, connection)
            .await
            .unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
