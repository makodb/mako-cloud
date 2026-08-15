use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroUsize};

use async_trait::async_trait;
use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, KeyCondition, ScanDirection, ScanRequest, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    AppUserId, AppUserRecord, AppUserStatus, IdentityProvider, IdentityStore, IdentityStoreError,
    NormalizedEmail, SessionId, SessionStatus, TrustedAppMetadata, UserIdentityId,
    UserIdentityRecord, UserProfileMetadata,
};

const MAX_SEARCH_RESULTS: usize = 100;
const MAX_SEARCH_SCAN: usize = 1_000;
const MAX_SESSION_RESULTS: usize = 100;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AdminUserPermission {
    Read,
    Create,
    UpdateMetadata,
    ManageLifecycle,
    RevokeSessions,
    Delete,
}

#[derive(Clone, Debug)]
pub struct AdminRequestContext {
    tenant: TenantScope,
    actor_id: String,
    request_id: String,
    permissions: BTreeSet<AdminUserPermission>,
    occurred_at_unix_seconds: u64,
}

impl AdminRequestContext {
    pub fn new(
        tenant: TenantScope,
        actor_id: impl Into<String>,
        request_id: impl Into<String>,
        permissions: impl IntoIterator<Item = AdminUserPermission>,
        occurred_at_unix_seconds: u64,
    ) -> Result<Self, AdminUserApiError> {
        let actor_id = actor_id.into();
        let request_id = request_id.into();
        if !valid_audit_identifier(&actor_id) || !valid_audit_identifier(&request_id) {
            return Err(AdminUserApiError::InvalidRequest);
        }
        Ok(Self {
            tenant,
            actor_id,
            request_id,
            permissions: permissions.into_iter().collect(),
            occurred_at_unix_seconds,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminUserAction {
    Search,
    Inspect,
    Create,
    Invite,
    UpdateMetadata,
    Disable,
    Restore,
    RevokeSession,
    RevokeAllSessions,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminAuditOutcome {
    Succeeded,
    Denied,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminUserAuditEvent {
    tenant: TenantScope,
    actor_id: String,
    action: AdminUserAction,
    target_id: String,
    outcome: AdminAuditOutcome,
    request_id: String,
    occurred_at_unix_seconds: u64,
}

impl AdminUserAuditEvent {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    #[must_use]
    pub fn action(&self) -> AdminUserAction {
        self.action
    }

    #[must_use]
    pub const fn outcome(&self) -> AdminAuditOutcome {
        self.outcome
    }

    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn occurred_at_unix_seconds(&self) -> u64 {
        self.occurred_at_unix_seconds
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminUserAuditError(String);

impl AdminUserAuditError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AdminUserAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for AdminUserAuditError {}

pub trait AdminUserAuditSink: Send + Sync {
    fn record(&self, event: &AdminUserAuditEvent) -> Result<(), AdminUserAuditError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationUserInvitation {
    tenant: TenantScope,
    user_id: AppUserId,
    recipient: NormalizedEmail,
}

impl ApplicationUserInvitation {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn recipient(&self) -> &NormalizedEmail {
        &self.recipient
    }
}

pub trait ApplicationUserInvitationSink: Send + Sync {
    fn enqueue(&self, invitation: ApplicationUserInvitation);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedMetadataInvalidationError(String);

impl TrustedMetadataInvalidationError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TrustedMetadataInvalidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for TrustedMetadataInvalidationError {}

#[async_trait]
pub trait TrustedMetadataInvalidationSink: Send + Sync {
    async fn trusted_metadata_changed(
        &self,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Result<(), TrustedMetadataInvalidationError>;
}

#[derive(Clone, Debug)]
pub struct AdminCreateUserRequest {
    pub email: NormalizedEmail,
    pub trusted_metadata: TrustedAppMetadata,
    pub profile_metadata: UserProfileMetadata,
}

#[derive(Clone, Debug)]
pub struct AdminUpdateUserMetadataRequest {
    pub trusted_metadata: TrustedAppMetadata,
    pub profile_metadata: UserProfileMetadata,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdminUserSummary {
    id: AppUserId,
    email: Option<NormalizedEmail>,
    status: AppUserStatus,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl AdminUserSummary {
    #[must_use]
    pub fn id(&self) -> &AppUserId {
        &self.id
    }

    #[must_use]
    pub fn email(&self) -> Option<&NormalizedEmail> {
        self.email.as_ref()
    }

    #[must_use]
    pub const fn status(&self) -> AppUserStatus {
        self.status
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdminSessionSummary {
    id: SessionId,
    status: SessionStatus,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdminUserView {
    user: AppUserRecord,
    email: Option<NormalizedEmail>,
    sessions: Vec<AdminSessionSummary>,
    sessions_truncated: bool,
}

impl AdminUserView {
    #[must_use]
    pub fn user(&self) -> &AppUserRecord {
        &self.user
    }

    #[must_use]
    pub fn email(&self) -> Option<&NormalizedEmail> {
        self.email.as_ref()
    }

    #[must_use]
    pub fn sessions(&self) -> &[AdminSessionSummary] {
        &self.sessions
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdminUserSearchPage {
    pub users: Vec<AdminUserSummary>,
    pub truncated: bool,
}

pub struct AdminUserService<'a> {
    store: &'a IdentityStore,
    audit: &'a dyn AdminUserAuditSink,
    invitations: &'a dyn ApplicationUserInvitationSink,
    invalidations: &'a dyn TrustedMetadataInvalidationSink,
}

impl<'a> AdminUserService<'a> {
    #[must_use]
    pub const fn new(
        store: &'a IdentityStore,
        audit: &'a dyn AdminUserAuditSink,
        invitations: &'a dyn ApplicationUserInvitationSink,
        invalidations: &'a dyn TrustedMetadataInvalidationSink,
    ) -> Self {
        Self {
            store,
            audit,
            invitations,
            invalidations,
        }
    }

    pub async fn search(
        &self,
        context: &AdminRequestContext,
        query: Option<&str>,
        limit: usize,
    ) -> Result<AdminUserSearchPage, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::Read,
            AdminUserAction::Search,
            "users",
        )?;
        let result = self.search_impl(query, limit).await;
        self.finish(context, AdminUserAction::Search, "users", result)
    }

    pub async fn inspect(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::Read,
            AdminUserAction::Inspect,
            user_id.as_str(),
        )?;
        let result = self.inspect_impl(user_id).await;
        self.finish(context, AdminUserAction::Inspect, user_id.as_str(), result)
    }

    pub async fn create(
        &self,
        context: &AdminRequestContext,
        request: AdminCreateUserRequest,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::Create,
            AdminUserAction::Create,
            request.email.as_str(),
        )?;
        let target = request.email.as_str().to_owned();
        let result = match self
            .create_impl(
                request,
                AppUserStatus::Active,
                context.occurred_at_unix_seconds,
            )
            .await
        {
            Ok(user) => self.inspect_impl(user.id()).await,
            Err(error) => Err(error),
        };
        self.finish(context, AdminUserAction::Create, &target, result)
    }

    pub async fn invite(
        &self,
        context: &AdminRequestContext,
        request: AdminCreateUserRequest,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::Create,
            AdminUserAction::Invite,
            request.email.as_str(),
        )?;
        let target = request.email.as_str().to_owned();
        let recipient = request.email.clone();
        let result = match self
            .create_impl(
                request,
                AppUserStatus::PendingVerification,
                context.occurred_at_unix_seconds,
            )
            .await
        {
            Ok(user) => {
                self.invitations.enqueue(ApplicationUserInvitation {
                    tenant: self.store.tenant().clone(),
                    user_id: user.id().clone(),
                    recipient,
                });
                self.inspect_impl(user.id()).await
            }
            Err(error) => Err(error),
        };
        self.finish(context, AdminUserAction::Invite, &target, result)
    }

    pub async fn update_metadata(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
        request: AdminUpdateUserMetadataRequest,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::UpdateMetadata,
            AdminUserAction::UpdateMetadata,
            user_id.as_str(),
        )?;
        let result = self
            .update_metadata_impl(user_id, request, context.occurred_at_unix_seconds)
            .await;
        let result = match result {
            Ok(()) => self.inspect_impl(user_id).await.map_err(Into::into),
            Err(error) => Err(error),
        };
        self.finish(
            context,
            AdminUserAction::UpdateMetadata,
            user_id.as_str(),
            result,
        )
    }

    pub async fn disable(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::ManageLifecycle,
            AdminUserAction::Disable,
            user_id.as_str(),
        )?;
        let result = match self
            .store
            .disable_user(user_id, context.occurred_at_unix_seconds)
            .await
        {
            Ok(_) => self.inspect_impl(user_id).await,
            Err(error) => Err(error),
        };
        self.finish(context, AdminUserAction::Disable, user_id.as_str(), result)
    }

    pub async fn restore(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::ManageLifecycle,
            AdminUserAction::Restore,
            user_id.as_str(),
        )?;
        let result = match self
            .store
            .restore_user(user_id, context.occurred_at_unix_seconds)
            .await
        {
            Ok(_) => self.inspect_impl(user_id).await,
            Err(error) => Err(error),
        };
        self.finish(context, AdminUserAction::Restore, user_id.as_str(), result)
    }

    pub async fn revoke_session(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
        session_id: &SessionId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::RevokeSessions,
            AdminUserAction::RevokeSession,
            session_id.as_str(),
        )?;
        let result = match self
            .store
            .sign_out_session(user_id, session_id, context.occurred_at_unix_seconds)
            .await
        {
            Ok(_) => self.inspect_impl(user_id).await,
            Err(error) => Err(error),
        };
        self.finish(
            context,
            AdminUserAction::RevokeSession,
            session_id.as_str(),
            result,
        )
    }

    pub async fn revoke_all_sessions(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::RevokeSessions,
            AdminUserAction::RevokeAllSessions,
            user_id.as_str(),
        )?;
        let result = match self
            .store
            .sign_out_all_sessions(user_id, context.occurred_at_unix_seconds)
            .await
        {
            Ok(_) => self.inspect_impl(user_id).await,
            Err(error) => Err(error),
        };
        self.finish(
            context,
            AdminUserAction::RevokeAllSessions,
            user_id.as_str(),
            result,
        )
    }

    pub async fn delete(
        &self,
        context: &AdminRequestContext,
        user_id: &AppUserId,
    ) -> Result<AdminUserView, AdminUserApiError> {
        self.authorize(
            context,
            AdminUserPermission::Delete,
            AdminUserAction::Delete,
            user_id.as_str(),
        )?;
        let result = match self
            .store
            .delete_user(user_id, context.occurred_at_unix_seconds)
            .await
        {
            Ok(_) => self.inspect_impl(user_id).await,
            Err(error) => Err(error),
        };
        self.finish(context, AdminUserAction::Delete, user_id.as_str(), result)
    }

    async fn create_impl(
        &self,
        request: AdminCreateUserRequest,
        status: AppUserStatus,
        now_unix_seconds: u64,
    ) -> Result<AppUserRecord, IdentityStoreError> {
        let user_id = AppUserId::parse(random_id("usr"))?;
        let user = AppUserRecord::new(
            self.store.tenant().clone(),
            user_id.clone(),
            status,
            request.trusted_metadata,
            request.profile_metadata,
            now_unix_seconds,
        );
        let identity = UserIdentityRecord::new(
            self.store.tenant().clone(),
            UserIdentityId::parse(random_id("idn"))?,
            user_id,
            IdentityProvider::Email,
            request.email.as_str(),
            now_unix_seconds,
        )?;
        self.store
            .create_email_user(&user, &identity, &request.email)
            .await?;
        Ok(user)
    }

    async fn update_metadata_impl(
        &self,
        user_id: &AppUserId,
        request: AdminUpdateUserMetadataRequest,
        now_unix_seconds: u64,
    ) -> Result<(), AdminUserApiError> {
        let user_key = self.store.keyspace.application_user_key(user_id.as_str())?;
        let user_bytes = self
            .store
            .adapter
            .get(&user_key)
            .await?
            .ok_or(IdentityStoreError::UserNotFound)?;
        let user: AppUserRecord = serde_json::from_slice(&user_bytes)?;
        if user.scope() != self.store.tenant() || user.id() != user_id {
            return Err(IdentityStoreError::CorruptCredentialOwner.into());
        }
        if user.trusted_metadata() != &request.trusted_metadata {
            self.invalidations
                .trusted_metadata_changed(self.store.tenant(), user_id)
                .await?;
        }
        let updated = user.with_metadata(
            request.trusted_metadata,
            request.profile_metadata,
            now_unix_seconds,
        );
        let mut batch = WriteBatch::new();
        batch.put(&user_key, serde_json::to_vec(&updated)?);
        if self
            .store
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key: user_key,
                    value: user_bytes,
                }],
                batch,
                durability: self.store.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::ConcurrentIdentityChange.into());
        }
        Ok(())
    }

    async fn inspect_impl(&self, user_id: &AppUserId) -> Result<AdminUserView, IdentityStoreError> {
        let user = self
            .store
            .user_by_id(user_id)
            .await?
            .ok_or(IdentityStoreError::UserNotFound)?;
        let email = self.email_for_user(user_id).await?;
        let (sessions, sessions_truncated) = self.sessions_for_user(user_id).await?;
        Ok(AdminUserView {
            user,
            email,
            sessions,
            sessions_truncated,
        })
    }

    async fn search_impl(
        &self,
        query: Option<&str>,
        limit: usize,
    ) -> Result<AdminUserSearchPage, IdentityStoreError> {
        if limit == 0 || limit > MAX_SEARCH_RESULTS {
            return Err(IdentityStoreError::InvalidAdminQuery);
        }
        let normalized_query = query.map(str::trim).filter(|value| !value.is_empty());
        let mut users = Vec::new();
        if let Some(query) = normalized_query {
            if let Ok(user_id) = AppUserId::parse(query) {
                if let Some(user) = self.store.user_by_id(&user_id).await? {
                    users.push(self.summary(user).await?);
                }
            } else {
                let range = self.store.keyspace.normalized_email_owners_range()?;
                let scan_limit = self
                    .store
                    .adapter
                    .capabilities()
                    .maximum_scan_items
                    .get()
                    .min(MAX_SEARCH_SCAN);
                let entries = self
                    .store
                    .adapter
                    .scan(ScanRequest::new(
                        range,
                        ScanDirection::Forward,
                        NonZeroUsize::new(scan_limit).expect("adapter scan limit is non-zero"),
                    ))
                    .await?;
                let query = query.to_ascii_lowercase();
                for entry in entries {
                    let email = String::from_utf8(
                        self.store
                            .keyspace
                            .decode_normalized_email_owner_key(&entry.key)?,
                    )
                    .map_err(|_| IdentityStoreError::CorruptEmailOwner)?;
                    if email.contains(&query) {
                        let user_id = std::str::from_utf8(&entry.value)
                            .map_err(|_| IdentityStoreError::CorruptEmailOwner)?;
                        let user_id = AppUserId::parse(user_id)?;
                        if let Some(user) = self.store.user_by_id(&user_id).await? {
                            users.push(self.summary(user).await?);
                        }
                        if users.len() > limit {
                            break;
                        }
                    }
                }
            }
        } else {
            let range = self.store.keyspace.application_users_range()?;
            let requested = limit
                .saturating_add(1)
                .min(self.store.adapter.capabilities().maximum_scan_items.get());
            let entries = self
                .store
                .adapter
                .scan(ScanRequest::new(
                    range,
                    ScanDirection::Forward,
                    NonZeroUsize::new(requested).expect("positive bounded search"),
                ))
                .await?;
            for entry in entries {
                let key_user_id = self
                    .store
                    .keyspace
                    .decode_application_user_key(&entry.key)?;
                let user: AppUserRecord = serde_json::from_slice(&entry.value)?;
                if user.scope() != self.store.tenant()
                    || user.id().as_str().as_bytes() != key_user_id
                {
                    return Err(IdentityStoreError::CorruptCredentialOwner);
                }
                users.push(self.summary(user).await?);
            }
        }
        let truncated = users.len() > limit;
        users.truncate(limit);
        Ok(AdminUserSearchPage { users, truncated })
    }

    async fn summary(&self, user: AppUserRecord) -> Result<AdminUserSummary, IdentityStoreError> {
        Ok(AdminUserSummary {
            email: self.email_for_user(user.id()).await?,
            id: user.id().clone(),
            status: user.status(),
            created_at_unix_seconds: user.created_at_unix_seconds(),
            updated_at_unix_seconds: user.updated_at_unix_seconds(),
        })
    }

    async fn email_for_user(
        &self,
        user_id: &AppUserId,
    ) -> Result<Option<NormalizedEmail>, IdentityStoreError> {
        let key = self
            .store
            .keyspace
            .application_user_email_key(user_id.as_str())?;
        self.store
            .adapter
            .get(&key)
            .await?
            .map(|bytes| {
                let value =
                    String::from_utf8(bytes).map_err(|_| IdentityStoreError::CorruptEmailOwner)?;
                NormalizedEmail::parse(value).map_err(IdentityStoreError::from)
            })
            .transpose()
    }

    async fn sessions_for_user(
        &self,
        user_id: &AppUserId,
    ) -> Result<(Vec<AdminSessionSummary>, bool), IdentityStoreError> {
        let range = self
            .store
            .keyspace
            .application_user_sessions_range(user_id.as_str())?;
        let requested = (MAX_SESSION_RESULTS + 1)
            .min(self.store.adapter.capabilities().maximum_scan_items.get());
        let entries = self
            .store
            .adapter
            .scan(ScanRequest::new(
                range,
                ScanDirection::Forward,
                NonZeroUsize::new(requested).expect("positive session limit"),
            ))
            .await?;
        let truncated = entries.len() > MAX_SESSION_RESULTS;
        let mut sessions = Vec::with_capacity(entries.len().min(MAX_SESSION_RESULTS));
        for entry in entries.into_iter().take(MAX_SESSION_RESULTS) {
            let session_id = std::str::from_utf8(&entry.value)
                .map_err(|_| IdentityStoreError::CorruptSession)?;
            let session_id = SessionId::parse(session_id)?;
            let session = self
                .store
                .session_by_id(&session_id)
                .await?
                .ok_or(IdentityStoreError::CorruptSession)?;
            if session.user_id() != user_id {
                return Err(IdentityStoreError::CorruptSession);
            }
            sessions.push(AdminSessionSummary {
                id: session.id().clone(),
                status: session.status(),
                created_at_unix_seconds: session.created_at_unix_seconds(),
                expires_at_unix_seconds: session.expires_at_unix_seconds(),
                revoked_at_unix_seconds: session.revoked_at_unix_seconds(),
            });
        }
        Ok((sessions, truncated))
    }

    fn authorize(
        &self,
        context: &AdminRequestContext,
        permission: AdminUserPermission,
        action: AdminUserAction,
        target: &str,
    ) -> Result<(), AdminUserApiError> {
        if context.tenant == *self.store.tenant() && context.permissions.contains(&permission) {
            return Ok(());
        }
        self.record(context, action, target, AdminAuditOutcome::Denied)?;
        Err(AdminUserApiError::PermissionDenied)
    }

    fn finish<T, E>(
        &self,
        context: &AdminRequestContext,
        action: AdminUserAction,
        target: &str,
        result: Result<T, E>,
    ) -> Result<T, AdminUserApiError>
    where
        E: Into<AdminUserApiError>,
    {
        let outcome = if result.is_ok() {
            AdminAuditOutcome::Succeeded
        } else {
            AdminAuditOutcome::Failed
        };
        self.record(context, action, target, outcome)?;
        result.map_err(Into::into)
    }

    fn record(
        &self,
        context: &AdminRequestContext,
        action: AdminUserAction,
        target: &str,
        outcome: AdminAuditOutcome,
    ) -> Result<(), AdminUserApiError> {
        self.audit.record(&AdminUserAuditEvent {
            tenant: context.tenant.clone(),
            actor_id: context.actor_id.clone(),
            action,
            target_id: target.to_owned(),
            outcome,
            request_id: context.request_id.clone(),
            occurred_at_unix_seconds: context.occurred_at_unix_seconds,
        })?;
        Ok(())
    }
}

fn valid_audit_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut value = format!("{prefix}_");
    for byte in bytes {
        use fmt::Write;
        write!(&mut value, "{byte:02x}").expect("writing to a string cannot fail");
    }
    value
}

#[derive(Debug)]
pub enum AdminUserApiError {
    InvalidRequest,
    PermissionDenied,
    Store(IdentityStoreError),
    Audit(AdminUserAuditError),
    Invalidation(TrustedMetadataInvalidationError),
}

impl fmt::Display for AdminUserApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest => formatter.write_str("invalid application-user admin request"),
            Self::PermissionDenied => formatter.write_str("application-user permission denied"),
            Self::Store(error) => error.fmt(formatter),
            Self::Audit(error) => error.fmt(formatter),
            Self::Invalidation(error) => error.fmt(formatter),
        }
    }
}

