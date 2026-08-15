use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use mako_api::{ExplorerAccessMode, ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

const UPDATE_ATTEMPTS: usize = 8;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerGrantAuthorityRecord {
    pub nonce: String,
    pub developer_identity_id: String,
    pub mode: ExplorerAccessMode,
    pub authorization_epoch: u64,
    pub issued_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    pub revoked_at_unix_seconds: Option<u64>,
}

#[derive(Clone)]
pub struct ExplorerAuthorizationStore {
    adapter: Arc<dyn KvAdapter>,
    keyspace: TenantKeyspace,
    durability: Durability,
}

impl fmt::Debug for ExplorerAuthorizationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplorerAuthorizationStore")
            .field("keyspace", &self.keyspace)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ExplorerAuthorizationStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
        durability: Durability,
    ) -> Result<Self, ExplorerAuthorizationError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < durability {
            return Err(ExplorerAuthorizationError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            keyspace: TenantKeyspace::new(
                trusted_tenant.project_id().as_str(),
                trusted_tenant.environment_id().as_str(),
            )?,
            durability,
        })
    }

    pub async fn current_epoch(
        &self,
        developer_identity_id: &str,
    ) -> Result<u64, ExplorerAuthorizationError> {
        let key = self
            .keyspace
            .explorer_developer_epoch_key(developer_identity_id)?;
        self.adapter
            .get(&key)
            .await?
            .map_or(Ok(1), |bytes| decode_epoch(&bytes))
    }

    pub async fn record_issue(
        &self,
        record: &ExplorerGrantAuthorityRecord,
    ) -> Result<(), ExplorerAuthorizationError> {
        if record.authorization_epoch == 0
            || record.expires_at_unix_seconds <= record.issued_at_unix_seconds
            || record.revoked_at_unix_seconds.is_some()
        {
            return Err(ExplorerAuthorizationError::InvalidRecord);
        }
        let current = self.current_epoch(&record.developer_identity_id).await?;
        if current != record.authorization_epoch {
            return Err(ExplorerAuthorizationError::StaleEpoch);
        }
        let grant_key = self.keyspace.explorer_grant_key(&record.nonce)?;
        let epoch_key = self
            .keyspace
            .explorer_developer_epoch_key(&record.developer_identity_id)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&grant_key, serde_json::to_vec(record)?);
        let mut conditions = vec![KeyCondition::Missing {
            key: grant_key.clone(),
        }];
        if self.adapter.get(&epoch_key).await?.is_none() {
            conditions.push(KeyCondition::Missing {
                key: epoch_key.clone(),
            });
            batch.put(&epoch_key, 1_u64.to_be_bytes());
        } else {
            conditions.push(KeyCondition::ValueEquals {
                key: epoch_key,
                value: current.to_be_bytes().to_vec(),
            });
        }
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => {
                Err(ExplorerAuthorizationError::ConcurrentUpdate)
            }
        }
    }

    pub async fn validate_grant(
        &self,
        nonce: &str,
        developer_identity_id: &str,
        mode: ExplorerAccessMode,
        authorization_epoch: u64,
        now_unix_seconds: u64,
    ) -> Result<ExplorerGrantAuthorityRecord, ExplorerAuthorizationError> {
        let key = self.keyspace.explorer_grant_key(nonce)?;
        let bytes = self
            .adapter
            .get(&key)
            .await?
            .ok_or(ExplorerAuthorizationError::NotFound)?;
        let record: ExplorerGrantAuthorityRecord = serde_json::from_slice(&bytes)?;
        if record.nonce != nonce
            || record.developer_identity_id != developer_identity_id
            || record.mode != mode
            || record.authorization_epoch != authorization_epoch
            || record.revoked_at_unix_seconds.is_some()
            || now_unix_seconds >= record.expires_at_unix_seconds
            || self.current_epoch(developer_identity_id).await? != authorization_epoch
        {
            return Err(ExplorerAuthorizationError::NotAuthorized);
        }
        Ok(record)
    }

    pub async fn revoke(
        &self,
        nonce: &str,
        developer_identity_id: &str,
        now_unix_seconds: u64,
    ) -> Result<ExplorerGrantAuthorityRecord, ExplorerAuthorizationError> {
        let key = self.keyspace.explorer_grant_key(nonce)?;
        let bytes = self
            .adapter
            .get(&key)
            .await?
            .ok_or(ExplorerAuthorizationError::NotFound)?;
        let mut record: ExplorerGrantAuthorityRecord = serde_json::from_slice(&bytes)?;
        if record.developer_identity_id != developer_identity_id {
            return Err(ExplorerAuthorizationError::NotFound);
        }
        if record.revoked_at_unix_seconds.is_some() {
            return Ok(record);
        }
        record.revoked_at_unix_seconds = Some(now_unix_seconds);
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals { key, value: bytes }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(record),
            CompareAndWriteResult::Conflict { .. } => {
                Err(ExplorerAuthorizationError::ConcurrentUpdate)
            }
        }
    }

    pub async fn advance_epoch(
        &self,
        developer_identity_id: &str,
    ) -> Result<u64, ExplorerAuthorizationError> {
        let key = self
            .keyspace
            .explorer_developer_epoch_key(developer_identity_id)?;
        for _ in 0..UPDATE_ATTEMPTS {
            let previous = self.adapter.get(&key).await?;
            let current = previous.as_deref().map_or(Ok(1), decode_epoch)?;
            let next = current
                .checked_add(1)
                .ok_or(ExplorerAuthorizationError::CounterExhausted)?;
            let condition = previous.map_or_else(
                || KeyCondition::Missing { key: key.clone() },
                |value| KeyCondition::ValueEquals {
                    key: key.clone(),
                    value,
                },
            );
            let mut batch = WriteBatch::new();
            batch.put(&key, next.to_be_bytes());
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
                return Ok(next);
            }
        }
        Err(ExplorerAuthorizationError::ConcurrentUpdate)
    }

    /// Advances every developer explorer epoch in this tenant. This is used for tenant-wide
    /// lifecycle changes (for example an application user's policy context changing) so an
    /// already-issued preview capability cannot outlive the authorization change.
    pub async fn advance_all_epochs(&self) -> Result<usize, ExplorerAuthorizationError> {
        let mut range = self.keyspace.explorer_developer_epochs_range()?;
        let mut advanced = 0_usize;
        loop {
            let values = self
                .adapter
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    NonZeroUsize::new(256).expect("epoch scan batch is non-zero"),
                ))
                .await?;
            if values.is_empty() {
                return Ok(advanced);
            }
            for value in &values {
                self.advance_epoch_key(&value.key).await?;
                advanced = advanced.saturating_add(1);
            }
            if values.len() < 256 {
                return Ok(advanced);
            }
            let mut after = values.last().expect("non-empty epoch page").key.clone();
            after.push(0);
            if after >= range.end_exclusive {
                return Ok(advanced);
            }
            range.start_inclusive = after;
        }
    }

    async fn advance_epoch_key(&self, key: &[u8]) -> Result<u64, ExplorerAuthorizationError> {
        for _ in 0..UPDATE_ATTEMPTS {
            let previous = self
                .adapter
                .get(key)
                .await?
                .ok_or(ExplorerAuthorizationError::ConcurrentUpdate)?;
            let current = decode_epoch(&previous)?;
            let next = current
                .checked_add(1)
                .ok_or(ExplorerAuthorizationError::CounterExhausted)?;
            let mut batch = WriteBatch::new();
            batch.put(key, next.to_be_bytes());
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: key.to_vec(),
                        value: previous,
                    }],
                    batch,
                    durability: self.durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(next);
            }
        }
        Err(ExplorerAuthorizationError::ConcurrentUpdate)
    }
}

