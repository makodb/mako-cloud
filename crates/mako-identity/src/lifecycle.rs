use std::num::NonZeroUsize;

use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, KeyCondition, ScanDirection, ScanRequest, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, IdentityStore, IdentityStoreError, SessionId,
    SessionRecord, SessionStatus,
};

const MAX_CONCURRENT_RETRIES: usize = 32;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityRevocationKind {
    UserDisabled,
    UserRestored,
    UserDeleted,
    SessionSignedOut,
    AllSessionsSignedOut,
    PasswordChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdentityRevocationEvent {
    scope: TenantScope,
    sequence: u64,
    kind: IdentityRevocationKind,
    user_id: AppUserId,
    session_id: Option<SessionId>,
    user_session_epoch: u64,
    occurred_at_unix_seconds: u64,
}

impl IdentityRevocationEvent {
    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn kind(&self) -> IdentityRevocationKind {
        self.kind
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    #[must_use]
    pub const fn user_session_epoch(&self) -> u64 {
        self.user_session_epoch
    }

    #[must_use]
    pub const fn occurred_at_unix_seconds(&self) -> u64 {
        self.occurred_at_unix_seconds
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UserLifecycleAction {
    Disable,
    Restore,
    Delete,
    SignOutAll,
}

impl IdentityStore {
    pub async fn create_session(&self, session: &SessionRecord) -> Result<(), IdentityStoreError> {
        if session.scope() != &self.tenant {
            return Err(IdentityStoreError::RecordScopeMismatch);
        }
        let user_key = self
            .keyspace
            .application_user_key(session.user_id().as_str())?;
        let user_bytes = self
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::UserNotFound)?;
        let user: AppUserRecord = serde_json::from_slice(&user_bytes)?;
        if user.scope() != &self.tenant || user.id() != session.user_id() {
            return Err(IdentityStoreError::CorruptSession);
        }
        if user.status() != AppUserStatus::Active
            || session.status() != SessionStatus::Active
            || session.user_session_epoch() != user.session_epoch()
        {
            return Err(IdentityStoreError::InvalidUserStatusTransition);
        }
        let session_key = self
            .keyspace
            .application_session_key(session.id().as_str())?;
        let user_session_key = self
            .keyspace
            .application_user_session_key(user.id().as_str(), session.id().as_str())?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&session_key, serde_json::to_vec(session)?);
        batch.put(&user_session_key, session.id().as_str());
        let result = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::ValueEquals {
                        key: user_key,
                        value: user_bytes,
                    },
                    KeyCondition::Missing { key: session_key },
                    KeyCondition::Missing {
                        key: user_session_key,
                    },
                ],
                batch,
                durability: self.durability,
            })
            .await?;
        if result == CompareAndWriteResult::Applied {
            Ok(())
        } else {
            Err(IdentityStoreError::ConcurrentIdentityChange)
        }
    }

    pub async fn user_by_id(
        &self,
        user_id: &AppUserId,
    ) -> Result<Option<AppUserRecord>, IdentityStoreError> {
        let key = self.keyspace.application_user_key(user_id.as_str())?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let user: AppUserRecord = serde_json::from_slice(&bytes)?;
        if user.scope() != &self.tenant || user.id() != user_id {
            return Err(IdentityStoreError::CorruptCredentialOwner);
        }
        Ok(Some(user))
    }

    pub async fn session_by_id(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionRecord>, IdentityStoreError> {
        let key = self.keyspace.application_session_key(session_id.as_str())?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let session: SessionRecord = serde_json::from_slice(&bytes)?;
        if session.scope() != &self.tenant || session.id() != session_id {
            return Err(IdentityStoreError::CorruptSession);
        }
        Ok(Some(session))
    }

    pub async fn session_is_active(
        &self,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<bool, IdentityStoreError> {
        let Some(session) = self.session_by_id(session_id).await? else {
            return Ok(false);
        };
        if session.user_id() != user_id
            || session.status() != SessionStatus::Active
            || now_unix_seconds >= session.expires_at_unix_seconds()
        {
            return Ok(false);
        }
        let Some(user) = self.user_by_id(user_id).await? else {
            return Ok(false);
        };
        Ok(user.status() == AppUserStatus::Active
            && user.session_epoch() == session.user_session_epoch())
    }

    pub async fn sign_out_session(
        &self,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        for _ in 0..MAX_CONCURRENT_RETRIES {
            let session_key = self.keyspace.application_session_key(session_id.as_str())?;
            let session_bytes = self
                .adapter
                .get(&session_key)
                .await?
                .ok_or(IdentityStoreError::SessionNotFound)?;
            let session: SessionRecord = serde_json::from_slice(&session_bytes)?;
            if session.scope() != &self.tenant || session.id() != session_id {
                return Err(IdentityStoreError::CorruptSession);
            }
            if session.user_id() != user_id {
                return Err(IdentityStoreError::SessionOwnerMismatch);
            }
            if session.status() != SessionStatus::Active {
                return Ok(None);
            }
            let prepared = self.prepare_revocation().await?;
            let event = IdentityRevocationEvent {
                scope: self.tenant.clone(),
                sequence: prepared.next_sequence,
                kind: IdentityRevocationKind::SessionSignedOut,
                user_id: user_id.clone(),
                session_id: Some(session_id.clone()),
                user_session_epoch: session.user_session_epoch(),
                occurred_at_unix_seconds: now_unix_seconds,
            };
            let mut batch = prepared.batch(&event)?;
            batch.put(
                &session_key,
                serde_json::to_vec(&session.revoke(now_unix_seconds))?,
            );
            let mut conditions = prepared.conditions();
            conditions.insert(
                0,
                KeyCondition::ValueEquals {
                    key: session_key,
                    value: session_bytes,
                },
            );
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
                return Ok(Some(event));
            }
        }
        Err(IdentityStoreError::ConcurrentIdentityChange)
    }

    pub async fn sign_out_all_sessions(
        &self,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        self.change_user_lifecycle(user_id, UserLifecycleAction::SignOutAll, now_unix_seconds)
            .await
    }

    pub async fn disable_user(
        &self,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        self.change_user_lifecycle(user_id, UserLifecycleAction::Disable, now_unix_seconds)
            .await
    }

    pub async fn restore_user(
        &self,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        self.change_user_lifecycle(user_id, UserLifecycleAction::Restore, now_unix_seconds)
            .await
    }

    pub async fn delete_user(
        &self,
        user_id: &AppUserId,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        self.change_user_lifecycle(user_id, UserLifecycleAction::Delete, now_unix_seconds)
            .await
    }

    async fn change_user_lifecycle(
        &self,
        user_id: &AppUserId,
        action: UserLifecycleAction,
        now_unix_seconds: u64,
    ) -> Result<Option<IdentityRevocationEvent>, IdentityStoreError> {
        for _ in 0..MAX_CONCURRENT_RETRIES {
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
            let (updated, kind) = transition_user(user, action, now_unix_seconds)?;
            let Some((updated, kind)) = updated.zip(kind) else {
                return Ok(None);
            };
            let prepared = self.prepare_revocation().await?;
            let event = IdentityRevocationEvent {
                scope: self.tenant.clone(),
                sequence: prepared.next_sequence,
                kind,
                user_id: user_id.clone(),
                session_id: None,
                user_session_epoch: updated.session_epoch(),
                occurred_at_unix_seconds: now_unix_seconds,
            };
            let mut batch = prepared.batch(&event)?;
            batch.put(&user_key, serde_json::to_vec(&updated)?);
            let mut conditions = prepared.conditions();
            conditions.insert(
                0,
                KeyCondition::ValueEquals {
                    key: user_key,
                    value: user_bytes,
                },
            );
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
                return Ok(Some(event));
            }
        }
        Err(IdentityStoreError::ConcurrentIdentityChange)
    }

    pub async fn latest_revocation_sequence(&self) -> Result<u64, IdentityStoreError> {
        let key = self.keyspace.identity_revocation_sequence_key();
        self.adapter
            .get(&key)
            .await?
            .map_or(Ok(0), |bytes| decode_sequence(&bytes))
    }

    pub async fn revocation_events_after(
        &self,
        sequence: u64,
        limit: NonZeroUsize,
    ) -> Result<Vec<IdentityRevocationEvent>, IdentityStoreError> {
        let Some(range) = self.keyspace.identity_revocations_after(sequence)? else {
            return Ok(Vec::new());
        };
        let entries = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        entries
            .into_iter()
            .map(|entry| {
                let key_sequence = self.keyspace.decode_identity_revocation_key(&entry.key)?;
                let event: IdentityRevocationEvent = serde_json::from_slice(&entry.value)?;
                if event.scope() != &self.tenant || event.sequence() != key_sequence {
                    return Err(IdentityStoreError::CorruptRevocationEvent);
                }
                Ok(event)
            })
            .collect()
    }

    pub(crate) async fn prepare_revocation(
        &self,
    ) -> Result<PreparedRevocation, IdentityStoreError> {
        let sequence_key = self.keyspace.identity_revocation_sequence_key();
        let previous_bytes = self.adapter.get(&sequence_key).await?;
        let previous_sequence = previous_bytes.as_deref().map_or(Ok(0), decode_sequence)?;
        let next_sequence = previous_sequence
            .checked_add(1)
            .ok_or(IdentityStoreError::RevocationSequenceExhausted)?;
        Ok(PreparedRevocation {
            sequence_key,
            previous_bytes,
            next_sequence,
            event_key: self.keyspace.identity_revocation_key(next_sequence),
        })
    }
}