impl Error for AdminUserApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Audit(error) => Some(error),
            Self::Invalidation(error) => Some(error),
            Self::InvalidRequest | Self::PermissionDenied => None,
        }
    }
}

impl From<IdentityStoreError> for AdminUserApiError {
    fn from(error: IdentityStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<AdminUserAuditError> for AdminUserApiError {
    fn from(error: AdminUserAuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<TrustedMetadataInvalidationError> for AdminUserApiError {
    fn from(error: TrustedMetadataInvalidationError) -> Self {
        Self::Invalidation(error)
    }
}

impl From<mako_storage::StorageError> for AdminUserApiError {
    fn from(error: mako_storage::StorageError) -> Self {
        Self::Store(error.into())
    }
}

impl From<mako_storage::KeyCodecError> for AdminUserApiError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::Store(error.into())
    }
}

impl From<serde_json::Error> for AdminUserApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::Store(error.into())
    }
}

impl From<crate::IdentityRecordError> for AdminUserApiError {
    fn from(error: crate::IdentityRecordError) -> Self {
        Self::Store(error.into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::SessionRecord;

    #[derive(Default)]
    struct Audit(Mutex<Vec<AdminUserAuditEvent>>);

    impl AdminUserAuditSink for Audit {
        fn record(&self, event: &AdminUserAuditEvent) -> Result<(), AdminUserAuditError> {
            self.0.lock().expect("audit").push(event.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct Invitations(Mutex<Vec<ApplicationUserInvitation>>);

    impl ApplicationUserInvitationSink for Invitations {
        fn enqueue(&self, invitation: ApplicationUserInvitation) {
            self.0.lock().expect("invitations").push(invitation);
        }
    }

    #[derive(Default)]
    struct Invalidations(Mutex<Vec<AppUserId>>);

    #[async_trait]
    impl TrustedMetadataInvalidationSink for Invalidations {
        async fn trusted_metadata_changed(
            &self,
            _: &TenantScope,
            user_id: &AppUserId,
        ) -> Result<(), TrustedMetadataInvalidationError> {
            self.0.lock().expect("invalidations").push(user_id.clone());
            Ok(())
        }
    }

    #[test]
    fn admin_surface_checks_permissions_audits_and_exposes_no_credentials() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let audit = Audit::default();
            let invitations = Invitations::default();
            let invalidations = Invalidations::default();
            let service = AdminUserService::new(&store, &audit, &invitations, &invalidations);
            let admin = context(
                &tenant,
                [
                    AdminUserPermission::Read,
                    AdminUserPermission::Create,
                    AdminUserPermission::UpdateMetadata,
                    AdminUserPermission::ManageLifecycle,
                    AdminUserPermission::RevokeSessions,
                    AdminUserPermission::Delete,
                ],
            );
            let created = service
                .create(&admin, create_request("person@example.com"))
                .await
                .expect("create");
            let user_id = created.user().id().clone();
            assert_eq!(
                created.email().expect("email").as_str(),
                "person@example.com"
            );
            let page = service
                .search(&admin, Some("person@"), 10)
                .await
                .expect("search");
            assert_eq!(page.users.len(), 1);

            let session = SessionRecord::new(
                tenant.clone(),
                SessionId::parse("ses_abcdefgh").expect("session"),
                user_id.clone(),
                1,
                100,
                0,
            );
            store.create_session(&session).await.expect("session");
            assert_eq!(
                service
                    .inspect(&admin, &user_id)
                    .await
                    .expect("inspect")
                    .sessions()
                    .len(),
                1
            );
            service
                .update_metadata(
                    &admin,
                    &user_id,
                    AdminUpdateUserMetadataRequest {
                        trusted_metadata: TrustedAppMetadata::new(json!({"role": "editor"}))
                            .expect("trusted"),
                        profile_metadata: UserProfileMetadata::new(json!({"name": "Person"}))
                            .expect("profile"),
                    },
                )
                .await
                .expect("metadata");
            assert_eq!(invalidations.0.lock().expect("invalidations").len(), 1);
            service
                .revoke_session(&admin, &user_id, session.id())
                .await
                .expect("revoke");
            service.disable(&admin, &user_id).await.expect("disable");
            service.restore(&admin, &user_id).await.expect("restore");

            service
                .invite(&admin, create_request("invited@example.com"))
                .await
                .expect("invite");
            assert_eq!(invitations.0.lock().expect("invitations").len(), 1);

            let read_only = context(&tenant, [AdminUserPermission::Read]);
            assert!(matches!(
                service.delete(&read_only, &user_id).await,
                Err(AdminUserApiError::PermissionDenied)
            ));
            service.delete(&admin, &user_id).await.expect("delete");
            let events = audit.0.lock().expect("audit");
            assert!(events.iter().any(|event| {
                event.action() == AdminUserAction::Delete
                    && event.outcome() == AdminAuditOutcome::Denied
            }));
            assert!(events.iter().any(|event| {
                event.action() == AdminUserAction::Delete
                    && event.outcome() == AdminAuditOutcome::Succeeded
            }));
            let serialized = serde_json::to_string(&created).expect("view json");
            assert!(!serialized.contains("password"));
            assert!(!serialized.contains("digest"));
        });
    }

    fn create_request(email: &str) -> AdminCreateUserRequest {
        AdminCreateUserRequest {
            email: NormalizedEmail::parse(email).expect("email"),
            trusted_metadata: TrustedAppMetadata::new(json!({})).expect("trusted"),
            profile_metadata: UserProfileMetadata::new(json!({})).expect("profile"),
        }
    }

    fn context(
        tenant: &TenantScope,
        permissions: impl IntoIterator<Item = AdminUserPermission>,
    ) -> AdminRequestContext {
        AdminRequestContext::new(tenant.clone(), "developer-1", "request-1", permissions, 10)
            .expect("context")
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
