use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::SubjectId;

const INVALIDATION_ENCODING_VERSION: u8 = 1;
const MAX_UPDATE_ATTEMPTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AuthorizationEpoch(u64);

impl AuthorizationEpoch {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, AuthorizationEpochError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(AuthorizationEpochError::CounterExhausted)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct InvalidationSequence(u64);

impl InvalidationSequence {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, AuthorizationEpochError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(AuthorizationEpochError::CounterExhausted)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AuthorizationEpochSnapshot {
    environment: AuthorizationEpoch,
    user: AuthorizationEpoch,
}

impl AuthorizationEpochSnapshot {
    #[must_use]
    pub const fn environment(self) -> AuthorizationEpoch {
        self.environment
    }

    #[must_use]
    pub const fn user(self) -> AuthorizationEpoch {
        self.user
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AuthorizationInvalidationKind {
    PolicyActivated {
        collection: String,
        policy_version: u64,
    },
    TrustedClaimsChanged {
        subject: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationInvalidation {
    encoding_version: u8,
    sequence: InvalidationSequence,
    environment_epoch: AuthorizationEpoch,
    user_epoch: Option<AuthorizationEpoch>,
    kind: AuthorizationInvalidationKind,
}

impl AuthorizationInvalidation {
    #[must_use]
    pub const fn sequence(&self) -> InvalidationSequence {
        self.sequence
    }

    #[must_use]
    pub const fn environment_epoch(&self) -> AuthorizationEpoch {
        self.environment_epoch
    }

    #[must_use]
    pub const fn user_epoch(&self) -> Option<AuthorizationEpoch> {
        self.user_epoch
    }

    #[must_use]
    pub const fn kind(&self) -> &AuthorizationInvalidationKind {
        &self.kind
    }

    fn encode(&self) -> Result<Vec<u8>, AuthorizationEpochError> {
        serde_json::to_vec(self).map_err(Into::into)
    }

    fn decode(bytes: &[u8]) -> Result<Self, AuthorizationEpochError> {
        let event: Self = serde_json::from_slice(bytes)?;
        if event.encoding_version != INVALIDATION_ENCODING_VERSION {
            return Err(AuthorizationEpochError::UnsupportedEncodingVersion(
                event.encoding_version,
            ));
        }
        Ok(event)
    }
}

#[derive(Clone)]
pub struct AuthorizationEpochStore {
    adapter: Arc<dyn KvAdapter>,
    keyspace: TenantKeyspace,
    durability: Durability,
}

impl fmt::Debug for AuthorizationEpochStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationEpochStore")
            .field("keyspace", &self.keyspace)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl AuthorizationEpochStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
        durability: Durability,
    ) -> Result<Self, AuthorizationEpochError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < durability {
            return Err(AuthorizationEpochError::UnsupportedDurability(durability));
        }
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str().as_bytes(),
            trusted_tenant.environment_id().as_str().as_bytes(),
        )?;
        Ok(Self {
            adapter,
            keyspace,
            durability,
        })
    }

    pub async fn epochs_for(
        &self,
        subject: &SubjectId,
    ) -> Result<AuthorizationEpochSnapshot, AuthorizationEpochError> {
        let environment = read_counter(
            self.adapter.as_ref(),
            self.keyspace.authorization_environment_epoch_key(),
        )
        .await?
        .value;
        let user = read_counter(
            self.adapter.as_ref(),
            self.keyspace
                .authorization_user_epoch_key(subject.as_str())?,
        )
        .await?
        .value;
        Ok(AuthorizationEpochSnapshot { environment, user })
    }

    pub async fn environment_epoch(&self) -> Result<AuthorizationEpoch, AuthorizationEpochError> {
        Ok(read_counter(
            self.adapter.as_ref(),
            self.keyspace.authorization_environment_epoch_key(),
        )
        .await?
        .value)
    }

    /// Records a verified administrator-controlled claim change. User-editable
    /// profile metadata must never call this path.
    pub async fn trusted_claims_changed(
        &self,
        subject: &SubjectId,
    ) -> Result<AuthorizationInvalidation, AuthorizationEpochError> {
        for _ in 0..MAX_UPDATE_ATTEMPTS {
            let prepared =
                prepare_user_advance(self.adapter.as_ref(), &self.keyspace, subject).await?;
            let event = prepared.event.clone();
            if apply_prepared(self.adapter.as_ref(), self.durability, prepared).await? {
                return Ok(event);
            }
        }
        Err(AuthorizationEpochError::ConcurrentUpdate)
    }

