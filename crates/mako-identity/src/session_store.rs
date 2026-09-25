use std::{error::Error, fmt};

use mako_storage::{
    AtomicWrite, CompareAndWriteResult, KeyCodecError, KeyCondition, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, IdentityRecordError, IdentityStore,
    IdentityStoreError, KeyEncryptionKey, NormalizedEmail, RefreshCredential, SessionId,
    SessionRecord, SigningKeyError, TokenFamilyId, TokenFamilyStatus,
};

const SESSION_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;
const REFRESH_CONCURRENCY_GRACE_SECONDS: u64 = 5;
const MAX_CONFLICT_RETRIES: usize = 8;

#[derive(Clone, Debug)]
pub struct ApplicationSessionGrant {
    pub user: AppUserRecord,
    pub email: Option<NormalizedEmail>,
    pub session: SessionRecord,
    pub refresh_credential: RefreshCredential,
}

#[derive(Clone, Debug)]
pub enum RefreshSessionOutcome {
    Rotated(Box<ApplicationSessionGrant>),
    Invalid,
    ReplayDetected,
}

pub struct ApplicationSessionStore<'a> {
    identity: &'a IdentityStore,
    protection_key: &'a KeyEncryptionKey,
}

impl<'a> ApplicationSessionStore<'a> {
    #[must_use]
    pub const fn new(identity: &'a IdentityStore, protection_key: &'a KeyEncryptionKey) -> Self {
        Self {
            identity,
            protection_key,
        }
    }

    pub async fn create(
        &self,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<ApplicationSessionGrant, ApplicationSessionStoreError> {
        let user = self
            .identity
            .user_by_id(user_id)
            .await?
            .ok_or(ApplicationSessionStoreError::InvalidUser)?;
        if user.status() != AppUserStatus::Active {
            return Err(ApplicationSessionStoreError::InvalidUser);
        }
        let expires_at = now_unix_seconds
            .checked_add(SESSION_LIFETIME_SECONDS)
            .ok_or(ApplicationSessionStoreError::TimeOverflow)?;
        let session = SessionRecord::new(
            self.identity.tenant.clone(),
            SessionId::parse(random_id("ses"))?,
            user.id().clone(),
            now_unix_seconds,
            expires_at,
            user.session_epoch(),
        );
        let family_id = TokenFamilyId::parse(random_id("fam"))?;
        let refresh = RefreshCredential::generate();
        let digest = credential_digest(&refresh);
        let aad = family_aad(self.identity.tenant(), &family_id, 0);
        let (encrypted_current_credential, encryption_nonce) = self
            .protection_key
            .encrypt_secret(refresh.expose_for_token_response().as_bytes(), &aad)?;
        let family = StoredRefreshFamily {
            tenant: self.identity.tenant.clone(),
            family_id: family_id.clone(),
            session_id: session.id().clone(),
            user_id: user.id().clone(),
            status: TokenFamilyStatus::Active,
            generation: 0,
            current_digest: digest,
            previous: None,
            encrypted_current_credential,
            encryption_nonce,
            expires_at_unix_seconds: expires_at,
        };
        let session_key = self
            .identity
            .keyspace
            .application_session_key(session.id().as_str())?;
        let user_session_key = self
            .identity
            .keyspace
            .application_user_session_key(user.id().as_str(), session.id().as_str())?;
        let family_key = self
            .identity
            .keyspace
            .application_token_family_key(family_id.as_str())?;
        let digest_key = self
            .identity
            .keyspace
            .identity_token_digest_owner_key(digest)?;
        let user_key = self
            .identity
            .keyspace
            .application_user_key(user.id().as_str())?;
        let user_bytes = self
            .identity
            .adapter
            .get(&user_key)
            .await?
            .ok_or(ApplicationSessionStoreError::InvalidUser)?;
        let conditions = vec![
            KeyCondition::ValueEquals {
                key: user_key,
                value: user_bytes,
            },
            KeyCondition::Missing {
                key: session_key.clone(),
            },
            KeyCondition::Missing {
                key: user_session_key.clone(),
            },
            KeyCondition::Missing {
                key: family_key.clone(),
            },
            KeyCondition::Missing {
                key: digest_key.clone(),
            },
        ];
        let mut batch = WriteBatch::with_capacity(4);
        batch.put(session_key, serde_json::to_vec(&session)?);
        batch.put(user_session_key, session.id().as_str());
        batch.put(family_key, serde_json::to_vec(&family)?);
        batch.put(digest_key, family_id.as_str());
        if self
            .identity
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.identity.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(ApplicationSessionStoreError::ConcurrentChange);
        }
        let email = self.identity.email_for_user(user.id()).await?;
        Ok(ApplicationSessionGrant {
            user,
            email,
            session,
            refresh_credential: refresh,
        })
    }

