use std::{error::Error, fmt, sync::Arc};

use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    TenantKeyspace, WriteBatch,
};

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, CredentialDigest, IdentityProvider,
    IdentityRecordError, IdentityRevocationKind, NormalizedEmail, UserCredentialKind,
    UserCredentialRecord, UserIdentityRecord,
};

#[derive(Clone)]
pub struct IdentityStore {
    pub(crate) adapter: Arc<dyn KvAdapter>,
    pub(crate) tenant: TenantScope,
    pub(crate) keyspace: TenantKeyspace,
    pub(crate) durability: Durability,
}

impl fmt::Debug for IdentityStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentityStore")
            .field("tenant", &self.tenant)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl IdentityStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        requested_tenant: &TenantScope,
        trusted_tenant: &TenantScope,
        durability: Durability,
    ) -> Result<Self, IdentityStoreError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < durability {
            return Err(IdentityStoreError::UnsupportedDurability(durability));
        }
        Ok(Self {
            adapter,
            tenant: trusted_tenant.clone(),
            keyspace: TenantKeyspace::new(
                trusted_tenant.project_id().as_str().as_bytes(),
                trusted_tenant.environment_id().as_str().as_bytes(),
            )?,
            durability,
        })
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    pub async fn create_email_user(
        &self,
        user: &AppUserRecord,
        identity: &UserIdentityRecord,
        email: &NormalizedEmail,
    ) -> Result<(), IdentityStoreError> {
        if user.scope() != &self.tenant
            || identity.scope() != &self.tenant
            || identity.user_id() != user.id()
            || identity.provider() != &IdentityProvider::Email
            || identity.provider_subject() != email.as_str()
        {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }
        let user_key = self.keyspace.application_user_key(user.id().as_str())?;
        let identity_key = self
            .keyspace
            .application_identity_key(identity.id().as_str())?;
        let email_key = self.keyspace.normalized_email_owner_key(email.as_str())?;
        let user_email_key = self
            .keyspace
            .application_user_email_key(user.id().as_str())?;
        let conditions = [&user_key, &identity_key, &email_key, &user_email_key]
            .into_iter()
            .map(|key| KeyCondition::Missing { key: key.clone() })
            .collect();
        let mut batch = WriteBatch::with_capacity(4);
        batch.put(&user_key, serde_json::to_vec(user)?);
        batch.put(&identity_key, serde_json::to_vec(identity)?);
        batch.put(&email_key, user.id().as_str());
        batch.put(&user_email_key, email.as_str());
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
            CompareAndWriteResult::Conflict {
                failed_condition: 2,
                ..
            } => Err(IdentityStoreError::EmailAlreadyExists),
            CompareAndWriteResult::Conflict { .. } => Err(IdentityStoreError::RecordAlreadyExists),
        }
    }

    pub async fn create_email_signup(
        &self,
        user: &AppUserRecord,
        identity: &UserIdentityRecord,
        password: &UserCredentialRecord,
        verification: Option<(&UserCredentialRecord, &[u8])>,
        email: &NormalizedEmail,
    ) -> Result<(), IdentityStoreError> {
        if user.scope() != &self.tenant
            || identity.scope() != &self.tenant
            || password.scope() != &self.tenant
            || identity.user_id() != user.id()
            || password.user_id() != user.id()
            || identity.provider() != &IdentityProvider::Email
            || identity.provider_subject() != email.as_str()
            || password.kind() != UserCredentialKind::Password
            || verification.is_some_and(|(credential, digest)| {
                credential.scope() != &self.tenant
                    || credential.user_id() != user.id()
                    || credential.kind() != UserCredentialKind::EmailVerification
                    || digest.is_empty()
            })
        {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }

        let user_key = self.keyspace.application_user_key(user.id().as_str())?;
        let identity_key = self
            .keyspace
            .application_identity_key(identity.id().as_str())?;
        let password_key = self
            .keyspace
            .application_credential_key(password.id().as_str())?;
        let email_key = self.keyspace.normalized_email_owner_key(email.as_str())?;
        let password_owner_key = self
            .keyspace
            .application_user_credential_owner_key(user.id().as_str(), "password")?;
        let user_email_key = self
            .keyspace
            .application_user_email_key(user.id().as_str())?;
        let mut keys = vec![
            user_key.clone(),
            identity_key.clone(),
            password_key.clone(),
            email_key.clone(),
            password_owner_key.clone(),
            user_email_key.clone(),
        ];
        let mut batch = WriteBatch::with_capacity(7);
        batch.put(&user_key, serde_json::to_vec(user)?);
        batch.put(&identity_key, serde_json::to_vec(identity)?);
        batch.put(&password_key, serde_json::to_vec(password)?);
        batch.put(&email_key, user.id().as_str());
        batch.put(&password_owner_key, password.id().as_str());
        batch.put(&user_email_key, email.as_str());
        if let Some((credential, digest)) = verification {
            let credential_key = self
                .keyspace
                .application_credential_key(credential.id().as_str())?;
            let digest_key = self.keyspace.identity_token_digest_owner_key(digest)?;
            keys.push(credential_key.clone());
            keys.push(digest_key.clone());
            batch.put(&credential_key, serde_json::to_vec(credential)?);
            batch.put(&digest_key, credential.id().as_str());
        }
        let conditions = keys
            .into_iter()
            .map(|key| KeyCondition::Missing { key })
            .collect();
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
            CompareAndWriteResult::Conflict { .. }
                if self.adapter.get(&email_key).await?.is_some() =>
            {
                Err(IdentityStoreError::EmailAlreadyExists)
            }
            CompareAndWriteResult::Conflict { .. } => Err(IdentityStoreError::RecordAlreadyExists),
        }
    }

    pub async fn complete_email_verification(
        &self,
        token_digest: &[u8],
        now_unix_seconds: u64,
    ) -> Result<EmailVerificationOutcome, IdentityStoreError> {
        let digest_key = self
            .keyspace
            .identity_token_digest_owner_key(token_digest)?;
        let Some(credential_id) = self.adapter.get(&digest_key).await? else {
            return Ok(EmailVerificationOutcome::InvalidOrExpired);
        };
        let credential_key = self.keyspace.application_credential_key(&credential_id)?;
        let Some(credential_bytes) = self.adapter.get(&credential_key).await? else {
            return Err(IdentityStoreError::CorruptTokenOwner);
        };
        let credential: UserCredentialRecord = serde_json::from_slice(&credential_bytes)?;
        if credential.scope() != &self.tenant
            || credential.id().as_str().as_bytes() != credential_id
            || credential.kind() != UserCredentialKind::EmailVerification
            || credential.invalidated_at_unix_seconds().is_some()
            || credential
                .expires_at_unix_seconds()
                .is_none_or(|expires| now_unix_seconds >= expires)
        {
            return Ok(EmailVerificationOutcome::InvalidOrExpired);
        }
        let user_key = self
            .keyspace
            .application_user_key(credential.user_id().as_str())?;
        let user_bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::CorruptTokenOwner)?;
        let user: AppUserRecord = serde_json::from_slice(&user_bytes)?;
        if user.scope() != &self.tenant || user.id() != credential.user_id() {
            return Err(IdentityStoreError::CorruptTokenOwner);
        }
        let activated = user.with_status(AppUserStatus::Active, now_unix_seconds);
        let invalidated = credential.invalidate(now_unix_seconds);
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&user_key, serde_json::to_vec(&activated)?);
        batch.put(&credential_key, serde_json::to_vec(&invalidated)?);
        batch.delete(&digest_key);
        let outcome = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::ValueEquals {
                        key: user_key,
                        value: user_bytes,
                    },
                    KeyCondition::ValueEquals {
                        key: credential_key,
                        value: credential_bytes,
                    },
                    KeyCondition::ValueEquals {
                        key: digest_key,
                        value: credential_id,
                    },
                ],
                batch,
                durability: self.durability,
            })
            .await?;
        Ok(if outcome == CompareAndWriteResult::Applied {
            EmailVerificationOutcome::Verified
        } else {
            EmailVerificationOutcome::InvalidOrExpired
        })
    }

    pub async fn user_by_email(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<AppUserRecord>, IdentityStoreError> {
        let email_key = self.keyspace.normalized_email_owner_key(email.as_str())?;
        let Some(user_id) = self.adapter.get(&email_key).await? else {
            return Ok(None);
        };
        let user_key = self.keyspace.application_user_key(&user_id)?;
        let bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::CorruptEmailOwner)?;
        let user: AppUserRecord = serde_json::from_slice(&bytes)?;
        if user.scope() != &self.tenant || user.id().as_str().as_bytes() != user_id {
            return Err(IdentityStoreError::CorruptEmailOwner);
        }
        Ok(Some(user))
    }

    pub async fn email_for_user(
        &self,
        user_id: &AppUserId,
    ) -> Result<Option<NormalizedEmail>, IdentityStoreError> {
        let key = self.keyspace.application_user_email_key(user_id.as_str())?;
        self.adapter
            .get(&key)
            .await?
            .map(|bytes| {
                std::str::from_utf8(&bytes)
                    .map_err(|_| IdentityStoreError::CorruptEmailOwner)
                    .and_then(|email| {
                        NormalizedEmail::parse(email).map_err(IdentityStoreError::from)
                    })
            })
            .transpose()
    }

    pub(crate) async fn password_login_by_email(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<PasswordLoginRecord>, IdentityStoreError> {
        let Some(user) = self.user_by_email(email).await? else {
            return Ok(None);
        };
        self.password_login_for_user(user).await.map(Some)
    }

    pub(crate) async fn password_login_by_user_id(
        &self,
        user_id: &AppUserId,
    ) -> Result<Option<PasswordLoginRecord>, IdentityStoreError> {
        let user_key = self.keyspace.application_user_key(user_id.as_str())?;
        let Some(user_bytes) = self.adapter.get(&user_key).await? else {
            return Ok(None);
        };
        let user: AppUserRecord = serde_json::from_slice(&user_bytes)?;
        if user.scope() != &self.tenant || user.id() != user_id {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        }
        self.password_login_for_user_loaded(user, user_key, user_bytes)
            .await
            .map(Some)
    }

    async fn password_login_for_user(
        &self,
        user: AppUserRecord,
    ) -> Result<PasswordLoginRecord, IdentityStoreError> {
        let user_key = self.keyspace.application_user_key(user.id().as_str())?;
        let user_bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::CorruptCredentialOwner)?;
        self.password_login_for_user_loaded(user, user_key, user_bytes)
            .await
    }

    async fn password_login_for_user_loaded(
        &self,
        user: AppUserRecord,
        user_key: Vec<u8>,
        user_bytes: Vec<u8>,
    ) -> Result<PasswordLoginRecord, IdentityStoreError> {
        let owner_key = self
            .keyspace
            .application_user_credential_owner_key(user.id().as_str(), "password")?;
        let credential_id = self
            .adapter
            .get(&owner_key)
            .await?
            .ok_or(IdentityStoreError::CorruptCredentialOwner)?;
        let credential_key = self.keyspace.application_credential_key(&credential_id)?;
        let credential_bytes = self
            .adapter
            .get(&credential_key)
            .await?
            .ok_or(IdentityStoreError::CorruptCredentialOwner)?;
        let credential: UserCredentialRecord = serde_json::from_slice(&credential_bytes)?;
        if credential.scope() != &self.tenant
            || credential.user_id() != user.id()
            || credential.kind() != UserCredentialKind::Password
            || credential.id().as_str().as_bytes() != credential_id
        {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        }
        Ok(PasswordLoginRecord {
            user,
            credential,
            user_key,
            user_bytes,
            credential_key,
            credential_bytes,
        })
    }

    pub(crate) async fn upgrade_password_hash(
        &self,
        login: &PasswordLoginRecord,
        digest: crate::CredentialDigest,
    ) -> Result<(), IdentityStoreError> {
        let upgraded = login.credential.clone().with_digest(digest);
        let mut batch = WriteBatch::new();
        batch.put(&login.credential_key, serde_json::to_vec(&upgraded)?);
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key: login.credential_key.clone(),
                    value: login.credential_bytes.clone(),
                }],
                batch,
                durability: self.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::ConcurrentCredentialChange);
        }
        Ok(())
    }

    pub async fn create_password_recovery(
        &self,
        user: &AppUserRecord,
        credential: &UserCredentialRecord,
        token_digest: &[u8],
    ) -> Result<(), IdentityStoreError> {
        if user.scope() != &self.tenant
            || credential.scope() != &self.tenant
            || credential.user_id() != user.id()
            || credential.kind() != UserCredentialKind::PasswordRecovery
            || token_digest.is_empty()
        {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }
        let credential_key = self
            .keyspace
            .application_credential_key(credential.id().as_str())?;
        let digest_key = self
            .keyspace
            .identity_token_digest_owner_key(token_digest)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&credential_key, serde_json::to_vec(credential)?);
        batch.put(&digest_key, credential.id().as_str());
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::Missing {
                        key: credential_key,
                    },
                    KeyCondition::Missing { key: digest_key },
                ],
                batch,
                durability: self.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::RecordAlreadyExists);
        }
        Ok(())
    }

    pub async fn reset_password_with_token(
        &self,
        token_digest: &[u8],
        password_digest: CredentialDigest,
        now_unix_seconds: u64,
    ) -> Result<PasswordResetOutcome, IdentityStoreError> {
        let digest_key = self
            .keyspace
            .identity_token_digest_owner_key(token_digest)?;
        let Some(recovery_id) = self.adapter.get(&digest_key).await? else {
            return Ok(PasswordResetOutcome::InvalidOrExpired);
        };
        let recovery_key = self.keyspace.application_credential_key(&recovery_id)?;
        let recovery_bytes = self
            .adapter
            .get(&recovery_key)
            .await?
            .ok_or(IdentityStoreError::CorruptTokenOwner)?;
        let recovery: UserCredentialRecord = serde_json::from_slice(&recovery_bytes)?;
        if recovery.scope() != &self.tenant
            || recovery.kind() != UserCredentialKind::PasswordRecovery
            || recovery.invalidated_at_unix_seconds().is_some()
            || recovery
                .expires_at_unix_seconds()
                .is_none_or(|expires| now_unix_seconds >= expires)
        {
            return Ok(PasswordResetOutcome::InvalidOrExpired);
        }
        let Some(login) = self.password_login_by_user_id(recovery.user_id()).await? else {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        };
        self.commit_password_replacement(
            &login,
            password_digest,
            Some(PreparedRecoveryConsumption {
                record: recovery,
                key: recovery_key,
                bytes: recovery_bytes,
                digest_key,
                owner: recovery_id,
            }),
            now_unix_seconds,
        )
        .await
    }

    pub(crate) async fn replace_password_and_revoke_sessions(
        &self,
        login: &PasswordLoginRecord,
        password_digest: CredentialDigest,
        now_unix_seconds: u64,
    ) -> Result<PasswordResetOutcome, IdentityStoreError> {
        self.commit_password_replacement(login, password_digest, None, now_unix_seconds)
            .await
    }

    async fn commit_password_replacement(
        &self,
        login: &PasswordLoginRecord,
        password_digest: CredentialDigest,
        recovery: Option<PreparedRecoveryConsumption>,
        now_unix_seconds: u64,
    ) -> Result<PasswordResetOutcome, IdentityStoreError> {
        let updated_user = login.user.clone().revoke_all_sessions(now_unix_seconds)?;
        let updated_password = login.credential.clone().with_digest(password_digest);
        for _ in 0..32 {
            let prepared = self.prepare_revocation().await?;
            let event = prepared.event(
                self.tenant.clone(),
                IdentityRevocationKind::PasswordChanged,
                login.user.id().clone(),
                None,
                updated_user.session_epoch(),
                now_unix_seconds,
            );
            let mut conditions = vec![
                KeyCondition::ValueEquals {
                    key: login.user_key.clone(),
                    value: login.user_bytes.clone(),
                },
                KeyCondition::ValueEquals {
                    key: login.credential_key.clone(),
                    value: login.credential_bytes.clone(),
                },
            ];
            conditions.extend(prepared.conditions());
            let mut batch = prepared.batch(&event)?;
            batch.put(&login.user_key, serde_json::to_vec(&updated_user)?);
            batch.put(
                &login.credential_key,
                serde_json::to_vec(&updated_password)?,
            );
            if let Some(recovery) = &recovery {
                conditions.push(KeyCondition::ValueEquals {
                    key: recovery.key.clone(),
                    value: recovery.bytes.clone(),
                });
                conditions.push(KeyCondition::ValueEquals {
                    key: recovery.digest_key.clone(),
                    value: recovery.owner.clone(),
                });
                batch.put(
                    &recovery.key,
                    serde_json::to_vec(&recovery.record.clone().invalidate(now_unix_seconds))?,
                );
                batch.delete(&recovery.digest_key);
            }
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions,
                    batch,
                    durability: self.durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(PasswordResetOutcome::Changed);
            }
            if self.adapter.get(&login.user_key).await?.as_deref()
                != Some(login.user_bytes.as_slice())
                || self.adapter.get(&login.credential_key).await?.as_deref()
                    != Some(login.credential_bytes.as_slice())
            {
                return Ok(PasswordResetOutcome::InvalidOrExpired);
            }
            if let Some(recovery) = &recovery
                && (self.adapter.get(&recovery.key).await?.as_deref()
                    != Some(recovery.bytes.as_slice())
                    || self.adapter.get(&recovery.digest_key).await?.as_deref()
                        != Some(recovery.owner.as_slice()))
            {
                return Ok(PasswordResetOutcome::InvalidOrExpired);
            }
        }
        Err(IdentityStoreError::ConcurrentIdentityChange)
    }
}

