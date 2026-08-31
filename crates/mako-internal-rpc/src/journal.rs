use std::{error::Error, fmt, sync::Arc};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, ConditionalPut, Durability, KeyCodecError, KeyCondition,
    KvAdapter, StorageError, TenantKeyspace, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{DeploymentKey, VerifiedInternalRequest};

const RESPONSE_RETENTION_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseJournalLookup {
    Missing,
    Replay(Vec<u8>),
    Conflict,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponseJournalStoreOutcome {
    Stored,
    Replayed(Vec<u8>),
}

pub enum PreparedResponseJournal {
    Fresh(ConditionalPut),
    Replay(Vec<u8>),
    Conflict,
    Expired,
}

impl fmt::Debug for PreparedResponseJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Fresh(_) => "PreparedResponseJournal::Fresh([REDACTED])",
            Self::Replay(_) => "PreparedResponseJournal::Replay([REDACTED])",
            Self::Conflict => "PreparedResponseJournal::Conflict",
            Self::Expired => "PreparedResponseJournal::Expired",
        })
    }
}

#[derive(Clone)]
pub struct EncryptedResponseJournal {
    adapter: Arc<dyn KvAdapter>,
    tenant: TenantScope,
    keyspace: TenantKeyspace,
    encryption_key: [u8; 32],
}

impl EncryptedResponseJournal {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
        deployment_key: &DeploymentKey,
    ) -> Result<Self, ResponseJournalError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < Durability::Sync {
            return Err(ResponseJournalError::InsufficientDurability);
        }
        Ok(Self {
            adapter,
            tenant: trusted_tenant.clone(),
            keyspace: TenantKeyspace::new(
                trusted_tenant.project_id().as_str(),
                trusted_tenant.environment_id().as_str(),
            )?,
            encryption_key: deployment_key.response_encryption_key(),
        })
    }

    pub async fn lookup(
        &self,
        request: &VerifiedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<ResponseJournalLookup, ResponseJournalError> {
        self.ensure_tenant(request)?;
        let key = self.key(request)?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(ResponseJournalLookup::Missing);
        };
        let record: EncryptedResponseRecord = serde_json::from_slice(&bytes)?;
        if record.operation_digest != request.operation_digest {
            return Ok(ResponseJournalLookup::Conflict);
        }
        if record.expires_at_unix_seconds <= now_unix_seconds {
            return Ok(ResponseJournalLookup::Expired);
        }
        let plaintext = self.decrypt(request, &record)?;
        Ok(ResponseJournalLookup::Replay(plaintext))
    }

    pub async fn store(
        &self,
        request: &VerifiedInternalRequest,
        successful_response: &[u8],
        now_unix_seconds: u64,
    ) -> Result<ResponseJournalStoreOutcome, ResponseJournalError> {
        self.ensure_tenant(request)?;
        if successful_response.is_empty() || successful_response.len() > 1024 * 1024 {
            return Err(ResponseJournalError::InvalidResponse);
        }
        // An expired record is not an answer -- it is what an answer decays
        // into once nothing may replay it. Refusing to overwrite one made a
        // fixed idempotency key unusable forever the day after its first use:
        // the guard's fresh claim would execute, and the store would then
        // refuse to record the new answer because a stale one was in the way.
        // The stale bytes become the replace condition instead, so two
        // concurrent re-executions still resolve to a single recorded answer.
        let key = self.key(request)?;
        let expired_record = match self.lookup(request, now_unix_seconds).await? {
            ResponseJournalLookup::Replay(response) => {
                return Ok(ResponseJournalStoreOutcome::Replayed(response));
            }
            ResponseJournalLookup::Conflict => return Err(ResponseJournalError::Conflict),
            ResponseJournalLookup::Expired => self.adapter.get(&key).await?,
            ResponseJournalLookup::Missing => None,
        };

        let expires_at_unix_seconds = now_unix_seconds.saturating_add(RESPONSE_RETENTION_SECONDS);
        let record = self.encrypt(request, successful_response, expires_at_unix_seconds)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        let condition = match expired_record {
            Some(previous) => KeyCondition::ValueEquals {
                key,
                value: previous,
            },
            None => KeyCondition::Missing { key },
        };
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![condition],
                batch,
                durability: Durability::Sync,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(ResponseJournalStoreOutcome::Stored),
            CompareAndWriteResult::Conflict { .. } => {
                match self.lookup(request, now_unix_seconds).await? {
                    ResponseJournalLookup::Replay(response) => {
                        Ok(ResponseJournalStoreOutcome::Replayed(response))
                    }
                    ResponseJournalLookup::Conflict => Err(ResponseJournalError::Conflict),
                    ResponseJournalLookup::Expired => Err(ResponseJournalError::Expired),
                    ResponseJournalLookup::Missing => Err(ResponseJournalError::ConcurrentConflict),
                }
            }
        }
    }

    pub async fn prepare_atomic(
        &self,
        request: &VerifiedInternalRequest,
        successful_response: &[u8],
        now_unix_seconds: u64,
    ) -> Result<PreparedResponseJournal, ResponseJournalError> {
        self.ensure_tenant(request)?;
        if successful_response.is_empty() || successful_response.len() > 1024 * 1024 {
            return Err(ResponseJournalError::InvalidResponse);
        }
        match self.lookup(request, now_unix_seconds).await? {
            ResponseJournalLookup::Replay(response) => {
                return Ok(PreparedResponseJournal::Replay(response));
            }
            ResponseJournalLookup::Conflict => return Ok(PreparedResponseJournal::Conflict),
            ResponseJournalLookup::Expired => return Ok(PreparedResponseJournal::Expired),
            ResponseJournalLookup::Missing => {}
        }
        let expires_at_unix_seconds = now_unix_seconds.saturating_add(RESPONSE_RETENTION_SECONDS);
        let record = self.encrypt(request, successful_response, expires_at_unix_seconds)?;
        Ok(PreparedResponseJournal::Fresh(ConditionalPut::new(
            self.key(request)?,
            serde_json::to_vec(&record)?,
        )))
    }

    /// Removes at most the addressed expired entry. Callers can schedule this
    /// with a bounded list of observed idempotency keys; it never scans across
    /// tenants or decrypts a response during cleanup.
    pub async fn cleanup_expired(
        &self,
        request: &VerifiedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<bool, ResponseJournalError> {
        self.ensure_tenant(request)?;
        let key = self.key(request)?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(false);
        };
        let record: EncryptedResponseRecord = serde_json::from_slice(&bytes)?;
        if record.expires_at_unix_seconds > now_unix_seconds {
            return Ok(false);
        }
        let mut batch = WriteBatch::new();
        batch.delete(&key);
        Ok(matches!(
            self.adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals { key, value: bytes }],
                    batch,
                    durability: Durability::Sync,
                })
                .await?,
            CompareAndWriteResult::Applied
        ))
    }

    fn ensure_tenant(&self, request: &VerifiedInternalRequest) -> Result<(), ResponseJournalError> {
        if request.tenant != self.tenant {
            return Err(ResponseJournalError::TenantMismatch);
        }
        Ok(())
    }

    fn key(&self, request: &VerifiedInternalRequest) -> Result<Vec<u8>, ResponseJournalError> {
        self.keyspace
            .internal_rpc_response_key(request.caller.as_str(), &request.idempotency_key)
            .map_err(Into::into)
    }

    fn encrypt(
        &self,
        request: &VerifiedInternalRequest,
        plaintext: &[u8],
        expires_at_unix_seconds: u64,
    ) -> Result<EncryptedResponseRecord, ResponseJournalError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let aad = associated_data(request, expires_at_unix_seconds);
        let cipher = XChaCha20Poly1305::new((&self.encryption_key).into());
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| ResponseJournalError::Encryption)?;
        Ok(EncryptedResponseRecord {
            operation_digest: request.operation_digest,
            expires_at_unix_seconds,
            nonce,
            ciphertext,
        })
    }

    fn decrypt(
        &self,
        request: &VerifiedInternalRequest,
        record: &EncryptedResponseRecord,
    ) -> Result<Vec<u8>, ResponseJournalError> {
        let aad = associated_data(request, record.expires_at_unix_seconds);
        XChaCha20Poly1305::new((&self.encryption_key).into())
            .decrypt(
                XNonce::from_slice(&record.nonce),
                Payload {
                    msg: &record.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| ResponseJournalError::Corrupt)
    }
}