fn transition_user(
    user: AppUserRecord,
    action: UserLifecycleAction,
    now_unix_seconds: u64,
) -> Result<(Option<AppUserRecord>, Option<IdentityRevocationKind>), IdentityStoreError> {
    let status = user.status();
    let changed = match action {
        UserLifecycleAction::Disable if status == AppUserStatus::Disabled => {
            return Ok((None, None));
        }
        UserLifecycleAction::Disable if status == AppUserStatus::Deleted => {
            return Err(IdentityStoreError::InvalidUserStatusTransition);
        }
        UserLifecycleAction::Disable => (
            user.with_status(AppUserStatus::Disabled, now_unix_seconds)
                .revoke_all_sessions(now_unix_seconds)?,
            IdentityRevocationKind::UserDisabled,
        ),
        UserLifecycleAction::Restore if status == AppUserStatus::Active => return Ok((None, None)),
        UserLifecycleAction::Restore if status != AppUserStatus::Disabled => {
            return Err(IdentityStoreError::InvalidUserStatusTransition);
        }
        UserLifecycleAction::Restore => (
            user.with_status(AppUserStatus::Active, now_unix_seconds),
            IdentityRevocationKind::UserRestored,
        ),
        UserLifecycleAction::Delete if status == AppUserStatus::Deleted => return Ok((None, None)),
        UserLifecycleAction::Delete => (
            user.with_status(AppUserStatus::Deleted, now_unix_seconds)
                .revoke_all_sessions(now_unix_seconds)?,
            IdentityRevocationKind::UserDeleted,
        ),
        UserLifecycleAction::SignOutAll => (
            user.revoke_all_sessions(now_unix_seconds)?,
            IdentityRevocationKind::AllSessionsSignedOut,
        ),
    };
    Ok((Some(changed.0), Some(changed.1)))
}