pub(crate) struct PasswordLoginRecord {
    pub(crate) user: AppUserRecord,
    pub(crate) credential: UserCredentialRecord,
    user_key: Vec<u8>,
    user_bytes: Vec<u8>,
    credential_key: Vec<u8>,
    credential_bytes: Vec<u8>,
}

struct PreparedRecoveryConsumption {
    record: UserCredentialRecord,
    key: Vec<u8>,
    bytes: Vec<u8>,
    digest_key: Vec<u8>,
    owner: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordResetOutcome {
    Changed,
    InvalidOrExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmailVerificationOutcome {
    Verified,
    InvalidOrExpired,
}

#[derive(Debug)]
pub enum IdentityStoreError {
    Scope(ScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Record(IdentityRecordError),
    Json(serde_json::Error),
    UnsupportedDurability(Durability),
    RecordScopeMismatch,
    RecordAlreadyExists,
    EmailAlreadyExists,
    CorruptEmailOwner,
    CorruptTokenOwner,
    CorruptCredentialOwner,
    ConcurrentCredentialChange,
    UserNotFound,
    SessionNotFound,
    SessionOwnerMismatch,
    InvalidUserStatusTransition,
    RevocationSequenceExhausted,
    ConcurrentIdentityChange,
    CorruptSession,
    CorruptRevocationEvent,
    InvalidProjectCredential,
    ProjectCredentialNotFound,
    ConcurrentProjectCredentialChange,
    InvalidAdminQuery,
}

impl fmt::Display for IdentityStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Record(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::RecordScopeMismatch => formatter.write_str("identity record scope mismatch"),
            Self::RecordAlreadyExists => formatter.write_str("identity record already exists"),
            Self::EmailAlreadyExists => formatter.write_str("normalized email already exists"),
            Self::CorruptEmailOwner => formatter.write_str("normalized email owner is corrupt"),
            Self::CorruptTokenOwner => formatter.write_str("identity token owner is corrupt"),
            Self::CorruptCredentialOwner => formatter.write_str("user credential owner is corrupt"),
            Self::ConcurrentCredentialChange => {
                formatter.write_str("user credential changed concurrently")
            }
            Self::UserNotFound => formatter.write_str("application user was not found"),
            Self::SessionNotFound => formatter.write_str("application session was not found"),
            Self::SessionOwnerMismatch => {
                formatter.write_str("application session does not belong to the user")
            }
            Self::InvalidUserStatusTransition => {
                formatter.write_str("application user status transition is not allowed")
            }
            Self::RevocationSequenceExhausted => {
                formatter.write_str("identity revocation sequence is exhausted")
            }
            Self::ConcurrentIdentityChange => {
                formatter.write_str("identity record changed concurrently")
            }
            Self::CorruptSession => formatter.write_str("application session is corrupt"),
            Self::CorruptRevocationEvent => {
                formatter.write_str("identity revocation event is corrupt")
            }
            Self::InvalidProjectCredential => formatter.write_str("project credential is invalid"),
            Self::ProjectCredentialNotFound => {
                formatter.write_str("project credential was not found")
            }
            Self::ConcurrentProjectCredentialChange => {
                formatter.write_str("project credential changed concurrently")
            }
            Self::InvalidAdminQuery => {
                formatter.write_str("application-user admin query is invalid")
            }
        }
    }
}

impl Error for IdentityStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Record(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScopeError> for IdentityStoreError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for IdentityStoreError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for IdentityStoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<IdentityRecordError> for IdentityStoreError {
    fn from(error: IdentityRecordError) -> Self {
        Self::Record(error)
    }
}

impl From<serde_json::Error> for IdentityStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;
    use serde_json::json;

    use super::*;
    use crate::{
        AppUserId, AppUserStatus, TrustedAppMetadata, UserIdentityId, UserProfileMetadata,
    };

    #[test]
    fn normalized_email_is_unique_per_tenant_and_independent_across_projects() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let first_tenant = tenant("prj_abcdefgh");
            let second_tenant = tenant("prj_ijklmnop");
            let first = IdentityStore::new(
                adapter.clone(),
                &first_tenant,
                &first_tenant,
                Durability::Memory,
            )
            .expect("store");
            let second =
                IdentityStore::new(adapter, &second_tenant, &second_tenant, Durability::Memory)
                    .expect("store");
            let email = NormalizedEmail::parse("Person@Example.COM").expect("email");

            let (user, identity) = records(&first_tenant, "usr_first", "idn_first", &email);
            first
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create first");
            let (duplicate, duplicate_identity) =
                records(&first_tenant, "usr_second", "idn_second", &email);
            assert!(matches!(
                first
                    .create_email_user(&duplicate, &duplicate_identity, &email)
                    .await,
                Err(IdentityStoreError::EmailAlreadyExists)
            ));

            let (other_user, other_identity) =
                records(&second_tenant, "usr_third", "idn_third", &email);
            second
                .create_email_user(&other_user, &other_identity, &email)
                .await
                .expect("other project");
            assert_eq!(
                first
                    .user_by_email(&email)
                    .await
                    .expect("lookup")
                    .expect("user")
                    .id()
                    .as_str(),
                "usr_first"
            );
        });
    }

    fn records(
        tenant: &TenantScope,
        user_id: &str,
        identity_id: &str,
        email: &NormalizedEmail,
    ) -> (AppUserRecord, UserIdentityRecord) {
        let user_id = AppUserId::parse(user_id).expect("user");
        (
            AppUserRecord::new(
                tenant.clone(),
                user_id.clone(),
                AppUserStatus::Active,
                TrustedAppMetadata::new(json!({})).expect("trusted"),
                UserProfileMetadata::new(json!({})).expect("profile"),
                1,
            ),
            UserIdentityRecord::new(
                tenant.clone(),
                UserIdentityId::parse(identity_id).expect("identity"),
                user_id,
                IdentityProvider::Email,
                email.as_str(),
                1,
            )
            .expect("identity"),
        )
    }

    fn tenant(project: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