    pub async fn invalidations_after(
        &self,
        after: InvalidationSequence,
        limit: NonZeroUsize,
    ) -> Result<Vec<AuthorizationInvalidation>, AuthorizationEpochError> {
        let Some(range) = self
            .keyspace
            .authorization_invalidations_after(after.get())?
        else {
            return Ok(Vec::new());
        };
        let entries = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        entries
            .into_iter()
            .map(|entry| {
                let sequence = self
                    .keyspace
                    .decode_authorization_invalidation_key(&entry.key)?;
                let event = AuthorizationInvalidation::decode(&entry.value)?;
                if event.sequence.get() != sequence {
                    return Err(AuthorizationEpochError::CorruptInvalidation);
                }
                Ok(event)
            })
            .collect()
    }
}

pub(crate) struct PreparedAuthorizationAdvance {
    pub(crate) conditions: Vec<KeyCondition>,
    pub(crate) writes: Vec<(Vec<u8>, Vec<u8>)>,
    event: AuthorizationInvalidation,
}

pub(crate) async fn prepare_environment_advance(
    adapter: &dyn KvAdapter,
    keyspace: &TenantKeyspace,
    collection: &str,
    policy_version: u64,
) -> Result<PreparedAuthorizationAdvance, AuthorizationEpochError> {
    let environment = read_counter(adapter, keyspace.authorization_environment_epoch_key()).await?;
    let sequence =
        read_counter(adapter, keyspace.authorization_invalidation_sequence_key()).await?;
    let next_environment = environment.value.next()?;
    let next_sequence = InvalidationSequence(sequence.value.get()).next()?;
    let event = AuthorizationInvalidation {
        encoding_version: INVALIDATION_ENCODING_VERSION,
        sequence: next_sequence,
        environment_epoch: next_environment,
        user_epoch: None,
        kind: AuthorizationInvalidationKind::PolicyActivated {
            collection: collection.to_owned(),
            policy_version,
        },
    };
    prepare_advance(environment, None, sequence, event, keyspace)
}

async fn prepare_user_advance(
    adapter: &dyn KvAdapter,
    keyspace: &TenantKeyspace,
    subject: &SubjectId,
) -> Result<PreparedAuthorizationAdvance, AuthorizationEpochError> {
    let environment = read_counter(adapter, keyspace.authorization_environment_epoch_key()).await?;
    let user = read_counter(
        adapter,
        keyspace.authorization_user_epoch_key(subject.as_str())?,
    )
    .await?;
    let sequence =
        read_counter(adapter, keyspace.authorization_invalidation_sequence_key()).await?;
    let next_user = user.value.next()?;
    let next_sequence = InvalidationSequence(sequence.value.get()).next()?;
    let event = AuthorizationInvalidation {
        encoding_version: INVALIDATION_ENCODING_VERSION,
        sequence: next_sequence,
        environment_epoch: environment.value,
        user_epoch: Some(next_user),
        kind: AuthorizationInvalidationKind::TrustedClaimsChanged {
            subject: subject.as_str().to_owned(),
        },
    };
    prepare_advance(
        environment,
        Some((user, next_user)),
        sequence,
        event,
        keyspace,
    )
}

fn prepare_advance(
    environment: StoredCounter,
    user: Option<(StoredCounter, AuthorizationEpoch)>,
    sequence: StoredCounter,
    event: AuthorizationInvalidation,
    keyspace: &TenantKeyspace,
) -> Result<PreparedAuthorizationAdvance, AuthorizationEpochError> {
    let mut conditions = vec![environment.condition(), sequence.condition()];
    let mut writes = vec![
        (sequence.key, event.sequence.get().to_be_bytes().to_vec()),
        (
            keyspace.authorization_invalidation_key(event.sequence.get()),
            event.encode()?,
        ),
    ];
    match user {
        Some((stored, next)) => {
            conditions.push(stored.condition());
            writes.push((stored.key, next.get().to_be_bytes().to_vec()));
        }
        None => writes.push((
            environment.key,
            event.environment_epoch.get().to_be_bytes().to_vec(),
        )),
    }
    conditions.push(KeyCondition::Missing {
        key: keyspace.authorization_invalidation_key(event.sequence.get()),
    });
    Ok(PreparedAuthorizationAdvance {
        conditions,
        writes,
        event,
    })
}

async fn apply_prepared(
    adapter: &dyn KvAdapter,
    durability: Durability,
    prepared: PreparedAuthorizationAdvance,
) -> Result<bool, AuthorizationEpochError> {
    let mut batch = WriteBatch::with_capacity(prepared.writes.len());
    for (key, value) in prepared.writes {
        batch.put(key, value);
    }
    Ok(adapter
        .compare_and_write(AtomicWrite {
            conditions: prepared.conditions,
            batch,
            durability,
        })
        .await?
        == CompareAndWriteResult::Applied)
}

struct StoredCounter {
    key: Vec<u8>,
    bytes: Option<Vec<u8>>,
    value: AuthorizationEpoch,
}

impl StoredCounter {
    fn condition(&self) -> KeyCondition {
        match &self.bytes {
            Some(value) => KeyCondition::ValueEquals {
                key: self.key.clone(),
                value: value.clone(),
            },
            None => KeyCondition::Missing {
                key: self.key.clone(),
            },
        }
    }
}

async fn read_counter(
    adapter: &dyn KvAdapter,
    key: Vec<u8>,
) -> Result<StoredCounter, AuthorizationEpochError> {
    let bytes = adapter.get(&key).await?;
    let value = match bytes.as_deref() {
        Some(bytes) => {
            let encoded: [u8; 8] = bytes
                .try_into()
                .map_err(|_| AuthorizationEpochError::CorruptCounter)?;
            AuthorizationEpoch(u64::from_be_bytes(encoded))
        }
        None => AuthorizationEpoch::default(),
    };
    Ok(StoredCounter { key, bytes, value })
}

#[derive(Debug)]
pub enum AuthorizationEpochError {
    Scope(ScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Json(serde_json::Error),
    UnsupportedDurability(Durability),
    UnsupportedEncodingVersion(u8),
    CorruptCounter,
    CorruptInvalidation,
    CounterExhausted,
    ConcurrentUpdate,
}

impl fmt::Display for AuthorizationEpochError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::UnsupportedEncodingVersion(version) => {
                write!(
                    formatter,
                    "unsupported authorization invalidation encoding version {version}"
                )
            }
            Self::CorruptCounter => formatter.write_str("authorization epoch counter is corrupt"),
            Self::CorruptInvalidation => {
                formatter.write_str("authorization invalidation record is corrupt")
            }
            Self::CounterExhausted => formatter.write_str("authorization counter is exhausted"),
            Self::ConcurrentUpdate => {
                formatter.write_str("authorization epochs changed concurrently")
            }
        }
    }
}