    pub async fn refresh(
        &self,
        presented: &str,
        now_unix_seconds: u64,
    ) -> Result<RefreshSessionOutcome, ApplicationSessionStoreError> {
        if !valid_presented_refresh(presented) {
            return Ok(RefreshSessionOutcome::Invalid);
        }
        let presented_digest = *blake3::hash(presented.as_bytes()).as_bytes();
        let digest_key = self
            .identity
            .keyspace
            .identity_token_digest_owner_key(presented_digest)?;
        let Some(family_id_bytes) = self.identity.adapter.get(&digest_key).await? else {
            return Ok(RefreshSessionOutcome::Invalid);
        };
        let family_id = std::str::from_utf8(&family_id_bytes)
            .map_err(|_| ApplicationSessionStoreError::CorruptFamily)
            .and_then(|value| TokenFamilyId::parse(value).map_err(Into::into))?;
        let family_key = self
            .identity
            .keyspace
            .application_token_family_key(family_id.as_str())?;

        for _ in 0..MAX_CONFLICT_RETRIES {
            let Some(family_bytes) = self.identity.adapter.get(&family_key).await? else {
                return Err(ApplicationSessionStoreError::CorruptFamily);
            };
            let family: StoredRefreshFamily = serde_json::from_slice(&family_bytes)?;
            family.validate(self.identity.tenant(), &family_id)?;
            if family.status != TokenFamilyStatus::Active
                || now_unix_seconds >= family.expires_at_unix_seconds
            {
                return Ok(RefreshSessionOutcome::Invalid);
            }
            // A session that was signed out, expired, or whose user was
            // disabled, deleted, or signed out everywhere ends its refresh
            // family. Its credential is refused like a spent one, before any
            // rotation is written; failing only after rotating reported the
            // refusal as an outage, which clients retry instead of signing in.
            if !self
                .identity
                .session_is_active(&family.user_id, &family.session_id, now_unix_seconds)
                .await?
            {
                return Ok(RefreshSessionOutcome::Invalid);
            }
            if family.current_digest == presented_digest {
                if let Some(grant) = self
                    .rotate(
                        family,
                        family_key.clone(),
                        family_bytes,
                        digest_key.clone(),
                        now_unix_seconds,
                    )
                    .await?
                {
                    return Ok(RefreshSessionOutcome::Rotated(Box::new(grant)));
                }
                continue;
            }
            if let Some(previous) = family.previous
                && previous.digest == presented_digest
            {
                if now_unix_seconds
                    <= previous
                        .consumed_at_unix_seconds
                        .saturating_add(REFRESH_CONCURRENCY_GRACE_SECONDS)
                {
                    return self
                        .grant_from_family(&family, now_unix_seconds)
                        .await
                        .map(Box::new)
                        .map(RefreshSessionOutcome::Rotated);
                }
                self.identity
                    .sign_out_session(&family.user_id, &family.session_id, now_unix_seconds)
                    .await?;
                let replayed = StoredRefreshFamily {
                    status: TokenFamilyStatus::ReplayDetected,
                    ..family
                };
                let mut batch = WriteBatch::new();
                batch.put(&family_key, serde_json::to_vec(&replayed)?);
                let result = self
                    .identity
                    .adapter
                    .compare_and_write(AtomicWrite {
                        conditions: vec![KeyCondition::ValueEquals {
                            key: family_key.clone(),
                            value: family_bytes,
                        }],
                        batch,
                        durability: self.identity.durability,
                    })
                    .await?;
                if result == CompareAndWriteResult::Applied {
                    return Ok(RefreshSessionOutcome::ReplayDetected);
                }
                continue;
            }
            return Ok(RefreshSessionOutcome::Invalid);
        }
        Err(ApplicationSessionStoreError::ConcurrentChange)
    }