fn decode_epoch(bytes: &[u8]) -> Result<u64, ExplorerAuthorizationError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| ExplorerAuthorizationError::CorruptRecord)?;
    let value = u64::from_be_bytes(encoded);
    if value == 0 {
        return Err(ExplorerAuthorizationError::CorruptRecord);
    }
    Ok(value)
}

#[derive(Debug)]
pub enum ExplorerAuthorizationError {
    UnsupportedDurability,
    InvalidRecord,
    NotFound,
    NotAuthorized,
    StaleEpoch,
    ConcurrentUpdate,
    CounterExhausted,
    CorruptRecord,
    Scope(ScopeError),
    Key(mako_storage::KeyCodecError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ExplorerAuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("explorer authorization state was not accepted")
    }
}

impl Error for ExplorerAuthorizationError {}

impl From<ScopeError> for ExplorerAuthorizationError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for ExplorerAuthorizationError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::Key(error)
    }
}

impl From<StorageError> for ExplorerAuthorizationError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for ExplorerAuthorizationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    #[test]
    fn revoke_and_epoch_advance_fail_closed() {
        block_on(async {
            let tenant = tenant();
            let store = ExplorerAuthorizationStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let record = ExplorerGrantAuthorityRecord {
                nonce: "nonce_abcdefghijklmnop".to_owned(),
                developer_identity_id: "dev_abcdefgh".to_owned(),
                mode: ExplorerAccessMode::Administrative,
                authorization_epoch: 1,
                issued_at_unix_seconds: 10,
                expires_at_unix_seconds: 100,
                revoked_at_unix_seconds: None,
            };
            store.record_issue(&record).await.expect("issue");
            store
                .validate_grant(
                    &record.nonce,
                    &record.developer_identity_id,
                    record.mode,
                    1,
                    20,
                )
                .await
                .expect("valid");
            store
                .revoke(&record.nonce, &record.developer_identity_id, 30)
                .await
                .expect("revoke");
            assert!(
                store
                    .validate_grant(
                        &record.nonce,
                        &record.developer_identity_id,
                        record.mode,
                        1,
                        31,
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                store
                    .advance_epoch(&record.developer_identity_id)
                    .await
                    .expect("advance"),
                2
            );
        });
    }

    #[test]
    fn tenant_wide_epoch_advance_revokes_every_developer() {
        block_on(async {
            let tenant = tenant();
            let store = ExplorerAuthorizationStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            for (index, developer) in ["dev_abcdefgh", "dev_ijklmnop"].into_iter().enumerate() {
                store
                    .record_issue(&ExplorerGrantAuthorityRecord {
                        nonce: format!("nonce_{index:016}"),
                        developer_identity_id: developer.to_owned(),
                        mode: ExplorerAccessMode::Administrative,
                        authorization_epoch: 1,
                        issued_at_unix_seconds: 10,
                        expires_at_unix_seconds: 100,
                        revoked_at_unix_seconds: None,
                    })
                    .await
                    .expect("issue");
            }
            assert_eq!(store.advance_all_epochs().await.expect("advance all"), 2);
            assert_eq!(store.current_epoch("dev_abcdefgh").await.expect("epoch"), 2);
            assert_eq!(store.current_epoch("dev_ijklmnop").await.expect("epoch"), 2);
        });
    }
}