impl fmt::Debug for EncryptedResponseJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedResponseJournal")
            .field("tenant", &self.tenant)
            .field("encryption_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

fn associated_data(request: &VerifiedInternalRequest, expires_at_unix_seconds: u64) -> Vec<u8> {
    let mut bytes = b"mako/internal-rpc/success-response/v1".to_vec();
    for part in [
        request.tenant.project_id().as_str().as_bytes(),
        request.tenant.environment_id().as_str().as_bytes(),
        request.caller.as_str().as_bytes(),
        request.idempotency_key.as_bytes(),
        request.operation_digest.as_slice(),
        &expires_at_unix_seconds.to_be_bytes(),
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    bytes
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EncryptedResponseRecord {
    operation_digest: [u8; 32],
    expires_at_unix_seconds: u64,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

#[derive(Debug)]
pub enum ResponseJournalError {
    TenantMismatch,
    InsufficientDurability,
    InvalidResponse,
    Conflict,
    Expired,
    ConcurrentConflict,
    Encryption,
    Corrupt,
    Scope(ScopeError),
    Key(KeyCodecError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for ResponseJournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TenantMismatch | Self::Scope(_) => "response journal tenant binding failed",
            Self::InsufficientDurability => "response journal requires synchronous durability",
            Self::InvalidResponse => "response journal payload is invalid",
            Self::Conflict => "response journal idempotency key conflicts",
            Self::Expired => "response journal entry expired",
            Self::ConcurrentConflict => "response journal was concurrently modified",
            Self::Encryption => "response journal encryption failed",
            Self::Corrupt => "response journal entry is corrupt",
            Self::Key(_) => "response journal key is invalid",
            Self::Storage(_) => "response journal storage is unavailable",
            Self::Json(_) => "response journal record is corrupt",
        })
    }
}

impl Error for ResponseJournalError {}

impl From<ScopeError> for ResponseJournalError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<KeyCodecError> for ResponseJournalError {
    fn from(error: KeyCodecError) -> Self {
        Self::Key(error)
    }
}

impl From<StorageError> for ResponseJournalError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for ResponseJournalError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
