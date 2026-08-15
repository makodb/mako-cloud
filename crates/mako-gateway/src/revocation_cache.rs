use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::Mutex,
};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_identity::{
    AppUserId, AppUserStatus, IdentityRevocationEvent, IdentityRevocationKind, IdentityStore,
    SessionId, SessionStatus,
};

use crate::GatewaySessionResolver;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevocationSourceError {
    Unavailable,
    Corrupt,
    TenantMismatch,
}

impl fmt::Display for RevocationSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "revocation source is unavailable",
            Self::Corrupt => "revocation source returned corrupt state",
            Self::TenantMismatch => "revocation source tenant does not match",
        })
    }
}

impl Error for RevocationSourceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthoritativeSessionState {
    session_exists: bool,
    session_active: bool,
    session_epoch: u64,
    user_active: bool,
    user_session_epoch: u64,
}

impl AuthoritativeSessionState {
    #[must_use]
    pub const fn new(
        session_exists: bool,
        session_active: bool,
        session_epoch: u64,
        user_active: bool,
        user_session_epoch: u64,
    ) -> Self {
        Self {
            session_exists,
            session_active,
            session_epoch,
            user_active,
            user_session_epoch,
        }
    }

    #[must_use]
    pub const fn missing() -> Self {
        Self::new(false, false, 0, false, 0)
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        self.session_exists
            && self.session_active
            && self.user_active
            && self.session_epoch == self.user_session_epoch
    }
}

#[async_trait]
pub trait RevocationSource: Send + Sync {
    async fn latest_sequence(&self, tenant: &TenantScope) -> Result<u64, RevocationSourceError>;

    async fn events_after(
        &self,
        tenant: &TenantScope,
        sequence: u64,
        limit: NonZeroUsize,
    ) -> Result<Vec<IdentityRevocationEvent>, RevocationSourceError>;

    async fn authoritative_session(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<AuthoritativeSessionState, RevocationSourceError>;
}

#[derive(Debug)]
pub struct RevocationCache {
    tenant: TenantScope,
    maximum_freshness_seconds: u64,
    event_batch_size: NonZeroUsize,
    state: Mutex<CacheState>,
}

impl RevocationCache {
    pub fn new(
        tenant: TenantScope,
        maximum_freshness_seconds: u64,
        event_batch_size: NonZeroUsize,
    ) -> Result<Self, RevocationCacheError> {
        if maximum_freshness_seconds == 0 || maximum_freshness_seconds > 300 {
            return Err(RevocationCacheError::InvalidConfiguration);
        }
        Ok(Self {
            tenant,
            maximum_freshness_seconds,
            event_batch_size,
            state: Mutex::new(CacheState::default()),
        })
    }

    pub async fn refresh(
        &self,
        source: &dyn RevocationSource,
        now_unix_seconds: u64,
    ) -> Result<(), RevocationCacheError> {
        let starting_sequence = self
            .state
            .lock()
            .map_err(|_| RevocationCacheError::Internal)?
            .applied_sequence;
        let latest = match source.latest_sequence(&self.tenant).await {
            Ok(latest) => latest,
            Err(error) => return self.fail_closed(error.into()),
        };
        if latest < starting_sequence {
            return self.fail_closed(RevocationCacheError::SourceRegressed {
                cached: starting_sequence,
                source: latest,
            });
        }

        let mut cursor = starting_sequence;
        let mut pending = Vec::new();
        while cursor < latest {
            let events = match source
                .events_after(&self.tenant, cursor, self.event_batch_size)
                .await
            {
                Ok(events) => events,
                Err(error) => return self.fail_closed(error.into()),
            };
            let Some(first) = events.first() else {
                return self.fail_closed(RevocationCacheError::EventGap {
                    expected: cursor.saturating_add(1),
                    actual: None,
                });
            };
            if first.sequence() != cursor.saturating_add(1) {
                return self.fail_closed(RevocationCacheError::EventGap {
                    expected: cursor.saturating_add(1),
                    actual: Some(first.sequence()),
                });
            }
            for event in events {
                if event.sequence() > latest {
                    break;
                }
                let expected = cursor
                    .checked_add(1)
                    .ok_or(RevocationCacheError::SequenceExhausted)?;
                if event.scope() != &self.tenant || event.sequence() != expected {
                    return self.fail_closed(RevocationCacheError::EventGap {
                        expected,
                        actual: Some(event.sequence()),
                    });
                }
                cursor = event.sequence();
                pending.push(event);
            }
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| RevocationCacheError::Internal)?;
        if state.applied_sequence != starting_sequence {
            return Ok(());
        }
        for event in pending {
            state.apply(event);
        }
        state.last_proven_fresh_at_unix_seconds = Some(now_unix_seconds);
        state.freshness_proven = true;
        Ok(())
    }