impl Error for AuthorizationEpochError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScopeError> for AuthorizationEpochError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for AuthorizationEpochError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for AuthorizationEpochError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for AuthorizationEpochError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    #[test]
    fn trusted_claim_changes_advance_only_the_subject_and_publish_in_order() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let store = AuthorizationEpochStore::new(adapter, &tenant, &tenant, Durability::Memory)
                .expect("store");
            let first = SubjectId::parse("user-a").expect("subject");
            let second = SubjectId::parse("user-b").expect("subject");

            let event = store
                .trusted_claims_changed(&first)
                .await
                .expect("claim change");
            assert_eq!(event.sequence().get(), 1);
            assert_eq!(event.user_epoch().expect("user epoch").get(), 1);
            assert_eq!(
                store.epochs_for(&first).await.expect("first epochs"),
                AuthorizationEpochSnapshot {
                    environment: AuthorizationEpoch(0),
                    user: AuthorizationEpoch(1),
                }
            );
            assert_eq!(
                store.epochs_for(&second).await.expect("second epochs"),
                AuthorizationEpochSnapshot::default()
            );

            store
                .trusted_claims_changed(&first)
                .await
                .expect("second change");
            let events = store
                .invalidations_after(
                    InvalidationSequence::default(),
                    NonZeroUsize::new(10).expect("limit"),
                )
                .await
                .expect("invalidations");
            assert_eq!(
                events
                    .iter()
                    .map(|event| event.sequence().get())
                    .collect::<Vec<_>>(),
                vec![1, 2]
            );
        });
    }
}
