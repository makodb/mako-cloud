use std::{error::Error, fmt, sync::Arc};

use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    TenantKeyspace, WriteBatch,
};

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, CredentialDigest, IdentityProvider,
    IdentityRecordError, IdentityRevocationKind, NormalizedEmail, ProviderName, TrustedAppMetadata,
    UserCredentialKind, UserCredentialRecord, UserIdentityRecord, UserProfileMetadata,
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

    /// Creates a user whose first identity is an external provider sign-in
    /// with a verified email. The provider subject and the normalized email
    /// each become unique owner indexes in the same atomic write.
    pub async fn create_provider_user(
        &self,
        user: &AppUserRecord,
        identity: &UserIdentityRecord,
        email: &NormalizedEmail,
    ) -> Result<(), IdentityStoreError> {
        if user.scope() != &self.tenant
            || identity.scope() != &self.tenant
            || identity.user_id() != user.id()
        {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }
        let provider = provider_name_of(identity)?;
        let user_key = self.keyspace.application_user_key(user.id().as_str())?;
        let identity_key = self
            .keyspace
            .application_identity_key(identity.id().as_str())?;
        let email_key = self.keyspace.normalized_email_owner_key(email.as_str())?;
        let user_email_key = self
            .keyspace
            .application_user_email_key(user.id().as_str())?;
        let provider_key = self
            .keyspace
            .provider_identity_owner_key(provider.as_str(), identity.provider_subject())?;
        let conditions = [
            &user_key,
            &identity_key,
            &email_key,
            &user_email_key,
            &provider_key,
        ]
        .into_iter()
        .map(|key| KeyCondition::Missing { key: key.clone() })
        .collect();
        let mut batch = WriteBatch::with_capacity(5);
        batch.put(&user_key, serde_json::to_vec(user)?);
        batch.put(&identity_key, serde_json::to_vec(identity)?);
        batch.put(&email_key, user.id().as_str());
        batch.put(&user_email_key, email.as_str());
        batch.put(&provider_key, user.id().as_str());
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
            CompareAndWriteResult::Conflict {
                failed_condition: 4,
                ..
            } => Err(IdentityStoreError::ProviderIdentityAlreadyLinked),
            CompareAndWriteResult::Conflict { .. } => Err(IdentityStoreError::RecordAlreadyExists),
        }
    }

    /// Links an external provider identity to an existing user, after the
    /// caller has matched the provider's verified email to that user. The
    /// user must be active or pending verification; the write is fenced on
    /// the user record so a concurrent disable or delete cannot be raced.
    pub async fn link_provider_identity(
        &self,
        user_id: &AppUserId,
        identity: &UserIdentityRecord,
    ) -> Result<(), IdentityStoreError> {
        if identity.scope() != &self.tenant || identity.user_id() != user_id {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }
        let provider = provider_name_of(identity)?;
        let user_key = self.keyspace.application_user_key(user_id.as_str())?;
        let user_bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::UserNotFound)?;
        let user: AppUserRecord = serde_json::from_slice(&user_bytes)?;
        if user.scope() != &self.tenant || user.id() != user_id {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        }
        if !matches!(
            user.status(),
            AppUserStatus::Active | AppUserStatus::PendingVerification
        ) {
            return Err(IdentityStoreError::InvalidUserStatusTransition);
        }
        let identity_key = self
            .keyspace
            .application_identity_key(identity.id().as_str())?;
        let provider_key = self
            .keyspace
            .provider_identity_owner_key(provider.as_str(), identity.provider_subject())?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&identity_key, serde_json::to_vec(identity)?);
        batch.put(&provider_key, user_id.as_str());
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::ValueEquals {
                        key: user_key,
                        value: user_bytes,
                    },
                    KeyCondition::Missing { key: identity_key },
                    KeyCondition::Missing { key: provider_key },
                ],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict {
                failed_condition: 0,
                ..
            } => Err(IdentityStoreError::ConcurrentIdentityChange),
            CompareAndWriteResult::Conflict {
                failed_condition: 2,
                ..
            } => Err(IdentityStoreError::ProviderIdentityAlreadyLinked),
            CompareAndWriteResult::Conflict { .. } => Err(IdentityStoreError::RecordAlreadyExists),
        }
    }

    pub async fn user_by_provider(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<AppUserRecord>, IdentityStoreError> {
        let provider = ProviderName::parse(provider)?;
        let provider_key = self
            .keyspace
            .provider_identity_owner_key(provider.as_str(), subject)?;
        let Some(user_id) = self.adapter.get(&provider_key).await? else {
            return Ok(None);
        };
        let user_key = self.keyspace.application_user_key(&user_id)?;
        let bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::CorruptProviderOwner)?;
        let user: AppUserRecord = serde_json::from_slice(&bytes)?;
        if user.scope() != &self.tenant || user.id().as_str().as_bytes() != user_id {
            return Err(IdentityStoreError::CorruptProviderOwner);
        }
        Ok(Some(user))
    }

    /// Stores an unspent magic-link credential and the owner index from the
    /// token digest to it. The credential must expire; an unbounded link is
    /// refused rather than stored.
    pub async fn create_magic_link(
        &self,
        user: &AppUserRecord,
        credential: &UserCredentialRecord,
        token_digest: &[u8],
    ) -> Result<(), IdentityStoreError> {
        if user.scope() != &self.tenant
            || credential.scope() != &self.tenant
            || credential.user_id() != user.id()
            || credential.kind() != UserCredentialKind::MagicLink
            || credential.expires_at_unix_seconds().is_none()
            || credential.invalidated_at_unix_seconds().is_some()
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

    /// Spends a magic link. The credential is invalidated and its digest
    /// index deleted in one compare-and-write, so a token is redeemed at most
    /// once: a repeat sees no digest owner (`Unknown`) and a concurrent loser
    /// sees `AlreadyUsed`. Redeeming a link proves the email, so a user still
    /// pending verification is activated in the same write. A disabled or
    /// deleted user's link is spent without being redeemed and reported as
    /// `Unknown`, which never yields a user id.
    pub async fn redeem_magic_link(
        &self,
        token_digest: &[u8],
        now_unix_seconds: u64,
    ) -> Result<MagicLinkOutcome, IdentityStoreError> {
        let digest_key = self
            .keyspace
            .identity_token_digest_owner_key(token_digest)?;
        let Some(credential_id) = self.adapter.get(&digest_key).await? else {
            return Ok(MagicLinkOutcome::Unknown);
        };
        let credential_key = self.keyspace.application_credential_key(&credential_id)?;
        let Some(credential_bytes) = self.adapter.get(&credential_key).await? else {
            return Err(IdentityStoreError::CorruptTokenOwner);
        };
        let credential: UserCredentialRecord = serde_json::from_slice(&credential_bytes)?;
        if credential.scope() != &self.tenant
            || credential.id().as_str().as_bytes() != credential_id
            || credential.kind() != UserCredentialKind::MagicLink
        {
            return Ok(MagicLinkOutcome::Unknown);
        }
        if credential.invalidated_at_unix_seconds().is_some() {
            return Ok(MagicLinkOutcome::AlreadyUsed);
        }
        if credential
            .expires_at_unix_seconds()
            .is_none_or(|expires| now_unix_seconds >= expires)
        {
            return Ok(MagicLinkOutcome::Expired);
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
        let redeemable = match user.status() {
            AppUserStatus::PendingVerification | AppUserStatus::Active => true,
            AppUserStatus::Disabled | AppUserStatus::Deleted => false,
        };
        let invalidated = credential.invalidate(now_unix_seconds);
        let mut batch = WriteBatch::with_capacity(3);
        if user.status() == AppUserStatus::PendingVerification {
            let activated = user
                .clone()
                .with_status(AppUserStatus::Active, now_unix_seconds);
            batch.put(&user_key, serde_json::to_vec(&activated)?);
        }
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
        Ok(match outcome {
            CompareAndWriteResult::Applied if redeemable => MagicLinkOutcome::Redeemed {
                user_id: user.id().clone(),
            },
            CompareAndWriteResult::Applied => MagicLinkOutcome::Unknown,
            CompareAndWriteResult::Conflict { .. } => MagicLinkOutcome::AlreadyUsed,
        })
    }

    /// Loads a user for a metadata replacement. The returned update remembers
    /// exactly what it read, so [`AppUserMetadataUpdate::commit`] applies only
    /// if no other writer changed the user in between. Every writer of
    /// administrator-controlled metadata -- the management admin operation and
    /// the service-credential route alike -- goes through this one path.
    pub async fn begin_user_metadata_update(
        &self,
        user_id: &AppUserId,
    ) -> Result<AppUserMetadataUpdate<'_>, IdentityStoreError> {
        let key = self.keyspace.application_user_key(user_id.as_str())?;
        let current = self
            .adapter
            .get(&key)
            .await?
            .ok_or(IdentityStoreError::UserNotFound)?;
        let user: AppUserRecord = serde_json::from_slice(&current)?;
        if user.scope() != &self.tenant || user.id() != user_id {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        }
        Ok(AppUserMetadataUpdate {
            store: self,
            key,
            current,
            user,
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

/// The validated provider name of an external identity; email identities
/// and malformed provider names are refused before any key is formed.
fn provider_name_of(identity: &UserIdentityRecord) -> Result<ProviderName, IdentityStoreError> {
    let Some(name) = identity.provider().oidc_name() else {
        return Err(IdentityStoreError::RecordScopeMismatch);
    };
    Ok(ProviderName::parse(name)?)
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

/// The result of spending one magic-link token. Only `Redeemed` carries a
/// user, and it is returned at most once per token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MagicLinkOutcome {
    Redeemed { user_id: AppUserId },
    Unknown,
    Expired,
    AlreadyUsed,
}

/// A user read for a metadata replacement, carrying the bytes that were read
/// so the write is a compare-and-swap against them. Only the metadata and the
/// change timestamp move; status, epochs, and identity links are untouched.
pub struct AppUserMetadataUpdate<'a> {
    store: &'a IdentityStore,
    key: Vec<u8>,
    current: Vec<u8>,
    user: AppUserRecord,
}

impl fmt::Debug for AppUserMetadataUpdate<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppUserMetadataUpdate")
            .field("user", self.user.id())
            .finish_non_exhaustive()
    }
}

impl AppUserMetadataUpdate<'_> {
    /// The user as it was read.
    #[must_use]
    pub fn user(&self) -> &AppUserRecord {
        &self.user
    }

    /// Replaces both metadata maps, applying only if the user is still exactly
    /// what was read; a concurrent change surfaces as
    /// [`IdentityStoreError::ConcurrentIdentityChange`] so the caller can
    /// re-read and decide again.
    pub async fn commit(
        self,
        trusted_metadata: TrustedAppMetadata,
        profile_metadata: UserProfileMetadata,
        changed_at_unix_seconds: u64,
    ) -> Result<AppUserRecord, IdentityStoreError> {
        let updated =
            self.user
                .with_metadata(trusted_metadata, profile_metadata, changed_at_unix_seconds);
        let mut batch = WriteBatch::new();
        batch.put(&self.key, serde_json::to_vec(&updated)?);
        if self
            .store
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key: self.key,
                    value: self.current,
                }],
                batch,
                durability: self.store.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::ConcurrentIdentityChange);
        }
        Ok(updated)
    }
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
    ProviderIdentityAlreadyLinked,
    CorruptProviderOwner,
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
            Self::ProviderIdentityAlreadyLinked => {
                formatter.write_str("provider identity is already linked to a user")
            }
            Self::CorruptProviderOwner => formatter.write_str("provider identity owner is corrupt"),
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
        AppUserId, AppUserStatus, TrustedAppMetadata, UserCredentialId, UserIdentityId,
        UserProfileMetadata,
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

    #[test]
    fn provider_identities_are_unique_per_subject_and_tenant() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let first_tenant = tenant("prj_abcdefgh");
            let second_tenant = tenant("prj_ijklmnop");
            let first = store(adapter.clone(), &first_tenant);
            let second = store(adapter, &second_tenant);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let other_email = NormalizedEmail::parse("other@example.com").expect("email");

            let (user, identity) = provider_records(
                &first_tenant,
                "usr_first",
                "idn_first",
                "google",
                "subject-1",
            );
            first
                .create_provider_user(&user, &identity, &email)
                .await
                .expect("create provider user");
            let found = first
                .user_by_provider("google", "subject-1")
                .await
                .expect("lookup")
                .expect("user");
            assert_eq!(found.id().as_str(), "usr_first");
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

            let (duplicate, duplicate_identity) = provider_records(
                &first_tenant,
                "usr_second",
                "idn_second",
                "google",
                "subject-1",
            );
            assert!(matches!(
                first
                    .create_provider_user(&duplicate, &duplicate_identity, &other_email)
                    .await,
                Err(IdentityStoreError::ProviderIdentityAlreadyLinked)
            ));
            let (duplicate, duplicate_identity) = provider_records(
                &first_tenant,
                "usr_second",
                "idn_second",
                "google",
                "subject-2",
            );
            assert!(matches!(
                first
                    .create_provider_user(&duplicate, &duplicate_identity, &email)
                    .await,
                Err(IdentityStoreError::EmailAlreadyExists)
            ));
            assert!(
                first
                    .user_by_provider("google", "subject-2")
                    .await
                    .expect("lookup")
                    .is_none()
            );
            assert!(
                first
                    .user_by_provider("github", "subject-1")
                    .await
                    .expect("lookup")
                    .is_none()
            );

            let (email_user, email_identity) =
                records(&first_tenant, "usr_second", "idn_second", &other_email);
            assert!(matches!(
                first
                    .create_provider_user(&email_user, &email_identity, &other_email)
                    .await,
                Err(IdentityStoreError::RecordScopeMismatch)
            ));
            let (unvalidated, unvalidated_identity) = provider_records(
                &first_tenant,
                "usr_second",
                "idn_second",
                "Google",
                "subject-3",
            );
            assert!(matches!(
                first
                    .create_provider_user(&unvalidated, &unvalidated_identity, &other_email)
                    .await,
                Err(IdentityStoreError::Record(_))
            ));
            assert!(matches!(
                first.user_by_provider("Google", "subject-1").await,
                Err(IdentityStoreError::Record(_))
            ));

            assert!(
                second
                    .user_by_provider("google", "subject-1")
                    .await
                    .expect("lookup")
                    .is_none()
            );
            let (other_user, other_identity) = provider_records(
                &second_tenant,
                "usr_third",
                "idn_third",
                "google",
                "subject-1",
            );
            second
                .create_provider_user(&other_user, &other_identity, &email)
                .await
                .expect("other project");
            assert_eq!(
                second
                    .user_by_provider("google", "subject-1")
                    .await
                    .expect("lookup")
                    .expect("user")
                    .id()
                    .as_str(),
                "usr_third"
            );
            assert_eq!(
                first
                    .user_by_provider("google", "subject-1")
                    .await
                    .expect("lookup")
                    .expect("user")
                    .id()
                    .as_str(),
                "usr_first"
            );
        });
    }

    #[test]
    fn provider_identities_link_to_existing_email_users() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let first_tenant = tenant("prj_abcdefgh");
            let second_tenant = tenant("prj_ijklmnop");
            let first = store(adapter.clone(), &first_tenant);
            let second = store(adapter, &second_tenant);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let other_email = NormalizedEmail::parse("other@example.com").expect("email");
            let third_email = NormalizedEmail::parse("third@example.com").expect("email");
            let (user, identity) = records(&first_tenant, "usr_first", "idn_first", &email);
            first
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create email user");

            let link = provider_identity(
                &first_tenant,
                "idn_google",
                "usr_first",
                "google",
                "subject-1",
            );
            first
                .link_provider_identity(user.id(), &link)
                .await
                .expect("link");
            assert_eq!(
                first
                    .user_by_provider("google", "subject-1")
                    .await
                    .expect("lookup")
                    .expect("user")
                    .id()
                    .as_str(),
                "usr_first"
            );
            assert!(
                second
                    .user_by_provider("google", "subject-1")
                    .await
                    .expect("lookup")
                    .is_none()
            );
            assert!(matches!(
                first.link_provider_identity(user.id(), &link).await,
                Err(IdentityStoreError::RecordAlreadyExists)
            ));

            let (other, other_identity) =
                records(&first_tenant, "usr_second", "idn_second", &other_email);
            first
                .create_email_user(&other, &other_identity, &other_email)
                .await
                .expect("create other");
            let relink = provider_identity(
                &first_tenant,
                "idn_relink",
                "usr_second",
                "google",
                "subject-1",
            );
            assert!(matches!(
                first.link_provider_identity(other.id(), &relink).await,
                Err(IdentityStoreError::ProviderIdentityAlreadyLinked)
            ));
            let (claimant, claimant_identity) = provider_records(
                &first_tenant,
                "usr_third",
                "idn_third",
                "google",
                "subject-1",
            );
            assert!(matches!(
                first
                    .create_provider_user(&claimant, &claimant_identity, &third_email)
                    .await,
                Err(IdentityStoreError::ProviderIdentityAlreadyLinked)
            ));
            assert!(
                first
                    .user_by_email(&third_email)
                    .await
                    .expect("lookup")
                    .is_none()
            );

            let missing = AppUserId::parse("usr_missing").expect("user");
            let orphan = provider_identity(
                &first_tenant,
                "idn_orphan",
                "usr_missing",
                "github",
                "subject-9",
            );
            assert!(matches!(
                first.link_provider_identity(&missing, &orphan).await,
                Err(IdentityStoreError::UserNotFound)
            ));
            assert!(
                first
                    .user_by_provider("github", "subject-9")
                    .await
                    .expect("lookup")
                    .is_none()
            );
            let mismatched = provider_identity(
                &first_tenant,
                "idn_mismatched",
                "usr_second",
                "github",
                "subject-8",
            );
            assert!(matches!(
                first.link_provider_identity(user.id(), &mismatched).await,
                Err(IdentityStoreError::RecordScopeMismatch)
            ));
            let email_link = UserIdentityRecord::new(
                first_tenant.clone(),
                UserIdentityId::parse("idn_email").expect("identity"),
                user.id().clone(),
                IdentityProvider::Email,
                third_email.as_str(),
                1,
            )
            .expect("identity");
            assert!(matches!(
                first.link_provider_identity(user.id(), &email_link).await,
                Err(IdentityStoreError::RecordScopeMismatch)
            ));

            first
                .disable_user(other.id(), 5)
                .await
                .expect("disable other");
            let disabled_link = provider_identity(
                &first_tenant,
                "idn_disabled",
                "usr_second",
                "github",
                "subject-7",
            );
            assert!(matches!(
                first
                    .link_provider_identity(other.id(), &disabled_link)
                    .await,
                Err(IdentityStoreError::InvalidUserStatusTransition)
            ));
            assert!(
                first
                    .user_by_provider("github", "subject-7")
                    .await
                    .expect("lookup")
                    .is_none()
            );
        });
    }

    #[test]
    fn magic_links_are_single_use_and_activate_pending_users() {
        futures::executor::block_on(async {
            let scope = tenant("prj_abcdefgh");
            let store = store(Arc::new(MemoryAdapter::new()), &scope);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let (user, identity) = records(&scope, "usr_first", "idn_first", &email);
            let user = user.with_status(AppUserStatus::PendingVerification, 1);
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create pending user");

            let link = credential(
                &scope,
                "crd_link",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            );
            let digest = [1_u8; 32];
            store
                .create_magic_link(&user, &link, &digest)
                .await
                .expect("create link");
            assert!(matches!(
                store.create_magic_link(&user, &link, &digest).await,
                Err(IdentityStoreError::RecordAlreadyExists)
            ));

            assert_eq!(
                store.redeem_magic_link(&digest, 50).await.expect("redeem"),
                MagicLinkOutcome::Redeemed {
                    user_id: user.id().clone()
                }
            );
            assert_eq!(
                store
                    .user_by_id(user.id())
                    .await
                    .expect("lookup")
                    .expect("user")
                    .status(),
                AppUserStatus::Active
            );

            let again = store
                .redeem_magic_link(&digest, 51)
                .await
                .expect("redeem again");
            assert!(!matches!(again, MagicLinkOutcome::Redeemed { .. }));
            assert_eq!(again, MagicLinkOutcome::Unknown);
            assert_eq!(
                store
                    .complete_email_verification(&digest, 52)
                    .await
                    .expect("verify"),
                EmailVerificationOutcome::InvalidOrExpired
            );

            let active_link = credential(
                &scope,
                "crd_active",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            );
            let active_digest = [2_u8; 32];
            store
                .create_magic_link(&user, &active_link, &active_digest)
                .await
                .expect("create second link");
            assert_eq!(
                store
                    .redeem_magic_link(&active_digest, 60)
                    .await
                    .expect("redeem active"),
                MagicLinkOutcome::Redeemed {
                    user_id: user.id().clone()
                }
            );
            assert_eq!(
                store
                    .user_by_id(user.id())
                    .await
                    .expect("lookup")
                    .expect("user")
                    .updated_at_unix_seconds(),
                50,
                "an already active user is left untouched"
            );
        });
    }

    #[test]
    fn magic_links_expire_and_unknown_digests_are_rejected() {
        futures::executor::block_on(async {
            let scope = tenant("prj_abcdefgh");
            let store = store(Arc::new(MemoryAdapter::new()), &scope);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let (user, identity) = records(&scope, "usr_first", "idn_first", &email);
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create user");
            let link = credential(
                &scope,
                "crd_link",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            );
            let digest = [3_u8; 32];
            store
                .create_magic_link(&user, &link, &digest)
                .await
                .expect("create link");

            assert_eq!(
                store
                    .redeem_magic_link(&digest, 100)
                    .await
                    .expect("redeem at expiry"),
                MagicLinkOutcome::Expired
            );
            assert_eq!(
                store
                    .redeem_magic_link(&digest, 1_000)
                    .await
                    .expect("redeem after expiry"),
                MagicLinkOutcome::Expired
            );
            assert_eq!(
                store
                    .redeem_magic_link(&[4_u8; 32], 50)
                    .await
                    .expect("redeem unknown"),
                MagicLinkOutcome::Unknown
            );
            assert_eq!(
                store
                    .redeem_magic_link(&digest, 99)
                    .await
                    .expect("redeem before expiry"),
                MagicLinkOutcome::Redeemed {
                    user_id: user.id().clone()
                }
            );
        });
    }

    #[test]
    fn magic_link_redemption_is_bound_to_magic_link_credentials() {
        futures::executor::block_on(async {
            let scope = tenant("prj_abcdefgh");
            let store = store(Arc::new(MemoryAdapter::new()), &scope);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let (user, identity) = records(&scope, "usr_first", "idn_first", &email);
            let user = user.with_status(AppUserStatus::PendingVerification, 1);
            let password = credential(
                &scope,
                "crd_password",
                "usr_first",
                UserCredentialKind::Password,
                None,
            );
            let verification = credential(
                &scope,
                "crd_verify",
                "usr_first",
                UserCredentialKind::EmailVerification,
                Some(100),
            );
            let verification_digest = [5_u8; 32];
            store
                .create_email_signup(
                    &user,
                    &identity,
                    &password,
                    Some((&verification, &verification_digest)),
                    &email,
                )
                .await
                .expect("signup");

            assert_eq!(
                store
                    .redeem_magic_link(&verification_digest, 50)
                    .await
                    .expect("redeem verification token"),
                MagicLinkOutcome::Unknown
            );
            assert_eq!(
                store
                    .user_by_id(user.id())
                    .await
                    .expect("lookup")
                    .expect("user")
                    .status(),
                AppUserStatus::PendingVerification
            );

            for (id, kind, expires) in [
                ("crd_password", UserCredentialKind::Password, Some(100)),
                (
                    "crd_recovery",
                    UserCredentialKind::PasswordRecovery,
                    Some(100),
                ),
                (
                    "crd_verify",
                    UserCredentialKind::EmailVerification,
                    Some(100),
                ),
                ("crd_unbounded", UserCredentialKind::MagicLink, None),
            ] {
                let credential = credential(&scope, id, "usr_first", kind, expires);
                assert!(
                    matches!(
                        store
                            .create_magic_link(&user, &credential, &[6_u8; 32])
                            .await,
                        Err(IdentityStoreError::RecordScopeMismatch)
                    ),
                    "{kind:?} with expiry {expires:?} must not be issued as a magic link"
                );
            }
            let spent = credential(
                &scope,
                "crd_spent",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            )
            .invalidate(2);
            assert!(matches!(
                store.create_magic_link(&user, &spent, &[6_u8; 32]).await,
                Err(IdentityStoreError::RecordScopeMismatch)
            ));
            assert_eq!(
                store
                    .redeem_magic_link(&[6_u8; 32], 50)
                    .await
                    .expect("redeem never issued"),
                MagicLinkOutcome::Unknown
            );

            let link = credential(
                &scope,
                "crd_link",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            );
            let digest = [7_u8; 32];
            store
                .create_magic_link(&user, &link, &digest)
                .await
                .expect("create link");
            assert_eq!(
                store
                    .complete_email_verification(&digest, 50)
                    .await
                    .expect("verify with magic link"),
                EmailVerificationOutcome::InvalidOrExpired
            );
            assert_eq!(
                store
                    .reset_password_with_token(
                        &digest,
                        CredentialDigest::new(vec![8; 32]).expect("digest"),
                        50
                    )
                    .await
                    .expect("reset with magic link"),
                PasswordResetOutcome::InvalidOrExpired
            );
            assert_eq!(
                store.redeem_magic_link(&digest, 50).await.expect("redeem"),
                MagicLinkOutcome::Redeemed {
                    user_id: user.id().clone()
                }
            );
        });
    }

    #[test]
    fn magic_links_for_inactive_users_are_spent_without_redemption() {
        futures::executor::block_on(async {
            let scope = tenant("prj_abcdefgh");
            let store = store(Arc::new(MemoryAdapter::new()), &scope);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let (user, identity) = records(&scope, "usr_first", "idn_first", &email);
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create user");
            let link = credential(
                &scope,
                "crd_link",
                "usr_first",
                UserCredentialKind::MagicLink,
                Some(100),
            );
            let digest = [9_u8; 32];
            store
                .create_magic_link(&user, &link, &digest)
                .await
                .expect("create link");
            store.disable_user(user.id(), 5).await.expect("disable");

            assert_eq!(
                store
                    .redeem_magic_link(&digest, 50)
                    .await
                    .expect("redeem disabled"),
                MagicLinkOutcome::Unknown
            );
            store.restore_user(user.id(), 60).await.expect("restore");
            assert_eq!(
                store
                    .redeem_magic_link(&digest, 70)
                    .await
                    .expect("redeem after restore"),
                MagicLinkOutcome::Unknown,
                "a link spent while disabled stays spent"
            );
            let credential_key = store
                .keyspace
                .application_credential_key("crd_link")
                .expect("credential key");
            let stored: UserCredentialRecord = serde_json::from_slice(
                &store
                    .adapter
                    .get(&credential_key)
                    .await
                    .expect("read")
                    .expect("credential"),
            )
            .expect("decode");
            assert_eq!(stored.invalidated_at_unix_seconds(), Some(50));
        });
    }

    fn store(adapter: Arc<MemoryAdapter>, scope: &TenantScope) -> IdentityStore {
        IdentityStore::new(adapter, scope, scope, Durability::Memory).expect("store")
    }

    fn provider_records(
        tenant: &TenantScope,
        user_id: &str,
        identity_id: &str,
        provider: &str,
        subject: &str,
    ) -> (AppUserRecord, UserIdentityRecord) {
        (
            AppUserRecord::new(
                tenant.clone(),
                AppUserId::parse(user_id).expect("user"),
                AppUserStatus::Active,
                TrustedAppMetadata::new(json!({})).expect("trusted"),
                UserProfileMetadata::new(json!({})).expect("profile"),
                1,
            ),
            provider_identity(tenant, identity_id, user_id, provider, subject),
        )
    }

    /// Builds the provider variant directly, bypassing `IdentityProvider::oidc`,
    /// so the tests can show the store validates provider names itself.
    fn provider_identity(
        tenant: &TenantScope,
        identity_id: &str,
        user_id: &str,
        provider: &str,
        subject: &str,
    ) -> UserIdentityRecord {
        UserIdentityRecord::new(
            tenant.clone(),
            UserIdentityId::parse(identity_id).expect("identity"),
            AppUserId::parse(user_id).expect("user"),
            IdentityProvider::Oidc(provider.to_owned()),
            subject,
            1,
        )
        .expect("identity")
    }

    fn credential(
        tenant: &TenantScope,
        id: &str,
        user_id: &str,
        kind: UserCredentialKind,
        expires_at_unix_seconds: Option<u64>,
    ) -> UserCredentialRecord {
        UserCredentialRecord::new(
            tenant.clone(),
            UserCredentialId::parse(id).expect("credential"),
            AppUserId::parse(user_id).expect("user"),
            kind,
            CredentialDigest::new(vec![9; 32]).expect("digest"),
            1,
            expires_at_unix_seconds,
        )
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

    #[test]
    fn metadata_updates_are_compare_and_swap_over_what_was_read() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant("prj_abcdefgh");
            let store = store(adapter, &tenant);
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let (user, identity) = records(&tenant, "usr_metadata", "idn_metadata", &email);
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create");

            let missing = AppUserId::parse("usr_missing0").expect("user");
            assert!(matches!(
                store.begin_user_metadata_update(&missing).await,
                Err(IdentityStoreError::UserNotFound)
            ));

            let first = store
                .begin_user_metadata_update(user.id())
                .await
                .expect("first read");
            let second = store
                .begin_user_metadata_update(user.id())
                .await
                .expect("second read");
            assert_eq!(first.user(), &user);
            let written = first
                .commit(
                    TrustedAppMetadata::new(json!({"role": "editor"})).expect("trusted"),
                    user.profile_metadata().clone(),
                    50,
                )
                .await
                .expect("first commit");
            assert_eq!(written.trusted_metadata().values()["role"], "editor");
            assert_eq!(written.updated_at_unix_seconds(), 50);
            assert_eq!(written.status(), user.status());
            assert_eq!(written.session_epoch(), user.session_epoch());
            // The stale read cannot overwrite what the first writer committed.
            assert!(matches!(
                second
                    .commit(
                        TrustedAppMetadata::new(json!({"role": "admin"})).expect("trusted"),
                        user.profile_metadata().clone(),
                        51,
                    )
                    .await,
                Err(IdentityStoreError::ConcurrentIdentityChange)
            ));
            let stored = store
                .user_by_id(user.id())
                .await
                .expect("lookup")
                .expect("user");
            assert_eq!(stored, written);
        });
    }

    fn tenant(project: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