    async fn rotate(
        &self,
        family: StoredRefreshFamily,
        family_key: Vec<u8>,
        family_bytes: Vec<u8>,
        presented_digest_key: Vec<u8>,
        now_unix_seconds: u64,
    ) -> Result<Option<ApplicationSessionGrant>, ApplicationSessionStoreError> {
        let replacement = RefreshCredential::generate();
        let replacement_digest = credential_digest(&replacement);
        let replacement_digest_key = self
            .identity
            .keyspace
            .identity_token_digest_owner_key(replacement_digest)?;
        let generation = family
            .generation
            .checked_add(1)
            .ok_or(ApplicationSessionStoreError::GenerationExhausted)?;
        let aad = family_aad(self.identity.tenant(), &family.family_id, generation);
        let (encrypted_current_credential, encryption_nonce) = self
            .protection_key
            .encrypt_secret(replacement.expose_for_token_response().as_bytes(), &aad)?;
        let updated = StoredRefreshFamily {
            generation,
            current_digest: replacement_digest,
            previous: Some(ConsumedDigest {
                digest: family.current_digest,
                consumed_at_unix_seconds: now_unix_seconds,
            }),
            encrypted_current_credential,
            encryption_nonce,
            ..family.clone()
        };
        let mut conditions = vec![
            KeyCondition::ValueEquals {
                key: family_key.clone(),
                value: family_bytes,
            },
            KeyCondition::ValueEquals {
                key: presented_digest_key,
                value: family.family_id.as_str().as_bytes().to_vec(),
            },
            KeyCondition::Missing {
                key: replacement_digest_key.clone(),
            },
        ];
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&family_key, serde_json::to_vec(&updated)?);
        batch.put(&replacement_digest_key, family.family_id.as_str());
        if let Some(previous) = family.previous {
            let previous_key = self
                .identity
                .keyspace
                .identity_token_digest_owner_key(previous.digest)?;
            let previous_value = self.identity.adapter.get(&previous_key).await?;
            if let Some(value) = previous_value {
                conditions.push(KeyCondition::ValueEquals {
                    key: previous_key.clone(),
                    value,
                });
                batch.delete(previous_key);
            }
        }
        if self
            .identity
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.identity.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Ok(None);
        }
        self.grant_from_family_with_credential(&updated, replacement, now_unix_seconds)
            .await
            .map(Some)
    }

    async fn grant_from_family(
        &self,
        family: &StoredRefreshFamily,
        now_unix_seconds: u64,
    ) -> Result<ApplicationSessionGrant, ApplicationSessionStoreError> {
        let plaintext = self.protection_key.decrypt_secret(
            &family.encrypted_current_credential,
            &family.encryption_nonce,
            &family_aad(self.identity.tenant(), &family.family_id, family.generation),
        )?;
        let refresh = String::from_utf8(plaintext)
            .map(RefreshCredential::from_protected)
            .map_err(|_| ApplicationSessionStoreError::CorruptFamily)?;
        self.grant_from_family_with_credential(family, refresh, now_unix_seconds)
            .await
    }

    async fn grant_from_family_with_credential(
        &self,
        family: &StoredRefreshFamily,
        refresh_credential: RefreshCredential,
        now_unix_seconds: u64,
    ) -> Result<ApplicationSessionGrant, ApplicationSessionStoreError> {
        let session = self
            .identity
            .session_by_id(&family.session_id)
            .await?
            .ok_or(ApplicationSessionStoreError::CorruptFamily)?;
        let user = self
            .identity
            .user_by_id(&family.user_id)
            .await?
            .ok_or(ApplicationSessionStoreError::CorruptFamily)?;
        if !self
            .identity
            .session_is_active(user.id(), session.id(), now_unix_seconds)
            .await?
        {
            return Err(ApplicationSessionStoreError::InvalidUser);
        }
        let email = self.identity.email_for_user(user.id()).await?;
        Ok(ApplicationSessionGrant {
            user,
            email,
            session,
            refresh_credential,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredRefreshFamily {
    tenant: mako_api::TenantScope,
    family_id: TokenFamilyId,
    session_id: SessionId,
    user_id: AppUserId,
    status: TokenFamilyStatus,
    generation: u64,
    current_digest: [u8; 32],
    previous: Option<ConsumedDigest>,
    encrypted_current_credential: Vec<u8>,
    encryption_nonce: [u8; 24],
    expires_at_unix_seconds: u64,
}

impl StoredRefreshFamily {
    fn validate(
        &self,
        tenant: &mako_api::TenantScope,
        family_id: &TokenFamilyId,
    ) -> Result<(), ApplicationSessionStoreError> {
        if &self.tenant != tenant || &self.family_id != family_id {
            return Err(ApplicationSessionStoreError::CorruptFamily);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ConsumedDigest {
    digest: [u8; 32],
    consumed_at_unix_seconds: u64,
}

fn credential_digest(credential: &RefreshCredential) -> [u8; 32] {
    *blake3::hash(credential.expose_for_token_response().as_bytes()).as_bytes()
}

fn valid_presented_refresh(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn family_aad(
    tenant: &mako_api::TenantScope,
    family_id: &TokenFamilyId,
    generation: u64,
) -> Vec<u8> {
    format!(
        "mako-refresh-family\0{}\0{}\0{}\0{}",
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        family_id.as_str(),
        generation,
    )
    .into_bytes()
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut bytes);
    let mut value = String::with_capacity(prefix.len() + 1 + bytes.len() * 2);
    value.push_str(prefix);
    value.push('_');
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut value, "{byte:02x}").expect("formatting into a string cannot fail");
    }
    value
}

#[derive(Debug)]
pub enum ApplicationSessionStoreError {
    InvalidUser,
    TimeOverflow,
    GenerationExhausted,
    ConcurrentChange,
    CorruptFamily,
    Identity(IdentityStoreError),
    Record(IdentityRecordError),
    Key(KeyCodecError),
    Storage(StorageError),
    Signing(SigningKeyError),
    Json(serde_json::Error),
}

impl fmt::Display for ApplicationSessionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidUser => "application user cannot create or refresh a session",
            Self::TimeOverflow => "application session expiry overflowed",
            Self::GenerationExhausted => "refresh generation is exhausted",
            Self::ConcurrentChange => "application session changed concurrently",
            Self::CorruptFamily => "refresh family state is corrupt",
            Self::Identity(_) => "application identity operation failed",
            Self::Record(_) => "application session record is invalid",
            Self::Key(_) => "application session storage key is invalid",
            Self::Storage(_) => "application session storage is unavailable",
            Self::Signing(_) => "refresh credential protection failed",
            Self::Json(_) => "application session record serialization failed",
        })
    }
}