    pub async fn check_session(
        &self,
        source: &dyn RevocationSource,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<bool, RevocationCacheError> {
        self.refresh(source, now_unix_seconds).await?;
        if let Some(active) = self.cached_session(user_id, session_id, now_unix_seconds)? {
            return Ok(active);
        }
        let authoritative = match source
            .authoritative_session(&self.tenant, user_id, session_id, now_unix_seconds)
            .await
        {
            Ok(state) => state,
            Err(error) => return self.fail_closed(error.into()),
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| RevocationCacheError::Internal)?;
        if !state.is_fresh(now_unix_seconds, self.maximum_freshness_seconds) {
            return Err(RevocationCacheError::FreshnessUnproven);
        }
        state.users.insert(
            user_id.clone(),
            CachedUser {
                session_epoch: authoritative.user_session_epoch,
                blocked: !authoritative.user_active,
            },
        );
        state.sessions.insert(
            session_id.clone(),
            CachedSession {
                user_id: user_id.clone(),
                session_epoch: authoritative.session_epoch,
                active: authoritative.session_exists && authoritative.session_active,
            },
        );
        Ok(state.session_is_active(user_id, session_id))
    }

    #[must_use]
    pub fn applied_sequence(&self) -> Option<u64> {
        self.state.lock().ok().map(|state| state.applied_sequence)
    }

    fn cached_session(
        &self,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<Option<bool>, RevocationCacheError> {
        let state = self
            .state
            .lock()
            .map_err(|_| RevocationCacheError::Internal)?;
        if !state.is_fresh(now_unix_seconds, self.maximum_freshness_seconds) {
            return Err(RevocationCacheError::FreshnessUnproven);
        }
        Ok(state
            .sessions
            .contains_key(session_id)
            .then(|| state.session_is_active(user_id, session_id)))
    }

    fn fail_closed<T>(&self, error: RevocationCacheError) -> Result<T, RevocationCacheError> {
        if let Ok(mut state) = self.state.lock() {
            state.freshness_proven = false;
        }
        Err(error)
    }
}

impl GatewaySessionResolver for RevocationCache {
    fn is_active(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> bool {
        if tenant != &self.tenant {
            return false;
        }
        self.cached_session(user_id, session_id, now_unix_seconds)
            .ok()
            .flatten()
            .unwrap_or(false)
    }
}

#[derive(Debug, Default)]
struct CacheState {
    applied_sequence: u64,
    last_proven_fresh_at_unix_seconds: Option<u64>,
    freshness_proven: bool,
    users: HashMap<AppUserId, CachedUser>,
    sessions: HashMap<SessionId, CachedSession>,
    revoked_sessions: HashSet<SessionId>,
}

impl CacheState {
    fn apply(&mut self, event: IdentityRevocationEvent) {
        let user = self
            .users
            .entry(event.user_id().clone())
            .or_insert(CachedUser {
                session_epoch: event.user_session_epoch(),
                blocked: false,
            });
        user.session_epoch = event.user_session_epoch();
        match event.kind() {
            IdentityRevocationKind::UserDisabled | IdentityRevocationKind::UserDeleted => {
                user.blocked = true;
            }
            IdentityRevocationKind::UserRestored => user.blocked = false,
            IdentityRevocationKind::SessionSignedOut => {
                if let Some(session_id) = event.session_id() {
                    self.revoked_sessions.insert(session_id.clone());
                }
            }
            IdentityRevocationKind::AllSessionsSignedOut
            | IdentityRevocationKind::PasswordChanged => {}
        }
        self.applied_sequence = event.sequence();
    }

    fn is_fresh(&self, now_unix_seconds: u64, maximum_freshness_seconds: u64) -> bool {
        if !self.freshness_proven {
            return false;
        }
        self.last_proven_fresh_at_unix_seconds.is_some_and(|last| {
            now_unix_seconds >= last && now_unix_seconds - last <= maximum_freshness_seconds
        })
    }

    fn session_is_active(&self, user_id: &AppUserId, session_id: &SessionId) -> bool {
        let Some(user) = self.users.get(user_id) else {
            return false;
        };
        let Some(session) = self.sessions.get(session_id) else {
            return false;
        };
        !user.blocked
            && !self.revoked_sessions.contains(session_id)
            && session.active
            && &session.user_id == user_id
            && session.session_epoch == user.session_epoch
    }
}

#[derive(Debug)]
struct CachedUser {
    session_epoch: u64,
    blocked: bool,
}

#[derive(Debug)]
struct CachedSession {
    user_id: AppUserId,
    session_epoch: u64,
    active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevocationCacheError {
    InvalidConfiguration,
    Source(RevocationSourceError),
    SourceRegressed { cached: u64, source: u64 },
    EventGap { expected: u64, actual: Option<u64> },
    SequenceExhausted,
    FreshnessUnproven,
    Internal,
}

impl fmt::Display for RevocationCacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid revocation-cache configuration")
            }
            Self::Source(error) => error.fmt(formatter),
            Self::SourceRegressed { cached, source } => write!(
                formatter,
                "revocation source sequence regressed from {cached} to {source}"
            ),
            Self::EventGap { expected, actual } => write!(
                formatter,
                "revocation stream gap: expected {expected}, received {actual:?}"
            ),
            Self::SequenceExhausted => formatter.write_str("revocation sequence is exhausted"),
            Self::FreshnessUnproven => formatter.write_str("revocation freshness cannot be proven"),
            Self::Internal => formatter.write_str("revocation cache is unavailable"),
        }
    }
}

impl Error for RevocationCacheError {}

impl From<RevocationSourceError> for RevocationCacheError {
    fn from(error: RevocationSourceError) -> Self {
        Self::Source(error)
    }
}

#[async_trait]
impl RevocationSource for IdentityStore {
    async fn latest_sequence(&self, tenant: &TenantScope) -> Result<u64, RevocationSourceError> {
        if self.tenant() != tenant {
            return Err(RevocationSourceError::TenantMismatch);
        }
        self.latest_revocation_sequence()
            .await
            .map_err(|_| RevocationSourceError::Unavailable)
    }

