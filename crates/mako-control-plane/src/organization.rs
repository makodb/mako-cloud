use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    ControlKeyspace, ControlKeyspaceError, ControlModelError, DeveloperIdentityId,
    DeveloperPrincipal, InvitationId, InvitationInput, InvitationRecord, InvitationStatus,
    MembershipRecord, OrganizationId, OrganizationRecord, OrganizationRole,
};

/// A closed billing period, stored the first time it is derived after the
/// period ends and never rewritten.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvoiceRecord {
    pub organization_id: OrganizationId,
    pub period_start_unix_milliseconds: u64,
    pub period_end_unix_milliseconds: u64,
    /// Where retained evidence actually began when this was derived. Equal to
    /// the period start when the whole month was still on record; later when
    /// part of it had already aged out; equal to the period end when nothing
    /// remained and the invoice records that honestly rather than a zero that
    /// pretends to knowledge.
    pub derived_from_unix_milliseconds: u64,
    pub rated: mako_billing::rating::RatedPeriod,
    pub closed_at_unix_seconds: u64,
}

#[derive(Clone)]
pub struct OrganizationStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for OrganizationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrganizationStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl OrganizationStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, OrganizationStoreError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(OrganizationStoreError::UnsupportedDurability(durability));
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    pub async fn create_organization(
        &self,
        organization: &OrganizationRecord,
        owner: &MembershipRecord,
    ) -> Result<(), OrganizationStoreError> {
        if owner.organization_id() != organization.id() || owner.role() != OrganizationRole::Owner {
            return Err(OrganizationStoreError::RecordScopeMismatch);
        }
        let organization_key = ControlKeyspace::organization_key(organization.id())?;
        let owner_key = ControlKeyspace::membership_key(
            owner.organization_id(),
            owner.developer_identity_id(),
        )?;
        let developer_organization_key = ControlKeyspace::developer_organization_key(
            owner.developer_identity_id(),
            organization.id(),
        )?;
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&organization_key, serde_json::to_vec(organization)?);
        batch.put(&owner_key, serde_json::to_vec(owner)?);
        batch.put(
            &developer_organization_key,
            serde_json::to_vec(organization.id())?,
        );
        self.apply_create(
            vec![organization_key, owner_key, developer_organization_key],
            batch,
        )
        .await
    }

    /// Every organization on the deployment, in id order.
    ///
    /// For platform maintenance loops that must visit all tenants; anything
    /// member-facing goes through `list_organizations_for` and its membership
    /// checks instead.
    pub async fn all_organizations(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<OrganizationRecord>, OrganizationStoreError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::organizations_range()?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        entries
            .iter()
            .map(|entry| serde_json::from_slice(&entry.value).map_err(OrganizationStoreError::from))
            .collect()
    }

    pub async fn get_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<OrganizationRecord>, OrganizationStoreError> {
        self.read(&ControlKeyspace::organization_key(organization_id)?)
            .await
    }

    pub async fn list_organizations_for(
        &self,
        developer_id: &DeveloperIdentityId,
        limit: NonZeroUsize,
    ) -> Result<Vec<OrganizationRecord>, OrganizationStoreError> {
        let references = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_organizations_range(developer_id)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let mut organizations = Vec::with_capacity(references.len());
        for reference in references {
            let organization_id: OrganizationId = serde_json::from_slice(&reference.value)?;
            let organization = self
                .get_organization(&organization_id)
                .await?
                .ok_or(OrganizationStoreError::CorruptRecord)?;
            if self
                .get_membership(&organization_id, developer_id)
                .await?
                .is_none()
            {
                return Err(OrganizationStoreError::CorruptRecord);
            }
            organizations.push(organization);
        }
        Ok(organizations)
    }

    pub async fn replace_organization(
        &self,
        previous: &OrganizationRecord,
        next: &OrganizationRecord,
    ) -> Result<(), OrganizationStoreError> {
        if previous.id() != next.id() {
            return Err(OrganizationStoreError::RecordScopeMismatch);
        }
        let key = ControlKeyspace::organization_key(previous.id())?;
        self.replace(&key, previous, next).await
    }

    pub async fn get_membership(
        &self,
        organization_id: &OrganizationId,
        developer_id: &DeveloperIdentityId,
    ) -> Result<Option<MembershipRecord>, OrganizationStoreError> {
        self.read(&ControlKeyspace::membership_key(
            organization_id,
            developer_id,
        )?)
        .await
    }

    pub async fn list_memberships(
        &self,
        organization_id: &OrganizationId,
        limit: NonZeroUsize,
    ) -> Result<Vec<MembershipRecord>, OrganizationStoreError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::memberships_range(organization_id)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let memberships = values
            .into_iter()
            .map(|value| serde_json::from_slice::<MembershipRecord>(&value.value))
            .collect::<Result<Vec<_>, _>>()?;
        if memberships
            .iter()
            .any(|membership| membership.organization_id() != organization_id)
        {
            return Err(OrganizationStoreError::CorruptRecord);
        }
        Ok(memberships)
    }

    pub async fn replace_membership(
        &self,
        previous: &MembershipRecord,
        next: &MembershipRecord,
    ) -> Result<(), OrganizationStoreError> {
        if previous.organization_id() != next.organization_id()
            || previous.developer_identity_id() != next.developer_identity_id()
        {
            return Err(OrganizationStoreError::RecordScopeMismatch);
        }
        let key = ControlKeyspace::membership_key(
            previous.organization_id(),
            previous.developer_identity_id(),
        )?;
        self.replace(&key, previous, next).await
    }

    pub async fn delete_membership(
        &self,
        previous: &MembershipRecord,
    ) -> Result<(), OrganizationStoreError> {
        let key = ControlKeyspace::membership_key(
            previous.organization_id(),
            previous.developer_identity_id(),
        )?;
        let encoded = serde_json::to_vec(previous)?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.delete(&key);
        batch.delete(ControlKeyspace::developer_organization_key(
            previous.developer_identity_id(),
            previous.organization_id(),
        )?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: encoded,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn create_invitation(
        &self,
        invitation: &InvitationRecord,
    ) -> Result<(), OrganizationStoreError> {
        let key = ControlKeyspace::invitation_key(invitation.id())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(invitation)?);
        self.apply_create(vec![key], batch).await
    }

    pub async fn get_invitation(
        &self,
        invitation_id: &InvitationId,
    ) -> Result<Option<InvitationRecord>, OrganizationStoreError> {
        self.read(&ControlKeyspace::invitation_key(invitation_id)?)
            .await
    }

    pub async fn accept_invitation(
        &self,
        previous: &InvitationRecord,
        accepted: &InvitationRecord,
        membership: &MembershipRecord,
    ) -> Result<(), OrganizationStoreError> {
        if previous.id() != accepted.id()
            || previous.organization_id() != accepted.organization_id()
            || accepted.organization_id() != membership.organization_id()
        {
            return Err(OrganizationStoreError::RecordScopeMismatch);
        }
        let invitation_key = ControlKeyspace::invitation_key(previous.id())?;
        let membership_key = ControlKeyspace::membership_key(
            membership.organization_id(),
            membership.developer_identity_id(),
        )?;
        let developer_organization_key = ControlKeyspace::developer_organization_key(
            membership.developer_identity_id(),
            membership.organization_id(),
        )?;
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(&invitation_key, serde_json::to_vec(accepted)?);
        batch.put(&membership_key, serde_json::to_vec(membership)?);
        batch.put(
            &developer_organization_key,
            serde_json::to_vec(membership.organization_id())?,
        );
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: invitation_key,
                    value: serde_json::to_vec(previous)?,
                },
                KeyCondition::Missing {
                    key: membership_key,
                },
                KeyCondition::Missing {
                    key: developer_organization_key,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    /// Close a billing period: store its invoice, exactly once.
    ///
    /// The conditional create is the immutability: a period that has been
    /// closed conflicts instead of being rewritten, so whichever derivation
    /// got there first is the invoice forever, and a re-derivation that
    /// disagrees is a bug to investigate rather than silently absorb.
    pub async fn close_invoice(
        &self,
        invoice: &InvoiceRecord,
    ) -> Result<(), OrganizationStoreError> {
        if invoice.period_end_unix_milliseconds <= invoice.period_start_unix_milliseconds {
            return Err(OrganizationStoreError::RecordScopeMismatch);
        }
        let key = ControlKeyspace::organization_invoice_key(
            &invoice.organization_id,
            invoice.period_start_unix_milliseconds,
        )?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(invoice)?);
        self.apply_create(vec![key], batch).await
    }

    pub async fn get_invoice(
        &self,
        organization_id: &OrganizationId,
        period_start_unix_milliseconds: u64,
    ) -> Result<Option<InvoiceRecord>, OrganizationStoreError> {
        let key = ControlKeyspace::organization_invoice_key(
            organization_id,
            period_start_unix_milliseconds,
        )?;
        self.read(&key).await
    }

    /// Every closed period, oldest first.
    pub async fn list_invoices(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Vec<InvoiceRecord>, OrganizationStoreError> {
        let range = ControlKeyspace::organization_invoices_range(organization_id)?;
        let limit = self
            .adapter
            .capabilities()
            .maximum_scan_items
            .min(NonZeroUsize::new(256).expect("scan limit"));
        let entries = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        entries
            .iter()
            .map(|entry| serde_json::from_slice(&entry.value).map_err(OrganizationStoreError::from))
            .collect()
    }

    async fn read<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, OrganizationStoreError> {
        self.adapter
            .get(key)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(OrganizationStoreError::from))
            .transpose()
    }

    async fn replace<T: serde::Serialize>(
        &self,
        key: &[u8],
        previous: &T,
        next: &T,
    ) -> Result<(), OrganizationStoreError> {
        let mut batch = WriteBatch::new();
        batch.put(key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key: key.to_vec(),
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply_create(
        &self,
        keys: Vec<Vec<u8>>,
        batch: WriteBatch,
    ) -> Result<(), OrganizationStoreError> {
        self.apply(AtomicWrite {
            conditions: keys
                .into_iter()
                .map(|key| KeyCondition::Missing { key })
                .collect(),
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), OrganizationStoreError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(OrganizationStoreError::Conflict),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct InvitationToken(String);

impl InvitationToken {
    pub fn parse(value: impl Into<String>) -> Result<Self, OrganizationServiceError> {
        let value = value.into();
        let decoded = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(|_| OrganizationServiceError::InvalidInvitation)?;
        if decoded.len() != 32 || value.chars().any(char::is_whitespace) {
            return Err(OrganizationServiceError::InvalidInvitation);
        }
        Ok(Self(value))
    }

    fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.0
    }

    fn digest(&self) -> String {
        blake3::hash(self.0.as_bytes()).to_hex().to_string()
    }
}

impl fmt::Debug for InvitationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvitationToken([REDACTED])")
    }
}

#[derive(Debug)]
pub struct IssuedInvitation {
    pub invitation: InvitationRecord,
    pub token: InvitationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewInvitation {
    pub id: InvitationId,
    pub organization_id: OrganizationId,
    pub normalized_email: String,
    pub role: OrganizationRole,
    pub created_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlAuditAction {
    OrganizationCreate,
    OrganizationRead,
    OrganizationUpdate,
    OrganizationRestore,
    OrganizationDeleteRequest,
    InvitationCreate,
    InvitationAccept,
    MembershipList,
    MembershipUpdate,
    MembershipDelete,
    ProjectCreate,
    ProjectRead,
    ProjectSuspend,
    ProjectRestore,
    ProjectDeleteRequest,
    ProjectRename,
    ProjectTransfer,
    EnvironmentCreate,
    EnvironmentRead,
    EnvironmentSuspend,
    EnvironmentRestore,
    EnvironmentDeleteRequest,
    CollectionCreate,
    CollectionRead,
    CollectionSchemaPublish,
    CollectionMigrationCreate,
    CollectionMigrationRead,
    CollectionIndexCreate,
    CollectionIndexRead,
    CollectionIndexDelete,
    PolicyDraftCreate,
    PolicyRead,
    PolicyValidate,
    PolicyTest,
    PolicyActivate,
    PolicyRollback,
    ApplicationUserAccess,
    ExplorerGrantIssue,
    ExplorerGrantRevoke,
    DataJobCreate,
    DataJobDryRun,
    DataJobConfirm,
    DataJobCancel,
    DataJobArtifactUpload,
    DataJobArtifactDownload,
    CredentialCreate,
    CredentialRead,
    CredentialRotate,
    CredentialRetire,
    SigningKeyRotate,
    FunctionSecretCreate,
    FunctionSecretRotate,
    FunctionSecretRetire,
    EmailTemplateRead,
    EmailTemplateUpdate,
    FunctionCreate,
    FunctionRead,
    FunctionConfigure,
    FunctionBundleUpload,
    FunctionDeploy,
    FunctionHealthCheck,
    FunctionPromote,
    FunctionRollback,
    FunctionTest,
    FunctionLogsRead,
    FunctionDelete,
    ObservabilityRead,
    WebhookEndpointRead,
    WebhookEndpointCreate,
    WebhookEndpointUpdate,
    WebhookEndpointDelete,
    WebhookEndpointRotateSecret,
    WebhookEndpointResume,
    WebhookEndpointRedeliver,
}

impl ControlAuditAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OrganizationCreate => "organization_create",
            Self::OrganizationRead => "organization_read",
            Self::OrganizationUpdate => "organization_update",
            Self::OrganizationRestore => "organization_restore",
            Self::OrganizationDeleteRequest => "organization_delete_request",
            Self::InvitationCreate => "invitation_create",
            Self::InvitationAccept => "invitation_accept",
            Self::MembershipList => "membership_list",
            Self::MembershipUpdate => "membership_update",
            Self::MembershipDelete => "membership_delete",
            Self::ProjectCreate => "project_create",
            Self::ProjectRead => "project_read",
            Self::ProjectSuspend => "project_suspend",
            Self::ProjectRestore => "project_restore",
            Self::ProjectDeleteRequest => "project_delete_request",
            Self::ProjectRename => "project_rename",
            Self::ProjectTransfer => "project_transfer",
            Self::EnvironmentCreate => "environment_create",
            Self::EnvironmentRead => "environment_read",
            Self::EnvironmentSuspend => "environment_suspend",
            Self::EnvironmentRestore => "environment_restore",
            Self::EnvironmentDeleteRequest => "environment_delete_request",
            Self::CollectionCreate => "collection_create",
            Self::CollectionRead => "collection_read",
            Self::CollectionSchemaPublish => "collection_schema_publish",
            Self::CollectionMigrationCreate => "collection_migration_create",
            Self::CollectionMigrationRead => "collection_migration_read",
            Self::CollectionIndexCreate => "collection_index_create",
            Self::CollectionIndexRead => "collection_index_read",
            Self::CollectionIndexDelete => "collection_index_delete",
            Self::PolicyDraftCreate => "policy_draft_create",
            Self::PolicyRead => "policy_read",
            Self::PolicyValidate => "policy_validate",
            Self::PolicyTest => "policy_test",
            Self::PolicyActivate => "policy_activate",
            Self::PolicyRollback => "policy_rollback",
            Self::ApplicationUserAccess => "application_user_access",
            Self::ExplorerGrantIssue => "explorer_grant_issue",
            Self::ExplorerGrantRevoke => "explorer_grant_revoke",
            Self::DataJobCreate => "data_job_create",
            Self::DataJobDryRun => "data_job_dry_run",
            Self::DataJobConfirm => "data_job_confirm",
            Self::DataJobCancel => "data_job_cancel",
            Self::DataJobArtifactUpload => "data_job_artifact_upload",
            Self::DataJobArtifactDownload => "data_job_artifact_download",
            Self::CredentialCreate => "credential_create",
            Self::CredentialRead => "credential_read",
            Self::CredentialRotate => "credential_rotate",
            Self::CredentialRetire => "credential_retire",
            Self::SigningKeyRotate => "signing_key_rotate",
            Self::FunctionSecretCreate => "function_secret_create",
            Self::FunctionSecretRotate => "function_secret_rotate",
            Self::FunctionSecretRetire => "function_secret_retire",
            Self::EmailTemplateRead => "email_template_read",
            Self::EmailTemplateUpdate => "email_template_update",
            Self::FunctionCreate => "function_create",
            Self::FunctionRead => "function_read",
            Self::FunctionConfigure => "function_configure",
            Self::FunctionBundleUpload => "function_bundle_upload",
            Self::FunctionDeploy => "function_deploy",
            Self::FunctionHealthCheck => "function_health_check",
            Self::FunctionPromote => "function_promote",
            Self::FunctionRollback => "function_rollback",
            Self::FunctionTest => "function_test",
            Self::FunctionLogsRead => "function_logs_read",
            Self::FunctionDelete => "function_delete",
            Self::ObservabilityRead => "observability_read",
            Self::WebhookEndpointRead => "webhook_endpoint_read",
            Self::WebhookEndpointCreate => "webhook_endpoint_create",
            Self::WebhookEndpointUpdate => "webhook_endpoint_update",
            Self::WebhookEndpointDelete => "webhook_endpoint_delete",
            Self::WebhookEndpointRotateSecret => "webhook_endpoint_rotate_secret",
            Self::WebhookEndpointResume => "webhook_endpoint_resume",
            Self::WebhookEndpointRedeliver => "webhook_endpoint_redeliver",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlAuditOutcome {
    Allowed,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlAuditEvent {
    pub organization_id: OrganizationId,
    pub actor_id: DeveloperIdentityId,
    pub action: ControlAuditAction,
    pub target: String,
    pub outcome: ControlAuditOutcome,
    pub at_unix_seconds: u64,
}

pub trait ControlAuditSink: Send + Sync {
    fn record(&self, event: ControlAuditEvent);
}

#[derive(Clone)]
pub struct OrganizationService {
    store: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for OrganizationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrganizationService")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl OrganizationService {
    #[must_use]
    pub fn new(store: OrganizationStore, audit: Arc<dyn ControlAuditSink>) -> Self {
        Self { store, audit }
    }

    pub async fn create_organization(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: OrganizationId,
        name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        let organization = OrganizationRecord::new(organization_id, name, now_unix_seconds)?;
        let owner = MembershipRecord::new(
            organization.id().clone(),
            actor.identity_id().clone(),
            OrganizationRole::Owner,
            now_unix_seconds,
        );
        self.store
            .create_organization(&organization, &owner)
            .await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::OrganizationCreate,
            "organization",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(organization)
    }

    /// The developer's personal space, created on first use.
    ///
    /// The id is derived by the caller from the developer's identity, so two
    /// concurrent first uses race to the same record: whichever creation
    /// lands first wins and the other reads it back. A record that exists
    /// under that id but is not a personal space, or does not belong to the
    /// caller, is refused rather than adopted.
    pub async fn ensure_personal_organization(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: OrganizationId,
        now_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        if let Some(existing) = self.store.get_organization(&organization_id).await? {
            return self.owned_personal_space(actor, existing).await;
        }
        let organization = OrganizationRecord::new_personal(
            organization_id.clone(),
            actor.display_name(),
            now_unix_seconds,
        )?;
        let owner = MembershipRecord::new(
            organization.id().clone(),
            actor.identity_id().clone(),
            OrganizationRole::Owner,
            now_unix_seconds,
        );
        match self.store.create_organization(&organization, &owner).await {
            Ok(()) => {}
            Err(OrganizationStoreError::Conflict) => {
                let existing = self
                    .store
                    .get_organization(&organization_id)
                    .await?
                    .ok_or(OrganizationServiceError::NotFound)?;
                return self.owned_personal_space(actor, existing).await;
            }
            Err(error) => return Err(error.into()),
        }
        self.audit(
            actor,
            &organization,
            ControlAuditAction::OrganizationCreate,
            "personal_space",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(organization)
    }

    async fn owned_personal_space(
        &self,
        actor: &DeveloperPrincipal,
        existing: OrganizationRecord,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        if !existing.is_personal() {
            return Err(OrganizationServiceError::Forbidden);
        }
        self.store
            .get_membership(existing.id(), actor.identity_id())
            .await?
            .ok_or(OrganizationServiceError::Forbidden)?;
        Ok(existing)
    }

    pub async fn list_organizations(
        &self,
        actor: &DeveloperPrincipal,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<OrganizationRecord>, OrganizationServiceError> {
        let organizations = self
            .store
            .list_organizations_for(actor.identity_id(), limit)
            .await?;
        for organization in &organizations {
            self.audit(
                actor,
                organization,
                ControlAuditAction::OrganizationRead,
                "organizations",
                ControlAuditOutcome::Allowed,
                now_unix_seconds,
            );
        }
        Ok(organizations)
    }

    pub async fn get_organization(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        now_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        let organization = self.organization(organization_id).await?;
        self.require_membership(
            actor,
            organization_id,
            ControlAuditAction::OrganizationRead,
            now_unix_seconds,
        )
        .await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::OrganizationRead,
            "organization",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(organization)
    }

    pub async fn rename_organization(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        name: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        let previous = self.organization(organization_id).await?;
        let membership = self
            .require_membership(
                actor,
                organization_id,
                ControlAuditAction::OrganizationUpdate,
                now_unix_seconds,
            )
            .await?;
        if !membership.role().can_manage_members() {
            self.audit(
                actor,
                &previous,
                ControlAuditAction::OrganizationUpdate,
                "organization",
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        let mut next = previous.clone();
        next.rename(name, now_unix_seconds)?;
        self.store.replace_organization(&previous, &next).await?;
        self.audit(
            actor,
            &next,
            ControlAuditAction::OrganizationUpdate,
            "organization",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn invite(
        &self,
        actor: &DeveloperPrincipal,
        input: NewInvitation,
    ) -> Result<IssuedInvitation, OrganizationServiceError> {
        let organization = self.organization(&input.organization_id).await?;
        // One person's space has nothing to invite, change, remove, or delete.
        if organization.is_personal() {
            return Err(OrganizationServiceError::PersonalSpace);
        }
        let actor_membership = self
            .require_membership(
                actor,
                &input.organization_id,
                ControlAuditAction::InvitationCreate,
                input.created_at_unix_seconds,
            )
            .await?;
        if !actor_membership.role().can_manage_members()
            || !actor_membership.role().can_assign(input.role)
        {
            self.audit(
                actor,
                &organization,
                ControlAuditAction::InvitationCreate,
                input.normalized_email.clone(),
                ControlAuditOutcome::Denied,
                input.created_at_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        let token = InvitationToken::generate();
        let invitation = InvitationRecord::new(InvitationInput {
            id: input.id,
            organization_id: input.organization_id,
            normalized_email: input.normalized_email,
            role: input.role,
            invited_by: actor.identity_id().clone(),
            token_digest: token.digest(),
            created_at_unix_seconds: input.created_at_unix_seconds,
            expires_at_unix_seconds: input.expires_at_unix_seconds,
        })?;
        self.store.create_invitation(&invitation).await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::InvitationCreate,
            invitation.id().as_str(),
            ControlAuditOutcome::Allowed,
            input.created_at_unix_seconds,
        );
        Ok(IssuedInvitation { invitation, token })
    }

    pub async fn accept_invitation(
        &self,
        actor: &DeveloperPrincipal,
        invitation_id: &InvitationId,
        token: &InvitationToken,
        now_unix_seconds: u64,
    ) -> Result<MembershipRecord, OrganizationServiceError> {
        let previous = self
            .store
            .get_invitation(invitation_id)
            .await?
            .ok_or(OrganizationServiceError::NotFound)?;
        let organization = self.organization(previous.organization_id()).await?;
        if previous.status() != InvitationStatus::Pending
            || now_unix_seconds >= previous.expires_at_unix_seconds()
            || previous.normalized_email() != actor.normalized_email()
            || !constant_time_eq(
                previous.token_digest().as_bytes(),
                token.digest().as_bytes(),
            )
        {
            self.audit(
                actor,
                &organization,
                ControlAuditAction::InvitationAccept,
                invitation_id.as_str(),
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::InvalidInvitation);
        }
        let mut accepted = previous.clone();
        accepted.accept(actor.identity_id().clone(), now_unix_seconds)?;
        let membership = MembershipRecord::new(
            previous.organization_id().clone(),
            actor.identity_id().clone(),
            previous.role(),
            now_unix_seconds,
        );
        self.store
            .accept_invitation(&previous, &accepted, &membership)
            .await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::InvitationAccept,
            invitation_id.as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(membership)
    }

    pub async fn list_memberships(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<MembershipRecord>, OrganizationServiceError> {
        let organization = self.organization(organization_id).await?;
        self.require_membership(
            actor,
            organization_id,
            ControlAuditAction::MembershipList,
            now_unix_seconds,
        )
        .await?;
        let memberships = self.store.list_memberships(organization_id, limit).await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::MembershipList,
            "memberships",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(memberships)
    }

    pub async fn change_membership_role(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        target_id: &DeveloperIdentityId,
        role: OrganizationRole,
        now_unix_seconds: u64,
    ) -> Result<MembershipRecord, OrganizationServiceError> {
        let organization = self.organization(organization_id).await?;
        // One person's space has nothing to invite, change, remove, or delete.
        if organization.is_personal() {
            return Err(OrganizationServiceError::PersonalSpace);
        }
        let actor_membership = self
            .require_membership(
                actor,
                organization_id,
                ControlAuditAction::MembershipUpdate,
                now_unix_seconds,
            )
            .await?;
        let previous = self
            .store
            .get_membership(organization_id, target_id)
            .await?
            .ok_or(OrganizationServiceError::NotFound)?;
        if !actor_membership.role().can_manage_members()
            || !actor_membership.role().can_assign(previous.role())
            || !actor_membership.role().can_assign(role)
        {
            self.audit(
                actor,
                &organization,
                ControlAuditAction::MembershipUpdate,
                target_id.as_str(),
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        if previous.role() == OrganizationRole::Owner
            && role != OrganizationRole::Owner
            && self.owner_count(organization_id).await? <= 1
        {
            return Err(OrganizationServiceError::LastOwner);
        }
        let mut next = previous.clone();
        next.change_role(role, now_unix_seconds);
        self.store.replace_membership(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::MembershipUpdate,
            target_id.as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn remove_membership(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        target_id: &DeveloperIdentityId,
        now_unix_seconds: u64,
    ) -> Result<(), OrganizationServiceError> {
        let organization = self.organization(organization_id).await?;
        // One person's space has nothing to invite, change, remove, or delete.
        if organization.is_personal() {
            return Err(OrganizationServiceError::PersonalSpace);
        }
        let actor_membership = self
            .require_membership(
                actor,
                organization_id,
                ControlAuditAction::MembershipDelete,
                now_unix_seconds,
            )
            .await?;
        let target = self
            .store
            .get_membership(organization_id, target_id)
            .await?
            .ok_or(OrganizationServiceError::NotFound)?;
        if !actor_membership.role().can_manage_members()
            || !actor_membership.role().can_assign(target.role())
        {
            self.audit(
                actor,
                &organization,
                ControlAuditAction::MembershipDelete,
                target_id.as_str(),
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        if target.role() == OrganizationRole::Owner && self.owner_count(organization_id).await? <= 1
        {
            return Err(OrganizationServiceError::LastOwner);
        }
        self.store.delete_membership(&target).await?;
        self.audit(
            actor,
            &organization,
            ControlAuditAction::MembershipDelete,
            target_id.as_str(),
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(())
    }

    pub async fn request_organization_deletion(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        now_unix_seconds: u64,
        deadline_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        let previous = self.organization(organization_id).await?;
        // One person's space has nothing to invite, change, remove, or delete.
        if previous.is_personal() {
            return Err(OrganizationServiceError::PersonalSpace);
        }
        let membership = self
            .require_membership(
                actor,
                organization_id,
                ControlAuditAction::OrganizationDeleteRequest,
                now_unix_seconds,
            )
            .await?;
        if !membership.role().can_delete_organization() {
            self.audit(
                actor,
                &previous,
                ControlAuditAction::OrganizationDeleteRequest,
                "organization",
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        let mut next = previous.clone();
        next.request_deletion(now_unix_seconds, deadline_unix_seconds)?;
        self.store.replace_organization(&previous, &next).await?;
        self.audit(
            actor,
            &next,
            ControlAuditAction::OrganizationDeleteRequest,
            "organization",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    pub async fn restore_organization(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        now_unix_seconds: u64,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        let previous = self.organization(organization_id).await?;
        let membership = self
            .require_membership(
                actor,
                organization_id,
                ControlAuditAction::OrganizationRestore,
                now_unix_seconds,
            )
            .await?;
        if !membership.role().can_delete_organization() {
            self.audit(
                actor,
                &previous,
                ControlAuditAction::OrganizationRestore,
                "organization",
                ControlAuditOutcome::Denied,
                now_unix_seconds,
            );
            return Err(OrganizationServiceError::Forbidden);
        }
        let mut next = previous.clone();
        next.restore(now_unix_seconds)?;
        self.store.replace_organization(&previous, &next).await?;
        self.audit(
            actor,
            &next,
            ControlAuditAction::OrganizationRestore,
            "organization",
            ControlAuditOutcome::Allowed,
            now_unix_seconds,
        );
        Ok(next)
    }

    async fn organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<OrganizationRecord, OrganizationServiceError> {
        self.store
            .get_organization(organization_id)
            .await?
            .ok_or(OrganizationServiceError::NotFound)
    }

    async fn require_membership(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        now_unix_seconds: u64,
    ) -> Result<MembershipRecord, OrganizationServiceError> {
        let membership = self
            .store
            .get_membership(organization_id, actor.identity_id())
            .await?;
        if let Some(membership) = membership {
            return Ok(membership);
        }
        self.audit.record(ControlAuditEvent {
            organization_id: organization_id.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: "organization".to_owned(),
            outcome: ControlAuditOutcome::Denied,
            at_unix_seconds: now_unix_seconds,
        });
        Err(OrganizationServiceError::Forbidden)
    }

    async fn owner_count(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<usize, OrganizationServiceError> {
        let limit = NonZeroUsize::new(10_000).expect("non-zero constant");
        Ok(self
            .store
            .list_memberships(organization_id, limit)
            .await?
            .into_iter()
            .filter(|membership| membership.role() == OrganizationRole::Owner)
            .count())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationRecord,
        action: ControlAuditAction,
        target: impl Into<String>,
        outcome: ControlAuditOutcome,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.id().clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: target.into(),
            outcome,
            at_unix_seconds,
        });
    }
}

#[derive(Debug)]
pub enum OrganizationStoreError {
    UnsupportedDurability(Durability),
    RecordScopeMismatch,
    Conflict,
    CorruptRecord,
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for OrganizationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support {durability:?} durability"
                )
            }
            Self::RecordScopeMismatch => formatter.write_str("control record scope does not match"),
            Self::Conflict => formatter.write_str("control record changed concurrently"),
            Self::CorruptRecord => formatter.write_str("control record is corrupt"),
            Self::Keyspace(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Serialization(_) => formatter.write_str("control record serialization failed"),
        }
    }
}

impl Error for OrganizationStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ControlKeyspaceError> for OrganizationStoreError {
    fn from(error: ControlKeyspaceError) -> Self {
        Self::Keyspace(error)
    }
}

impl From<StorageError> for OrganizationStoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for OrganizationStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[derive(Debug)]
pub enum OrganizationServiceError {
    NotFound,
    Forbidden,
    InvalidInvitation,
    LastOwner,
    /// The operation only makes sense for a team with more than one person
    /// in it, and this is a developer's personal space.
    PersonalSpace,
    Model(ControlModelError),
    Store(OrganizationStoreError),
}

impl fmt::Display for OrganizationServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("organization resource was not found"),
            Self::Forbidden => formatter.write_str("organization action is forbidden"),
            Self::InvalidInvitation => formatter.write_str("invitation is invalid or expired"),
            Self::LastOwner => formatter.write_str("organization must retain an owner"),
            Self::PersonalSpace => {
                formatter.write_str("a personal space has exactly one member and cannot be deleted")
            }
            Self::Model(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl Error for OrganizationServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Model(error) => Some(error),
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ControlModelError> for OrganizationServiceError {
    fn from(error: ControlModelError) -> Self {
        Self::Model(error)
    }
}

impl From<OrganizationStoreError> for OrganizationServiceError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Store(error)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    difference == 0
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        ControlPlaneAuthenticator, DeveloperIdentityProvider, DeveloperIdentityStatus,
        DeveloperSessionClaims, DeveloperSessionToken, IdentityProviderError,
    };

    #[derive(Default)]
    struct AuditLog(Mutex<Vec<ControlAuditEvent>>);

    impl ControlAuditSink for AuditLog {
        fn record(&self, event: ControlAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    struct FakeProvider(DeveloperSessionClaims);

    #[async_trait]
    impl DeveloperIdentityProvider for FakeProvider {
        async fn verify_session(
            &self,
            _token: &DeveloperSessionToken,
            _now_unix_seconds: u64,
        ) -> Result<DeveloperSessionClaims, IdentityProviderError> {
            Ok(self.0.clone())
        }
    }

    async fn principal(id: &str, email: &str) -> DeveloperPrincipal {
        let claims = DeveloperSessionClaims {
            developer_identity_id: DeveloperIdentityId::parse(id).expect("developer id"),
            issuer: "https://identity.example.test".to_owned(),
            subject: format!("subject-{id}"),
            audiences: vec!["mako-control-plane".to_owned()],
            normalized_email: email.to_owned(),
            email_verified: true,
            display_name: id.to_owned(),
            session_id: format!("session-{id}"),
            status: DeveloperIdentityStatus::Active,
            credential_epoch: 1,
            authorization_epoch: 1,
            authenticated_at_unix_seconds: 1,
            expires_at_unix_seconds: 1_000,
        };
        let authenticator = ControlPlaneAuthenticator::new(
            Arc::new(FakeProvider(claims)),
            "https://identity.example.test",
            "mako-control-plane",
        )
        .expect("authenticator");
        authenticator
            .authenticate(Some("Bearer developer-session-token"), 10)
            .await
            .expect("principal")
    }

    fn service(audit: Arc<AuditLog>) -> OrganizationService {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let store = OrganizationStore::new(adapter, Durability::Memory).expect("store");
        OrganizationService::new(store, audit)
    }

    #[test]
    fn owner_invites_developer_and_viewer_mutation_is_denied_and_audited() {
        futures::executor::block_on(async {
            let audit = Arc::new(AuditLog::default());
            let service = service(audit.clone());
            let owner = principal("dev_owner000", "owner@example.test").await;
            let invited = principal("dev_member00", "member@example.test").await;
            let organization_id = OrganizationId::parse("org_example00").expect("organization id");
            service
                .create_organization(&owner, organization_id.clone(), "Example", 10)
                .await
                .expect("organization");
            let issued = service
                .invite(
                    &owner,
                    NewInvitation {
                        id: InvitationId::parse("inv_example00").expect("invitation id"),
                        organization_id: organization_id.clone(),
                        normalized_email: "member@example.test".to_owned(),
                        role: OrganizationRole::Developer,
                        created_at_unix_seconds: 11,
                        expires_at_unix_seconds: 100,
                    },
                )
                .await
                .expect("invitation");
            let parsed_token =
                InvitationToken::parse(issued.token.expose_once()).expect("parse invitation token");
            let membership = service
                .accept_invitation(&invited, issued.invitation.id(), &parsed_token, 12)
                .await
                .expect("accept invitation");
            assert_eq!(membership.role(), OrganizationRole::Developer);
            assert_eq!(
                service
                    .list_organizations(&owner, NonZeroUsize::new(10).expect("limit"), 12)
                    .await
                    .expect("owner organizations")[0]
                    .id(),
                &organization_id
            );
            assert_eq!(
                service
                    .list_organizations(&invited, NonZeroUsize::new(10).expect("limit"), 12)
                    .await
                    .expect("invited organizations")[0]
                    .id(),
                &organization_id
            );

            let viewer = service
                .change_membership_role(
                    &owner,
                    &organization_id,
                    invited.identity_id(),
                    OrganizationRole::Viewer,
                    13,
                )
                .await
                .expect("change role");
            assert_eq!(viewer.role(), OrganizationRole::Viewer);
            assert!(matches!(
                service
                    .rename_organization(&invited, &organization_id, "Denied", 14)
                    .await,
                Err(OrganizationServiceError::Forbidden)
            ));
            assert!(audit.0.lock().expect("audit lock").iter().any(|event| {
                event.actor_id == *invited.identity_id()
                    && event.action == ControlAuditAction::OrganizationUpdate
                    && event.outcome == ControlAuditOutcome::Denied
            }));
            service
                .remove_membership(&owner, &organization_id, invited.identity_id(), 15)
                .await
                .expect("remove invited member");
            assert!(
                service
                    .list_organizations(&invited, NonZeroUsize::new(10).expect("limit"), 15)
                    .await
                    .expect("removed member organizations")
                    .is_empty()
            );
        });
    }

    #[test]
    fn last_owner_cannot_be_removed_or_demoted() {
        futures::executor::block_on(async {
            let service = service(Arc::new(AuditLog::default()));
            let owner = principal("dev_owner000", "owner@example.test").await;
            let organization_id = OrganizationId::parse("org_example00").expect("organization id");
            service
                .create_organization(&owner, organization_id.clone(), "Example", 10)
                .await
                .expect("organization");
            assert!(matches!(
                service
                    .remove_membership(&owner, &organization_id, owner.identity_id(), 11)
                    .await,
                Err(OrganizationServiceError::LastOwner)
            ));
            assert!(matches!(
                service
                    .change_membership_role(
                        &owner,
                        &organization_id,
                        owner.identity_id(),
                        OrganizationRole::Administrator,
                        11,
                    )
                    .await,
                Err(OrganizationServiceError::LastOwner)
            ));
        });
    }

    #[test]
    fn invitation_tokens_are_one_time_display_values() {
        let token = InvitationToken::generate();
        assert_eq!(format!("{token:?}"), "InvitationToken([REDACTED])");
        assert!(InvitationToken::parse(token.expose_once()).is_ok());
        assert!(InvitationToken::parse("not-a-token").is_err());
    }

    /// A developer's personal space appears on first use and is the same
    /// space on every later use; nobody else can be let into it and it
    /// cannot be deleted, because one person's space has no one to invite
    /// and nothing to hand over.
    #[test]
    fn a_personal_space_is_created_once_and_refuses_members_and_deletion() {
        futures::executor::block_on(async {
            let audit = Arc::new(AuditLog::default());
            let service = service(audit.clone());
            let alice = principal("dev_alice0000", "alice@example.test").await;
            let mallory = principal("dev_mallory00", "mallory@example.test").await;
            let personal_id = OrganizationId::parse("org_personalalice").expect("id");

            let first = service
                .ensure_personal_organization(&alice, personal_id.clone(), 10)
                .await
                .expect("first use creates the space");
            assert!(first.is_personal());
            assert_eq!(first.name(), alice.display_name());
            let again = service
                .ensure_personal_organization(&alice, personal_id.clone(), 20)
                .await
                .expect("second use reuses the space");
            assert_eq!(again.id(), first.id());
            assert_eq!(
                again.created_at_unix_seconds(),
                10,
                "the second use created a new space instead of reusing the first"
            );

            // Someone else deriving the same id must not adopt it.
            assert!(matches!(
                service
                    .ensure_personal_organization(&mallory, personal_id.clone(), 30)
                    .await,
                Err(OrganizationServiceError::Forbidden)
            ));

            // The owner cannot invite, and cannot delete.
            let refused = service
                .invite(
                    &alice,
                    NewInvitation {
                        id: InvitationId::parse("inv_personal00").expect("invitation id"),
                        organization_id: personal_id.clone(),
                        normalized_email: "friend@example.test".to_owned(),
                        role: OrganizationRole::Developer,
                        created_at_unix_seconds: 40,
                        expires_at_unix_seconds: 40 + 7 * 24 * 60 * 60,
                    },
                )
                .await;
            assert!(
                matches!(refused, Err(OrganizationServiceError::PersonalSpace)),
                "a personal space accepted an invitation: {refused:?}"
            );
            let deletion = service
                .request_organization_deletion(&alice, &personal_id, 50, 50 + 30 * 24 * 60 * 60)
                .await;
            assert!(
                matches!(deletion, Err(OrganizationServiceError::PersonalSpace)),
                "a personal space accepted deletion: {deletion:?}"
            );
        });
    }

    /// A closed period is closed once: whoever derives it first writes the
    /// invoice, a second closing conflicts instead of rewriting history, and
    /// the stored record reads back byte-for-byte what was closed.
    #[test]
    fn an_invoice_closes_exactly_once_and_reads_back_unchanged() {
        futures::executor::block_on(async {
            let store = OrganizationStore::new(
                Arc::new(MemoryAdapter::new()),
                mako_storage::Durability::Memory,
            )
            .expect("store");
            let organization_id = OrganizationId::parse("org_invoices0001").expect("id");
            let usage = std::collections::BTreeMap::from([(
                mako_api::QuotaResource::EdgeInvocationsPerMonth,
                3_000_000_u64,
            )]);
            let rated = mako_billing::rating::rate_period(
                &mako_billing::plan("pro").expect("plan"),
                &mako_billing::rating::default_rate_card(),
                &usage,
            );
            let invoice = InvoiceRecord {
                organization_id: organization_id.clone(),
                period_start_unix_milliseconds: 1_000,
                period_end_unix_milliseconds: 2_000,
                derived_from_unix_milliseconds: 1_000,
                rated: rated.clone(),
                closed_at_unix_seconds: 3,
            };
            store.close_invoice(&invoice).await.expect("first close");

            let mut rewrite = invoice.clone();
            rewrite.rated.total_micro_dollars = 0;
            assert!(
                matches!(
                    store.close_invoice(&rewrite).await,
                    Err(OrganizationStoreError::Conflict)
                ),
                "a closed period was closed again"
            );

            assert_eq!(
                store
                    .get_invoice(&organization_id, 1_000)
                    .await
                    .expect("read")
                    .as_ref(),
                Some(&invoice),
                "the stored invoice is not what was closed"
            );
            assert_eq!(
                store
                    .list_invoices(&organization_id)
                    .await
                    .expect("list")
                    .len(),
                1
            );
            // A period of no time cannot close.
            let mut degenerate = invoice;
            degenerate.period_end_unix_milliseconds = 1_000;
            assert!(store.close_invoice(&degenerate).await.is_err());
        });
    }
}