pub(crate) struct PreparedRevocation {
    sequence_key: Vec<u8>,
    previous_bytes: Option<Vec<u8>>,
    next_sequence: u64,
    event_key: Vec<u8>,
}

impl PreparedRevocation {
    pub(crate) fn conditions(&self) -> Vec<KeyCondition> {
        let sequence = match &self.previous_bytes {
            Some(value) => KeyCondition::ValueEquals {
                key: self.sequence_key.clone(),
                value: value.clone(),
            },
            None => KeyCondition::Missing {
                key: self.sequence_key.clone(),
            },
        };
        vec![
            sequence,
            KeyCondition::Missing {
                key: self.event_key.clone(),
            },
        ]
    }

    pub(crate) fn event(
        &self,
        scope: TenantScope,
        kind: IdentityRevocationKind,
        user_id: AppUserId,
        session_id: Option<SessionId>,
        user_session_epoch: u64,
        occurred_at_unix_seconds: u64,
    ) -> IdentityRevocationEvent {
        IdentityRevocationEvent {
            scope,
            sequence: self.next_sequence,
            kind,
            user_id,
            session_id,
            user_session_epoch,
            occurred_at_unix_seconds,
        }
    }

    pub(crate) fn batch(
        &self,
        event: &IdentityRevocationEvent,
    ) -> Result<WriteBatch, IdentityStoreError> {
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&self.sequence_key, self.next_sequence.to_be_bytes());
        batch.put(&self.event_key, serde_json::to_vec(event)?);
        Ok(batch)
    }
}