    async fn events_after(
        &self,
        tenant: &TenantScope,
        sequence: u64,
        limit: NonZeroUsize,
    ) -> Result<Vec<IdentityRevocationEvent>, RevocationSourceError> {
        if self.tenant() != tenant {
            return Err(RevocationSourceError::TenantMismatch);
        }
        self.revocation_events_after(sequence, limit)
            .await
            .map_err(|_| RevocationSourceError::Unavailable)
    }

    async fn authoritative_session(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
        session_id: &SessionId,
        now_unix_seconds: u64,
    ) -> Result<AuthoritativeSessionState, RevocationSourceError> {
        if self.tenant() != tenant {
            return Err(RevocationSourceError::TenantMismatch);
        }
        let session = self
            .session_by_id(session_id)
            .await
            .map_err(|_| RevocationSourceError::Unavailable)?;
        let user = self
            .user_by_id(user_id)
            .await
            .map_err(|_| RevocationSourceError::Unavailable)?;
        let (Some(session), Some(user)) = (session, user) else {
            return Ok(AuthoritativeSessionState::missing());
        };
        if session.user_id() != user_id {
            return Ok(AuthoritativeSessionState::missing());
        }
        Ok(AuthoritativeSessionState::new(
            true,
            session.status() == SessionStatus::Active
                && now_unix_seconds < session.expires_at_unix_seconds(),
            session.user_session_epoch(),
            user.status() == AppUserStatus::Active,
            user.session_epoch(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[derive(Default)]
    struct SourceState {
        events: Vec<IdentityRevocationEvent>,
        authoritative: HashMap<(AppUserId, SessionId), AuthoritativeSessionState>,
        unavailable: bool,
    }

    #[derive(Default)]
    struct FakeSource(Mutex<SourceState>);

    #[async_trait]
    impl RevocationSource for FakeSource {
        async fn latest_sequence(&self, _: &TenantScope) -> Result<u64, RevocationSourceError> {
            let state = self.0.lock().expect("state");
            if state.unavailable {
                return Err(RevocationSourceError::Unavailable);
            }
            Ok(state
                .events
                .last()
                .map_or(0, IdentityRevocationEvent::sequence))
        }

        async fn events_after(
            &self,
            _: &TenantScope,
            sequence: u64,
            limit: NonZeroUsize,
        ) -> Result<Vec<IdentityRevocationEvent>, RevocationSourceError> {
            let state = self.0.lock().expect("state");
            if state.unavailable {
                return Err(RevocationSourceError::Unavailable);
            }
            Ok(state
                .events
                .iter()
                .filter(|event| event.sequence() > sequence)
                .take(limit.get())
                .cloned()
                .collect())
        }

        async fn authoritative_session(
            &self,
            _: &TenantScope,
            user_id: &AppUserId,
            session_id: &SessionId,
            _: u64,
        ) -> Result<AuthoritativeSessionState, RevocationSourceError> {
            let state = self.0.lock().expect("state");
            if state.unavailable {
                return Err(RevocationSourceError::Unavailable);
            }
            Ok(state
                .authoritative
                .get(&(user_id.clone(), session_id.clone()))
                .copied()
                .unwrap_or_else(AuthoritativeSessionState::missing))
        }
    }

    #[test]
    fn ordered_events_revoke_cached_sessions_and_stale_or_failed_caches_deny() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let user_id = AppUserId::parse("usr_abcdefgh").expect("user");
            let session_id = SessionId::parse("ses_abcdefgh").expect("session");
            let source = FakeSource::default();
            source.0.lock().expect("state").authoritative.insert(
                (user_id.clone(), session_id.clone()),
                AuthoritativeSessionState::new(true, true, 0, true, 0),
            );
            let cache =
                RevocationCache::new(tenant.clone(), 5, NonZeroUsize::new(2).expect("batch"))
                    .expect("cache");

            assert!(
                cache
                    .check_session(&source, &user_id, &session_id, 10)
                    .await
                    .expect("active")
            );
            assert!(cache.is_active(&tenant, &user_id, &session_id, 15));
            assert!(!cache.is_active(&tenant, &user_id, &session_id, 16));

            source.0.lock().expect("state").events.push(event(
                &tenant,
                1,
                IdentityRevocationKind::SessionSignedOut,
                &user_id,
                Some(&session_id),
                0,
            ));
            assert!(
                !cache
                    .check_session(&source, &user_id, &session_id, 17)
                    .await
                    .expect("revoked")
            );
            assert_eq!(cache.applied_sequence(), Some(1));

            source.0.lock().expect("state").unavailable = true;
            assert!(matches!(
                cache.refresh(&source, 18).await,
                Err(RevocationCacheError::Source(
                    RevocationSourceError::Unavailable
                ))
            ));
            assert!(!cache.is_active(&tenant, &user_id, &session_id, 18));
        });
    }

    #[test]
    fn event_gaps_fail_closed_instead_of_advancing_freshness() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let user_id = AppUserId::parse("usr_abcdefgh").expect("user");
            let source = FakeSource::default();
            source.0.lock().expect("state").events.push(event(
                &tenant,
                2,
                IdentityRevocationKind::UserDisabled,
                &user_id,
                None,
                1,
            ));
            let cache = RevocationCache::new(tenant, 5, NonZeroUsize::new(2).expect("batch"))
                .expect("cache");

            assert!(matches!(
                cache.refresh(&source, 10).await,
                Err(RevocationCacheError::EventGap {
                    expected: 1,
                    actual: Some(2)
                })
            ));
            assert_eq!(cache.applied_sequence(), Some(0));
        });
    }

    fn event(
        tenant: &TenantScope,
        sequence: u64,
        kind: IdentityRevocationKind,
        user_id: &AppUserId,
        session_id: Option<&SessionId>,
        epoch: u64,
    ) -> IdentityRevocationEvent {
        let encoded = serde_json::json!({
            "scope": tenant,
            "sequence": sequence,
            "kind": kind,
            "userId": user_id,
            "sessionId": session_id,
            "userSessionEpoch": epoch,
            "occurredAtUnixSeconds": 10
        });
        serde_json::from_value(encoded).expect("event")
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