impl Error for ApplicationSessionStoreError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for ApplicationSessionStoreError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(IdentityStoreError, Identity);
error_from!(IdentityRecordError, Record);
error_from!(KeyCodecError, Key);
error_from!(StorageError, Storage);
error_from!(SigningKeyError, Signing);
error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::{
        IdentityProvider, TrustedAppMetadata, UserIdentityId, UserIdentityRecord,
        UserProfileMetadata,
    };

    #[test]
    fn refresh_state_persists_rotates_and_revokes_on_replay() {
        block_on(async {
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let adapter = Arc::new(MemoryAdapter::new());
            let store = IdentityStore::new(adapter.clone(), &tenant, &tenant, Durability::Memory)
                .expect("store");
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let user_id = AppUserId::parse("usr_abcdefgh").expect("user");
            let user = AppUserRecord::new(
                tenant.clone(),
                user_id.clone(),
                AppUserStatus::Active,
                TrustedAppMetadata::new(json!({"role":"member"})).expect("trusted metadata"),
                UserProfileMetadata::new(json!({})).expect("profile metadata"),
                1,
            );
            let identity = UserIdentityRecord::new(
                tenant.clone(),
                UserIdentityId::parse("idn_abcdefgh").expect("identity"),
                user_id.clone(),
                IdentityProvider::Email,
                email.as_str(),
                1,
            )
            .expect("identity record");
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create user");
            let key = KeyEncryptionKey::generate();
            let sessions = ApplicationSessionStore::new(&store, &key);
            let issued = sessions.create(&user_id, 10).await.expect("session");
            let first = issued
                .refresh_credential
                .expose_for_token_response()
                .to_owned();
            assert_eq!(issued.email.as_ref(), Some(&email));

            let reopened = IdentityStore::new(adapter, &tenant, &tenant, Durability::Memory)
                .expect("reopened store");
            let sessions = ApplicationSessionStore::new(&reopened, &key);
            let RefreshSessionOutcome::Rotated(rotated) =
                sessions.refresh(&first, 11).await.expect("rotate")
            else {
                panic!("refresh must rotate");
            };
            assert_ne!(
                rotated.refresh_credential.expose_for_token_response(),
                first
            );
            let RefreshSessionOutcome::Rotated(concurrent) = sessions
                .refresh(&first, 15)
                .await
                .expect("concurrent refresh")
            else {
                panic!("bounded concurrent refresh must succeed");
            };
            assert_eq!(
                concurrent.refresh_credential.expose_for_token_response(),
                rotated.refresh_credential.expose_for_token_response()
            );
            assert!(matches!(
                sessions.refresh(&first, 17).await.expect("replay"),
                RefreshSessionOutcome::ReplayDetected
            ));
            assert!(
                !reopened
                    .session_is_active(&user_id, issued.session.id(), 18)
                    .await
                    .expect("session state")
            );
        });
    }
    #[test]
    fn a_signed_out_session_refuses_its_refresh_credential() {
        block_on(async {
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let email = NormalizedEmail::parse("person@example.com").expect("email");
            let user_id = AppUserId::parse("usr_abcdefgh").expect("user");
            let user = AppUserRecord::new(
                tenant.clone(),
                user_id.clone(),
                AppUserStatus::Active,
                TrustedAppMetadata::new(json!({})).expect("trusted metadata"),
                UserProfileMetadata::new(json!({})).expect("profile metadata"),
                1,
            );
            let identity = UserIdentityRecord::new(
                tenant.clone(),
                UserIdentityId::parse("idn_abcdefgh").expect("identity"),
                user_id.clone(),
                IdentityProvider::Email,
                email.as_str(),
                1,
            )
            .expect("identity record");
            store
                .create_email_user(&user, &identity, &email)
                .await
                .expect("create user");
            let key = KeyEncryptionKey::generate();
            let sessions = ApplicationSessionStore::new(&store, &key);
            let issued = sessions.create(&user_id, 10).await.expect("session");
            let credential = issued
                .refresh_credential
                .expose_for_token_response()
                .to_owned();
            store
                .sign_out_session(&user_id, issued.session.id(), 11)
                .await
                .expect("sign out");
            // Refused, not an error the service would report as unavailable,
            // and nothing was rotated: a second attempt is refused the same way.
            for now in [12, 13] {
                assert!(matches!(
                    sessions.refresh(&credential, now).await.expect("refresh"),
                    RefreshSessionOutcome::Invalid
                ));
            }
        });
    }
}
