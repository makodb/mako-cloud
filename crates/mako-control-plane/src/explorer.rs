use std::{collections::BTreeSet, error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::{
    EXPLORER_CAPABILITY_AUDIENCE, EXPLORER_CAPABILITY_ISSUER, EXPLORER_MAX_GRANT_SECONDS,
    ExplorerAccessMode, ExplorerCapabilityClaims, ExplorerCapabilityError,
    ExplorerCapabilityKeyRing, ExplorerGrantRequest, ExplorerGrantRevocation, ExplorerGrantView,
    ExplorerOperation, TenantScope,
};
use mako_documents::CollectionLifecycle;
use mako_identity::{AdminUserView, AppUserId, AppUserStatus, IdentityStore, IdentityStoreError};
use mako_internal_rpc::{
    ControlToDataClient, IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    InternalClientError,
};
use mako_policy::{
    ExplorerAuthorizationError, ExplorerAuthorizationStore, ExplorerGrantAuthorityRecord,
};
use mako_storage::{Durability, KvAdapter};
use rand_core::{OsRng, RngCore};

use crate::{
    AuthenticationSecurityStatus, CollectionAdminError, CollectionAdminService, ControlAuditAction,
    ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, DeveloperIdentityStatus,
    DeveloperPrincipal, DeveloperRegistrationError, DeveloperRegistrationStore, LifecycleState,
    OrganizationStore, OrganizationStoreError, ProjectDataPermission, ProjectStore,
    ProjectStoreError,
};

#[derive(Clone)]
pub struct ExplorerGrantService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    registrations: DeveloperRegistrationStore,
    projects: ProjectStore,
    organizations: OrganizationStore,
    collections: CollectionAdminService,
    key_ring: ExplorerCapabilityKeyRing,
    audit: Arc<dyn ControlAuditSink>,
    application_users: Arc<dyn ExplorerApplicationUsers>,
    epochs: Arc<dyn ExplorerEpochs>,
}

impl fmt::Debug for ExplorerGrantService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplorerGrantService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ExplorerGrantService {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        registrations: DeveloperRegistrationStore,
        projects: ProjectStore,
        organizations: OrganizationStore,
        collections: CollectionAdminService,
        key_ring: ExplorerCapabilityKeyRing,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Self {
        let application_users = Arc::new(ControlStorageApplicationUsers {
            adapter: Arc::clone(&adapter),
            durability,
        });
        let epochs = Arc::new(ControlStorageExplorerEpochs {
            adapter: Arc::clone(&adapter),
            durability,
        });
        Self {
            adapter,
            durability,
            registrations,
            projects,
            organizations,
            collections,
            key_ring,
            audit,
            application_users,
            epochs,
        }
    }

    /// Answers "which application user is this preview for" from somewhere
    /// other than control storage. A deployment passes the data plane, where
    /// application users actually live.
    #[must_use]
    pub fn with_application_users(mut self, users: Arc<dyn ExplorerApplicationUsers>) -> Self {
        self.application_users = users;
        self
    }

    /// Takes each developer's explorer epoch from somewhere other than control
    /// storage. A deployment passes the data plane, which checks every
    /// capability against its epoch and advances it itself.
    #[must_use]
    pub fn with_epochs(mut self, epochs: Arc<dyn ExplorerEpochs>) -> Self {
        self.epochs = epochs;
        self
    }

