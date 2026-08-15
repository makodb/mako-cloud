use std::{error::Error, fmt, sync::Arc};

use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCodecError, KeyCondition, KvAdapter,
    StorageError, TenantKeyspace, WriteBatch,
};

use crate::{
    JsonWebKeySet, KeyEncryptionKey, ProjectSigningKeyRecord, ProjectSigningKeyRing,
    SigningKeyError,
};

/// Persistent, tenant-bound owner of application JWT signing-key records.
///
/// The data plane constructs this store over its exclusive adapter. Private key
/// material remains encrypted by [`KeyEncryptionKey`] inside the identity
/// namespace; only the public JWKS crosses verification boundaries.
#[derive(Clone)]
pub struct ProjectSigningKeyStore {
    adapter: Arc<dyn KvAdapter>,
    tenant: TenantScope,
    key: Vec<u8>,
    durability: Durability,
}

impl fmt::Debug for ProjectSigningKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSigningKeyStore")
            .field("tenant", &self.tenant)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ProjectSigningKeyStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
        durability: Durability,
    ) -> Result<Self, ProjectSigningKeyStoreError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < durability {
            return Err(ProjectSigningKeyStoreError::UnsupportedDurability(
                durability,
            ));
        }
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str().as_bytes(),
            trusted_tenant.environment_id().as_str().as_bytes(),
        )?;
        Ok(Self {
            adapter,
            tenant: trusted_tenant.clone(),
            key: keyspace.project_signing_key_ring_key(),
            durability,
        })
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    pub async fn key_ring(&self) -> Result<ProjectSigningKeyRing, ProjectSigningKeyStoreError> {
        self.load().await.map(|(_, ring)| ring)
    }

    pub async fn jwks(&self) -> Result<JsonWebKeySet, ProjectSigningKeyStoreError> {
        self.key_ring().await.map(|ring| ring.jwks())
    }

    pub async fn initialize(
        &self,
        encryption_key: &KeyEncryptionKey,
        now_unix_seconds: u64,
    ) -> Result<ProjectSigningKeyRecord, ProjectSigningKeyStoreError> {
        let (previous, mut ring) = self.load().await?;
        let created = ring
            .create_initial(encryption_key, now_unix_seconds)?
            .clone();
        self.save(previous, &ring).await?;
        Ok(created)
    }

    pub async fn rotate(
        &self,
        encryption_key: &KeyEncryptionKey,
        now_unix_seconds: u64,
        overlap_seconds: u64,
    ) -> Result<ProjectSigningKeyRecord, ProjectSigningKeyStoreError> {
        let (previous, mut ring) = self.load().await?;
        let created = ring
            .rotate(encryption_key, now_unix_seconds, overlap_seconds)?
            .clone();
        self.save(previous, &ring).await?;
        Ok(created)
    }

    pub async fn retire_due(
        &self,
        now_unix_seconds: u64,
    ) -> Result<usize, ProjectSigningKeyStoreError> {
        let (previous, mut ring) = self.load().await?;
        let retired = ring.retire_due(now_unix_seconds);
        if retired > 0 {
            self.save(previous, &ring).await?;
        }
        Ok(retired)
    }

    async fn load(
        &self,
    ) -> Result<(Option<Vec<u8>>, ProjectSigningKeyRing), ProjectSigningKeyStoreError> {
        let stored = self.adapter.get(&self.key).await?;
        let records = stored
            .as_deref()
            .map(serde_json::from_slice::<Vec<ProjectSigningKeyRecord>>)
            .transpose()?
            .unwrap_or_default();
        let ring = ProjectSigningKeyRing::from_records(self.tenant.clone(), records)?;
        Ok((stored, ring))
    }

    async fn save(
        &self,
        previous: Option<Vec<u8>>,
        ring: &ProjectSigningKeyRing,
    ) -> Result<(), ProjectSigningKeyStoreError> {
        let condition = previous.map_or_else(
            || KeyCondition::Missing {
                key: self.key.clone(),
            },
            |value| KeyCondition::ValueEquals {
                key: self.key.clone(),
                value,
            },
        );
        let mut batch = WriteBatch::new();
        batch.put(&self.key, serde_json::to_vec(ring.records())?);
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![condition],
                batch,
                durability: self.durability,
            })
            .await?
            == CompareAndWriteResult::Applied
        {
            Ok(())
        } else {
            Err(ProjectSigningKeyStoreError::ConcurrentChange)
        }
    }
}

#[derive(Debug)]
pub enum ProjectSigningKeyStoreError {
    Scope(ScopeError),
    KeyCodec(KeyCodecError),
    Storage(StorageError),
    Signing(SigningKeyError),
    Json(serde_json::Error),
    UnsupportedDurability(Durability),
    ConcurrentChange,
}

impl fmt::Display for ProjectSigningKeyStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Signing(error) => error.fmt(formatter),
            Self::Json(_) => formatter.write_str("project signing-key records are invalid"),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support {durability:?} durability"
                )
            }
            Self::ConcurrentChange => formatter.write_str("project signing-key ring changed"),
        }
    }
}

impl Error for ProjectSigningKeyStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Signing(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::UnsupportedDurability(_) | Self::ConcurrentChange => None,
        }
    }
}

impl From<ScopeError> for ProjectSigningKeyStoreError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<KeyCodecError> for ProjectSigningKeyStoreError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for ProjectSigningKeyStoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<SigningKeyError> for ProjectSigningKeyStoreError {
    fn from(error: SigningKeyError) -> Self {
        Self::Signing(error)
    }
}

impl From<serde_json::Error> for ProjectSigningKeyStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    #[test]
    fn persists_encrypted_key_rotation_in_one_tenant_namespace() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant("prj_example00", "env_example00");
            let store = ProjectSigningKeyStore::new(adapter, &tenant, &tenant, Durability::Memory)
                .expect("store");
            let encryption_key = KeyEncryptionKey::from_bytes([7; 32]);
            let first = store
                .initialize(&encryption_key, 10)
                .await
                .expect("initialize");
            let second = store.rotate(&encryption_key, 20, 30).await.expect("rotate");

            let ring = store.key_ring().await.expect("ring");
            assert_eq!(ring.records().len(), 2);
            assert_ne!(first.key_id(), second.key_id());
            assert_eq!(store.jwks().await.expect("JWKS").keys.len(), 2);
            assert_eq!(store.retire_due(49).await.expect("not due"), 0);
            assert_eq!(store.retire_due(50).await.expect("retire"), 1);
            assert_eq!(store.jwks().await.expect("JWKS").keys.len(), 1);
        });
    }

    #[test]
    fn rejects_cross_tenant_scope_before_storage() {
        let adapter = Arc::new(MemoryAdapter::new());
        let requested = tenant("prj_example00", "env_example00");
        let trusted = tenant("prj_other000", "env_example00");
        assert!(matches!(
            ProjectSigningKeyStore::new(adapter, &requested, &trusted, Durability::Memory,),
            Err(ProjectSigningKeyStoreError::Scope(_))
        ));
    }

    fn tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse(environment).expect("environment"),
        )
    }
}
