use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCodecError, KeyCondition, KvAdapter,
    StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::VerifiedInternalRequest;

const NONCE_RETENTION_SECONDS: u64 = 5 * 60;
const IDEMPOTENCY_RETENTION_SECONDS: u64 = 24 * 60 * 60;
const MAX_CONFLICT_RETRIES: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardDecision {
    Fresh,
    Duplicate,
}

#[async_trait]
pub trait InternalReplayGuard: Send + Sync {
    async fn claim(
        &self,
        request: &VerifiedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<GuardDecision, RocksInternalReplayGuardError>;
}

/// Test-only or explicitly non-authoritative guard. Production service graphs
/// use `RocksInternalReplayGuard` so restarts cannot reset replay state.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopReplayGuard;

#[async_trait]
impl InternalReplayGuard for NoopReplayGuard {
    async fn claim(
        &self,
        _request: &VerifiedInternalRequest,
        _now_unix_seconds: u64,
    ) -> Result<GuardDecision, RocksInternalReplayGuardError> {
        Ok(GuardDecision::Fresh)
    }
}

#[derive(Clone)]
pub struct RocksInternalReplayGuard {
    adapter: Arc<dyn KvAdapter>,
    tenant: TenantScope,
    keyspace: TenantKeyspace,
}

impl RocksInternalReplayGuard {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
    ) -> Result<Self, RocksInternalReplayGuardError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < Durability::Sync {
            return Err(RocksInternalReplayGuardError::InsufficientDurability);
        }
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str(),
            trusted_tenant.environment_id().as_str(),
        )?;
        Ok(Self {
            adapter,
            tenant: trusted_tenant.clone(),
            keyspace,
        })
    }

    async fn claim_once(
        &self,
        request: &VerifiedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<Option<GuardDecision>, RocksInternalReplayGuardError> {
        if request.tenant != self.tenant {
            return Err(RocksInternalReplayGuardError::TenantMismatch);
        }
        let nonce_key = self
            .keyspace
            .internal_rpc_nonce_key(request.caller.as_str(), &request.nonce)?;
        let idempotency_key = self
            .keyspace
            .internal_rpc_idempotency_key(request.caller.as_str(), &request.idempotency_key)?;
        let nonce_existing = self.adapter.get(&nonce_key).await?;
        if let Some(bytes) = &nonce_existing {
            let record: GuardRecord = serde_json::from_slice(bytes)?;
            if record.expires_at_unix_seconds > now_unix_seconds {
                return Err(RocksInternalReplayGuardError::Replay);
            }
        }
        let idempotency_existing = self.adapter.get(&idempotency_key).await?;
        let decision = if let Some(bytes) = &idempotency_existing {
            let record: GuardRecord = serde_json::from_slice(bytes)?;
            if record.expires_at_unix_seconds > now_unix_seconds {
                if record.operation_digest != request.operation_digest {
                    return Err(RocksInternalReplayGuardError::IdempotencyMismatch);
                }
                GuardDecision::Duplicate
            } else {
                GuardDecision::Fresh
            }
        } else {
            GuardDecision::Fresh
        };

        let nonce_record = GuardRecord {
            operation_digest: request.operation_digest,
            expires_at_unix_seconds: now_unix_seconds.saturating_add(NONCE_RETENTION_SECONDS),
        };
        let idempotency_record = GuardRecord {
            operation_digest: request.operation_digest,
            expires_at_unix_seconds: now_unix_seconds.saturating_add(IDEMPOTENCY_RETENTION_SECONDS),
        };
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&nonce_key, serde_json::to_vec(&nonce_record)?);
        if decision == GuardDecision::Fresh {
            batch.put(&idempotency_key, serde_json::to_vec(&idempotency_record)?);
        }
        let conditions = vec![
            condition_for(nonce_key, nonce_existing),
            condition_for(idempotency_key, idempotency_existing),
        ];
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: Durability::Sync,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(Some(decision)),
            CompareAndWriteResult::Conflict { .. } => Ok(None),
        }
    }
}

#[async_trait]
impl InternalReplayGuard for RocksInternalReplayGuard {
    async fn claim(
        &self,
        request: &VerifiedInternalRequest,
        now_unix_seconds: u64,
    ) -> Result<GuardDecision, RocksInternalReplayGuardError> {
        for _ in 0..MAX_CONFLICT_RETRIES {
            if let Some(decision) = self.claim_once(request, now_unix_seconds).await? {
                return Ok(decision);
            }
        }
        Err(RocksInternalReplayGuardError::ConcurrentConflict)
    }
}

fn condition_for(key: Vec<u8>, existing: Option<Vec<u8>>) -> KeyCondition {
    existing.map_or(KeyCondition::Missing { key: key.clone() }, |value| {
        KeyCondition::ValueEquals { key, value }
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GuardRecord {
    operation_digest: [u8; 32],
    expires_at_unix_seconds: u64,
}

#[derive(Debug)]
pub enum RocksInternalReplayGuardError {
    TenantMismatch,
    InsufficientDurability,
    Replay,
    IdempotencyMismatch,
    ConcurrentConflict,
    Scope(ScopeError),
    Key(KeyCodecError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl RocksInternalReplayGuardError {
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Storage(_) | Self::ConcurrentConflict)
    }
}

impl fmt::Display for RocksInternalReplayGuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TenantMismatch | Self::Scope(_) => "internal RPC tenant binding failed",
            Self::InsufficientDurability => "internal RPC guard requires synchronous durability",
            Self::Replay => "internal RPC nonce was already used",
            Self::IdempotencyMismatch => {
                "internal RPC idempotency key was reused for another operation"
            }
            Self::ConcurrentConflict => "internal RPC guard was concurrently modified",
            Self::Key(_) => "internal RPC guard key is invalid",
            Self::Storage(_) => "internal RPC guard storage is unavailable",
            Self::Json(_) => "internal RPC guard record is corrupt",
        })
    }
}

impl Error for RocksInternalReplayGuardError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::Key(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::TenantMismatch
            | Self::InsufficientDurability
            | Self::Replay
            | Self::IdempotencyMismatch
            | Self::ConcurrentConflict => None,
        }
    }
}

impl From<ScopeError> for RocksInternalReplayGuardError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<KeyCodecError> for RocksInternalReplayGuardError {
    fn from(error: KeyCodecError) -> Self {
        Self::Key(error)
    }
}

impl From<StorageError> for RocksInternalReplayGuardError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for RocksInternalReplayGuardError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