    pub async fn issue(
        &self,
        actor: &DeveloperPrincipal,
        request: ExplorerGrantRequest,
        now_unix_seconds: u64,
    ) -> Result<ExplorerGrantView, ExplorerGrantError> {
        request
            .validate()
            .map_err(|_| ExplorerGrantError::InvalidRequest)?;
        if request.operations.is_empty()
            || request.operations.len() > 9
            || request
                .operations
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != request.operations.len()
        {
            return Err(ExplorerGrantError::InvalidRequest);
        }
        let account = self
            .registrations
            .get_account(actor.identity_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        if account.status() != DeveloperIdentityStatus::Active
            || account.authentication_security_status() != AuthenticationSecurityStatus::Active
        {
            return Err(ExplorerGrantError::Forbidden);
        }
        let project = self
            .projects
            .get_project(request.tenant.project_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        let environment = self
            .projects
            .get_environment(request.tenant.project_id(), request.tenant.environment_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        if project.lifecycle() != LifecycleState::Active
            || environment.lifecycle() != LifecycleState::Active
        {
            return Err(ExplorerGrantError::TenantUnavailable);
        }
        let collection = self
            .collections
            .get_collection(
                actor,
                &request.tenant,
                &request.collection_id,
                now_unix_seconds,
            )
            .await
            .map_err(|error| match error {
                CollectionAdminError::NotFound | CollectionAdminError::Forbidden => {
                    ExplorerGrantError::NotFound
                }
                other => ExplorerGrantError::Collection(other),
            })?;
        if collection.lifecycle() != CollectionLifecycle::Active {
            return Err(ExplorerGrantError::TenantUnavailable);
        }

        let operations = request.operations.iter().copied().collect::<BTreeSet<_>>();
        let (application_user_id, reason_hash) = match request.mode {
            ExplorerAccessMode::PolicyPreview => {
                if !membership
                    .role()
                    .allows_data(ProjectDataPermission::DataRead)
                    || request.reason.is_some()
                    || operations.iter().any(|operation| {
                        !matches!(
                            operation,
                            ExplorerOperation::Get
                                | ExplorerOperation::Browse
                                | ExplorerOperation::Query
                                | ExplorerOperation::Plan
                                | ExplorerOperation::Simulate
                        )
                    })
                {
                    return self.denied(
                        actor,
                        &request,
                        project.organization_id(),
                        now_unix_seconds,
                    );
                }
                let user_id = request
                    .application_user_id
                    .as_deref()
                    .ok_or(ExplorerGrantError::InvalidRequest)
                    .and_then(|value| {
                        AppUserId::parse(value).map_err(|_| ExplorerGrantError::InvalidRequest)
                    })?;
                let status = self
                    .application_users
                    .status(actor, &request.tenant, &user_id)
                    .await?
                    .ok_or(ExplorerGrantError::NotFound)?;
                if status != AppUserStatus::Active {
                    return Err(ExplorerGrantError::NotFound);
                }
                (Some(user_id.as_str().to_owned()), None)
            }
            ExplorerAccessMode::Administrative => {
                if !membership
                    .role()
                    .allows_data(ProjectDataPermission::DataAdmin)
                    || request.application_user_id.is_some()
                    || operations.iter().any(|operation| {
                        required_permission(*operation)
                            .is_some_and(|permission| !membership.role().allows_data(permission))
                    })
                {
                    return self.denied(
                        actor,
                        &request,
                        project.organization_id(),
                        now_unix_seconds,
                    );
                }
                let reason = request
                    .reason
                    .as_deref()
                    .ok_or(ExplorerGrantError::InvalidRequest)?;
                if !(3..=500).contains(&reason.len())
                    || reason.trim() != reason
                    || reason.chars().any(char::is_control)
                {
                    return Err(ExplorerGrantError::InvalidRequest);
                }
                (
                    None,
                    Some(blake3::hash(reason.as_bytes()).to_hex().to_string()),
                )
            }
        };

        let authority = ExplorerAuthorizationStore::new(
            Arc::clone(&self.adapter),
            &request.tenant,
            &request.tenant,
            self.durability,
        )?;
        // The capability is checked against the data plane's epoch, which every
        // policy, schema and membership change advances there. Signing with the
        // local copy instead stranded the explorer at the first such change:
        // the data plane refused every grant as stale from then on.
        let authorization_epoch = self.epochs.current(actor, &request.tenant).await?;
        catch_up_epoch(
            &authority,
            actor.identity_id().as_str(),
            authorization_epoch,
        )
        .await?;
        let nonce = random_id("xnonce_");
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(request.duration_seconds.min(EXPLORER_MAX_GRANT_SECONDS))
            .ok_or(ExplorerGrantError::InvalidRequest)?;
        authority
            .record_issue(&ExplorerGrantAuthorityRecord {
                nonce: nonce.clone(),
                developer_identity_id: actor.identity_id().as_str().to_owned(),
                mode: request.mode,
                authorization_epoch,
                issued_at_unix_seconds: now_unix_seconds,
                expires_at_unix_seconds,
                revoked_at_unix_seconds: None,
            })
            .await?;
        let token = self.key_ring.issue(ExplorerCapabilityClaims {
            issuer: EXPLORER_CAPABILITY_ISSUER.to_owned(),
            audience: EXPLORER_CAPABILITY_AUDIENCE.to_owned(),
            developer_identity_id: actor.identity_id().as_str().to_owned(),
            tenant: request.tenant.clone(),
            collection_id: request.collection_id.clone(),
            mode: request.mode,
            operations: operations.clone(),
            reason_hash,
            application_user_id: application_user_id.clone(),
            nonce: nonce.clone(),
            authorization_epoch,
            issued_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            key_id: String::new(),
        })?;
        let grant_id = grant_id_for_nonce(&nonce)?;
        self.audit.record(ControlAuditEvent {
            organization_id: project.organization_id().clone(),
            actor_id: actor.identity_id().clone(),
            action: ControlAuditAction::ExplorerGrantIssue,
            target: format!(
                "{}/{}/{}:{grant_id}",
                request.tenant.project_id(),
                request.tenant.environment_id(),
                request.collection_id
            ),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(ExplorerGrantView {
            grant_id,
            capability: token.expose_for_authorization_header().to_owned(),
            mode: request.mode,
            operations: operations.into_iter().collect(),
            application_user_id,
            issued_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            authorization_epoch,
        })
    }

    pub async fn revoke(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        nonce: &str,
        now_unix_seconds: u64,
    ) -> Result<ExplorerGrantRevocation, ExplorerGrantError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        self.organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?
            .ok_or(ExplorerGrantError::NotFound)?;
        let authority = ExplorerAuthorizationStore::new(
            Arc::clone(&self.adapter),
            tenant,
            tenant,
            self.durability,
        )?;
        authority
            .revoke(nonce, actor.identity_id().as_str(), now_unix_seconds)
            .await?;
        self.audit.record(ControlAuditEvent {
            organization_id: project.organization_id().clone(),
            actor_id: actor.identity_id().clone(),
            action: ControlAuditAction::ExplorerGrantRevoke,
            target: format!(
                "{}/{}/explorer-grant",
                tenant.project_id(),
                tenant.environment_id()
            ),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(ExplorerGrantRevocation {
            grant_id: grant_id_for_nonce(nonce)?,
            revoked_at_unix_seconds: now_unix_seconds,
        })
    }

    pub async fn revoke_by_grant_id(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        grant_id: &str,
        now_unix_seconds: u64,
    ) -> Result<ExplorerGrantRevocation, ExplorerGrantError> {
        let suffix = grant_id
            .strip_prefix("xgr_")
            .filter(|suffix| suffix.len() == 32)
            .ok_or(ExplorerGrantError::NotFound)?;
        self.revoke(actor, tenant, &format!("xnonce_{suffix}"), now_unix_seconds)
            .await
    }

    fn denied<T>(
        &self,
        actor: &DeveloperPrincipal,
        request: &ExplorerGrantRequest,
        organization_id: &crate::OrganizationId,
        now_unix_seconds: u64,
    ) -> Result<T, ExplorerGrantError> {
        self.audit.record(ControlAuditEvent {
            organization_id: organization_id.clone(),
            actor_id: actor.identity_id().clone(),
            action: ControlAuditAction::ExplorerGrantIssue,
            target: format!(
                "{}/{}/{}",
                request.tenant.project_id(),
                request.tenant.environment_id(),
                request.collection_id
            ),
            outcome: ControlAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
        Err(ExplorerGrantError::Forbidden)
    }
}

fn required_permission(operation: ExplorerOperation) -> Option<ProjectDataPermission> {
    match operation {
        ExplorerOperation::History => Some(ProjectDataPermission::DocumentHistory),
        ExplorerOperation::Import => Some(ProjectDataPermission::Import),
        ExplorerOperation::Export => Some(ProjectDataPermission::Export),
        ExplorerOperation::Get
        | ExplorerOperation::Browse
        | ExplorerOperation::Query
        | ExplorerOperation::Plan
        | ExplorerOperation::Simulate
        | ExplorerOperation::Mutate => None,
    }
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 24];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn grant_id_for_nonce(nonce: &str) -> Result<String, ExplorerGrantError> {
    nonce
        .strip_prefix("xnonce_")
        .filter(|suffix| suffix.len() == 32)
        .map(|suffix| format!("xgr_{suffix}"))
        .ok_or(ExplorerGrantError::InvalidRequest)
}

/// Where a policy preview confirms the application user it impersonates.
///
/// Application users live in the data plane's identity store, not in control
/// storage, so a deployment answers this over the internal identity-admin RPC
/// -- the same way the Users screen lists them. A single-store test answers
/// it from the adapter the service was built on, which is where it registered
/// its users; that is the default.
#[async_trait]
pub trait ExplorerApplicationUsers: Send + Sync {
    /// The user's status, or `None` when no such user exists in the tenant.
    async fn status(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Result<Option<AppUserStatus>, ExplorerGrantError>;
}

struct ControlStorageApplicationUsers {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

#[async_trait]
impl ExplorerApplicationUsers for ControlStorageApplicationUsers {
    async fn status(
        &self,
        _actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Result<Option<AppUserStatus>, ExplorerGrantError> {
        let identities =
            IdentityStore::new(Arc::clone(&self.adapter), tenant, tenant, self.durability)?;
        Ok(identities
            .user_by_id(user_id)
            .await?
            .map(|user| user.status()))
    }
}

/// Application users as the data plane knows them, over the internal
/// identity-admin RPC.
pub struct DataPlaneApplicationUsers {
    client: Arc<ControlToDataClient>,
}

impl DataPlaneApplicationUsers {
    #[must_use]
    pub fn new(client: Arc<ControlToDataClient>) -> Self {
        Self { client }
    }
}

impl fmt::Debug for DataPlaneApplicationUsers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DataPlaneApplicationUsers")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ExplorerApplicationUsers for DataPlaneApplicationUsers {
    async fn status(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        user_id: &AppUserId,
    ) -> Result<Option<AppUserStatus>, ExplorerGrantError> {
        let command = IdentityAdminCommand {
            operation: IdentityAdminOperation::InspectUser,
            actor_id: actor.identity_id().as_str().to_owned(),
            permissions: BTreeSet::from([IdentityAdminPermission::ReadApplicationUsers]),
            input: serde_json::json!({ "userId": user_id.as_str() }),
        };
        // A lookup is a read. It travels under fresh ids every time so the
        // data plane's response journal can never replay an earlier answer
        // about this user -- a user disabled a minute ago must read as such.
        let mut nonce = [0_u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let nonce = blake3::hash(&nonce).to_hex();
        let request_id = format!("req_{}", &nonce[..32]);
        let idempotency_key = format!("idem_{}", &nonce[32..]);
        match self.client.administer::<AdminUserView>(
            tenant,
            &request_id,
            &idempotency_key,
            &command,
        ) {
            Ok(view) => Ok(Some(view.user().status())),
            Err(InternalClientError::Remote { status: 404, .. }) => Ok(None),
            Err(error) => Err(ExplorerGrantError::ApplicationUsers(error)),
        }
    }
}

/// Where a grant learns the developer's current explorer epoch.
#[async_trait]
pub trait ExplorerEpochs: Send + Sync {
    async fn current(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
    ) -> Result<u64, ExplorerGrantError>;
}

struct ControlStorageExplorerEpochs {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

#[async_trait]
impl ExplorerEpochs for ControlStorageExplorerEpochs {
    async fn current(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
    ) -> Result<u64, ExplorerGrantError> {
        let authority = ExplorerAuthorizationStore::new(
            Arc::clone(&self.adapter),
            tenant,
            tenant,
            self.durability,
        )?;
        Ok(authority
            .current_epoch(actor.identity_id().as_str())
            .await?)
    }
}

/// Explorer epochs as the data plane holds them, over the internal
/// identity-admin RPC.
pub struct DataPlaneExplorerEpochs {
    client: Arc<ControlToDataClient>,
}

impl DataPlaneExplorerEpochs {
    #[must_use]
    pub fn new(client: Arc<ControlToDataClient>) -> Self {
        Self { client }
    }
}

impl fmt::Debug for DataPlaneExplorerEpochs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DataPlaneExplorerEpochs")
            .finish_non_exhaustive()
    }
}

#[derive(serde::Deserialize)]
struct EpochWire {
    epoch: u64,
}

#[async_trait]
impl ExplorerEpochs for DataPlaneExplorerEpochs {
    async fn current(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
    ) -> Result<u64, ExplorerGrantError> {
        let command = IdentityAdminCommand {
            operation: IdentityAdminOperation::ReadExplorerEpoch,
            actor_id: actor.identity_id().as_str().to_owned(),
            permissions: BTreeSet::from([IdentityAdminPermission::ManageExplorerGrants]),
            input: serde_json::json!({ "developerIdentityId": actor.identity_id().as_str() }),
        };
        // A read, under fresh ids: the response journal must never answer with
        // an epoch that has since been advanced.
        let mut nonce = [0_u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let nonce = blake3::hash(&nonce).to_hex();
        self.client
            .administer::<EpochWire>(
                tenant,
                &format!("req_{}", &nonce[..32]),
                &format!("idem_{}", &nonce[32..]),
                &command,
            )
            .map(|wire| wire.epoch)
            .map_err(ExplorerGrantError::EpochSource)
    }
}

/// Brings control storage's copy of a developer's epoch up to `target`, so the
/// grant it records there carries the same epoch as the capability. A copy
/// that is already ahead cannot be wound back and fails the grant.
async fn catch_up_epoch(
    authority: &ExplorerAuthorizationStore,
    developer_identity_id: &str,
    target: u64,
) -> Result<(), ExplorerGrantError> {
    const MAX_ADVANCES: u64 = 4_096;
    let mut local = authority.current_epoch(developer_identity_id).await?;
    if local > target || target - local > MAX_ADVANCES {
        return Err(ExplorerGrantError::Authority(
            ExplorerAuthorizationError::StaleEpoch,
        ));
    }
    while local < target {
        local = authority.advance_epoch(developer_identity_id).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum ExplorerGrantError {
    InvalidRequest,
    NotFound,
    Forbidden,
    TenantUnavailable,
    Registration(DeveloperRegistrationError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Collection(CollectionAdminError),
    Identity(IdentityStoreError),
    Authority(ExplorerAuthorizationError),
    Capability(ExplorerCapabilityError),
    ApplicationUsers(InternalClientError),
    EpochSource(InternalClientError),
}

impl fmt::Display for ExplorerGrantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "explorer grant request is invalid",
            Self::TenantUnavailable => "explorer tenant is unavailable",
            Self::NotFound | Self::Forbidden => "explorer target was not found",
            _ => "explorer grant service is unavailable",
        })
    }
}

impl Error for ExplorerGrantError {}

macro_rules! grant_error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for ExplorerGrantError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

grant_error_from!(DeveloperRegistrationError, Registration);
grant_error_from!(ProjectStoreError, Project);
grant_error_from!(OrganizationStoreError, Organization);
grant_error_from!(CollectionAdminError, Collection);
grant_error_from!(IdentityStoreError, Identity);
grant_error_from!(ExplorerAuthorizationError, Authority);
grant_error_from!(ExplorerCapabilityError, Capability);

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    fn store() -> ExplorerAuthorizationStore {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        ExplorerAuthorizationStore::new(
            Arc::new(MemoryAdapter::new()),
            &tenant,
            &tenant,
            Durability::Memory,
        )
        .expect("store")
    }

    #[test]
    fn control_storage_catches_up_with_the_authoritative_epoch() {
        futures::executor::block_on(async {
            let authority = store();
            assert_eq!(
                authority.current_epoch("dev_abcdefgh").await.expect("read"),
                1
            );
            catch_up_epoch(&authority, "dev_abcdefgh", 3)
                .await
                .expect("catch up");
            assert_eq!(
                authority.current_epoch("dev_abcdefgh").await.expect("read"),
                3
            );
            // Already current: nothing to do.
            catch_up_epoch(&authority, "dev_abcdefgh", 3)
                .await
                .expect("current");
            // A copy ahead of the authority cannot be wound back.
            assert!(matches!(
                catch_up_epoch(&authority, "dev_abcdefgh", 2).await,
                Err(ExplorerGrantError::Authority(
                    ExplorerAuthorizationError::StaleEpoch
                ))
            ));
        });
    }
}