fn decode_sequence(bytes: &[u8]) -> Result<u64, IdentityStoreError> {
    let sequence: [u8; 8] = bytes
        .try_into()
        .map_err(|_| IdentityStoreError::CorruptRevocationEvent)?;
    Ok(u64::from_be_bytes(sequence))
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, sync::Arc};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::{
        IdentityProvider, NormalizedEmail, TrustedAppMetadata, UserIdentityId, UserIdentityRecord,
        UserProfileMetadata,
    };

    #[test]
    fn lifecycle_changes_revoke_sessions_and_publish_one_ordered_stream() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let user_id = AppUserId::parse("usr_abcdefgh").expect("user");
            create_user(&store, &tenant, &user_id).await;

            let first_session = SessionRecord::new(
                tenant.clone(),
                SessionId::parse("ses_first").expect("session"),
                user_id.clone(),
                1,
                1_000,
                0,
            );
            store.create_session(&first_session).await.expect("session");
            assert!(
                store
                    .session_is_active(&user_id, first_session.id(), 10)
                    .await
                    .expect("active")
            );
            let first = store
                .sign_out_session(&user_id, first_session.id(), 11)
                .await
                .expect("sign out")
                .expect("event");
            assert_eq!(first.sequence(), 1);
            assert_eq!(first.kind(), IdentityRevocationKind::SessionSignedOut);
            assert!(
                !store
                    .session_is_active(&user_id, first_session.id(), 12)
                    .await
                    .expect("inactive")
            );

            let second_session = SessionRecord::new(
                tenant.clone(),
                SessionId::parse("ses_second").expect("session"),
                user_id.clone(),
                12,
                1_000,
                0,
            );
            store
                .create_session(&second_session)
                .await
                .expect("second session");
            assert_eq!(
                store
                    .sign_out_all_sessions(&user_id, 13)
                    .await
                    .expect("all sign out")
                    .expect("event")
                    .sequence(),
                2
            );
            assert!(
                !store
                    .session_is_active(&user_id, second_session.id(), 14)
                    .await
                    .expect("epoch revoked")
            );
            assert_eq!(
                store
                    .disable_user(&user_id, 15)
                    .await
                    .expect("disable")
                    .expect("event")
                    .sequence(),
                3
            );
            assert!(
                store
                    .disable_user(&user_id, 16)
                    .await
                    .expect("idempotent disable")
                    .is_none()
            );
            assert_eq!(
                store
                    .restore_user(&user_id, 17)
                    .await
                    .expect("restore")
                    .expect("event")
                    .sequence(),
                4
            );
            assert_eq!(
                store
                    .delete_user(&user_id, 18)
                    .await
                    .expect("delete")
                    .expect("event")
                    .sequence(),
                5
            );
            assert!(matches!(
                store.restore_user(&user_id, 19).await,
                Err(IdentityStoreError::InvalidUserStatusTransition)
            ));

            let events = store
                .revocation_events_after(0, NonZeroUsize::new(10).expect("limit"))
                .await
                .expect("events");
            assert_eq!(
                events
                    .iter()
                    .map(IdentityRevocationEvent::sequence)
                    .collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5]
            );
            assert_eq!(store.latest_revocation_sequence().await.expect("latest"), 5);
            assert_eq!(
                store
                    .user_by_id(&user_id)
                    .await
                    .expect("user")
                    .expect("exists")
                    .session_epoch(),
                3
            );
        });
    }

    async fn create_user(store: &IdentityStore, tenant: &TenantScope, user_id: &AppUserId) {
        let email = NormalizedEmail::parse("person@example.com").expect("email");
        let user = AppUserRecord::new(
            tenant.clone(),
            user_id.clone(),
            AppUserStatus::Active,
            TrustedAppMetadata::new(json!({})).expect("trusted"),
            UserProfileMetadata::new(json!({})).expect("profile"),
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
        .expect("identity");
        store
            .create_email_user(&user, &identity, &email)
            .await
            .expect("user");
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
