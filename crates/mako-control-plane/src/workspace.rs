use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::{
    DeveloperBackupView, DeveloperRestoreRequest, DeveloperRestoreState, DeveloperRestoreView,
    EXPLORER_MAX_RESTORE_REQUESTS_PER_PROJECT, EnvironmentId, TenantScope,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    ControlKeyspace, ControlKeyspaceError, DeveloperPrincipal, LifecycleState, OrganizationStore,
    OrganizationStoreError, ProjectDataPermission, ProjectStore, ProjectStoreError,
    RecoveryJobRecord, RecoveryJobState,
};

const STEP_UP_LIFETIME_SECONDS: u64 = 5 * 60;

#[derive(Clone)]
pub struct DeveloperWorkspaceSecurity {
    signing_key: [u8; 32],
}

impl fmt::Debug for DeveloperWorkspaceSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperWorkspaceSecurity")
            .finish_non_exhaustive()
    }
}

impl DeveloperWorkspaceSecurity {
    #[must_use]
    pub const fn new(signing_key: [u8; 32]) -> Self {
        Self { signing_key }
    }

    pub fn issue_step_up(
        &self,
        actor: &DeveloperPrincipal,
        now_unix_seconds: u64,
    ) -> Result<DeveloperStepUpGrant, DeveloperWorkspaceError> {
        let mut nonce = [0_u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let claims = DeveloperStepUpClaims {
            identity_id: actor.identity_id().as_str().to_owned(),
            session_id: actor.session_id().to_owned(),
            issued_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds: now_unix_seconds.saturating_add(STEP_UP_LIFETIME_SECONDS),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
        let signature = blake3::keyed_hash(&self.signing_key, payload.as_bytes());
        Ok(DeveloperStepUpGrant {
            token: format!(
                "dst1_{payload}.{}",
                URL_SAFE_NO_PAD.encode(signature.as_bytes())
            ),
            expires_at_unix_seconds: claims.expires_at_unix_seconds,
        })
    }

    pub fn validate_step_up(
        &self,
        actor: &DeveloperPrincipal,
        token: &str,
        now_unix_seconds: u64,
    ) -> Result<(), DeveloperWorkspaceError> {
        let (payload, signature) = token
            .strip_prefix("dst1_")
            .and_then(|value| value.split_once('.'))
            .ok_or(DeveloperWorkspaceError::StepUpRequired)?;
        if payload.len() > 2_048 || signature.len() > 128 {
            return Err(DeveloperWorkspaceError::StepUpRequired);
        }
        let actual = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| DeveloperWorkspaceError::StepUpRequired)?;
        let expected = blake3::keyed_hash(&self.signing_key, payload.as_bytes());
        if !constant_time_eq(&actual, expected.as_bytes()) {
            return Err(DeveloperWorkspaceError::StepUpRequired);
        }
        let claims: DeveloperStepUpClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| DeveloperWorkspaceError::StepUpRequired)?,
        )
        .map_err(|_| DeveloperWorkspaceError::StepUpRequired)?;
        if claims.identity_id != actor.identity_id().as_str()
            || claims.session_id != actor.session_id()
            || claims.issued_at_unix_seconds > now_unix_seconds
            || now_unix_seconds >= claims.expires_at_unix_seconds
            || claims
                .expires_at_unix_seconds
                .saturating_sub(claims.issued_at_unix_seconds)
                > STEP_UP_LIFETIME_SECONDS
        {
            return Err(DeveloperWorkspaceError::StepUpRequired);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeveloperStepUpClaims {
    identity_id: String,
    session_id: String,
    issued_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    nonce: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeveloperStepUpGrant {
    pub token: String,
    pub expires_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeveloperRestoreRecord {
    view: DeveloperRestoreView,
    developer_identity_id: String,
    reason_hash: String,
    #[serde(default)]
    orchestrator_job_id: String,
}

#[derive(Clone)]
pub struct DeveloperRestoreService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    security: DeveloperWorkspaceSecurity,
}

impl fmt::Debug for DeveloperRestoreService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeveloperRestoreService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl DeveloperRestoreService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        security: DeveloperWorkspaceSecurity,
    ) -> Result<Self, DeveloperWorkspaceError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(DeveloperWorkspaceError::InvalidConfiguration);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            security,
        })
    }

    pub async fn request(
        &self,
        actor: &DeveloperPrincipal,
        backup: &DeveloperBackupView,
        request: DeveloperRestoreRequest,
        idempotency_key: &str,
        now_unix_seconds: u64,
    ) -> Result<DeveloperRestoreView, DeveloperWorkspaceError> {
        self.authorize(actor, backup.tenant.project_id()).await?;
        self.security
            .validate_step_up(actor, &request.step_up_token, now_unix_seconds)?;
        if request.backup_id != backup.backup_id
            || now_unix_seconds >= backup.retained_until_unix_seconds
            || !safe_text(&request.target_environment_name, 64)
            || !safe_text(&request.reason, 1_024)
            || !(16..=200).contains(&idempotency_key.len())
        {
            return Err(DeveloperWorkspaceError::InvalidRequest);
        }
        let active = self
            .list_records(
                backup.tenant.project_id(),
                NonZeroUsize::new(100).expect("restore list limit"),
            )
            .await?
            .into_iter()
            .filter(|record| {
                !matches!(
                    record.view.state,
                    DeveloperRestoreState::Ready | DeveloperRestoreState::Failed
                )
            })
            .count();
        if active >= EXPLORER_MAX_RESTORE_REQUESTS_PER_PROJECT as usize {
            return Err(DeveloperWorkspaceError::QuotaExceeded);
        }
        let digest = blake3::hash(
            format!(
                "{}:{}:{}",
                actor.identity_id().as_str(),
                backup.backup_id,
                idempotency_key
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let request_id = format!("drr_{}", &digest[..32]);
        let target_environment_id = EnvironmentId::parse(format!("env_recovery_{}", &digest[..20]))
            .map_err(|_| DeveloperWorkspaceError::InvalidRequest)?;
        let record = DeveloperRestoreRecord {
            view: DeveloperRestoreView {
                request_id: request_id.clone(),
                backup_id: backup.backup_id.clone(),
                target: TenantScope::new(backup.tenant.project_id().clone(), target_environment_id),
                state: DeveloperRestoreState::Requested,
                accessible: false,
                overwrite_permitted: false,
                promotion_permitted: false,
                requested_at_unix_seconds: now_unix_seconds,
                updated_at_unix_seconds: now_unix_seconds,
            },
            developer_identity_id: actor.identity_id().as_str().to_owned(),
            reason_hash: blake3::hash(request.reason.as_bytes()).to_hex().to_string(),
            orchestrator_job_id: format!("rcv_dev_{}", &digest[..32]),
        };
        let key = ControlKeyspace::developer_restore_key(
            backup.tenant.project_id(),
            &record.view.request_id,
        )?;
        if let Some(existing) = self.adapter.get(&key).await? {
            let existing: DeveloperRestoreRecord = serde_json::from_slice(&existing)?;
            if existing.developer_identity_id == record.developer_identity_id
                && existing.view.backup_id == record.view.backup_id
            {
                return Ok(existing.view);
            }
            return Err(DeveloperWorkspaceError::Conflict);
        }
        if self
            .projects
            .get_environment(
                record.view.target.project_id(),
                record.view.target.environment_id(),
            )
            .await?
            .is_some()
        {
            return Err(DeveloperWorkspaceError::Conflict);
        }
        let orchestrator = RecoveryJobRecord {
            id: record.orchestrator_job_id.clone(),
            project_id: record.view.target.project_id().clone(),
            backup_id: record.view.backup_id.clone(),
            target: format!(
                "developer-isolated:{}/{}",
                record.view.target.project_id(),
                record.view.target.environment_id()
            ),
            state: RecoveryJobState::Requested,
            version: 1,
            verification_succeeded: false,
            promotion_permitted: false,
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds: backup.retained_until_unix_seconds,
            last_error_class: None,
        };
        let orchestrator_key =
            ControlKeyspace::operator_recovery_job_key(&record.orchestrator_job_id)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        batch.put(&orchestrator_key, serde_json::to_vec(&orchestrator)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::Missing { key },
                    KeyCondition::Missing {
                        key: orchestrator_key,
                    },
                ],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(record.view),
            CompareAndWriteResult::Conflict { .. } => Err(DeveloperWorkspaceError::Conflict),
        }
    }

    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &mako_api::ProjectId,
    ) -> Result<Vec<DeveloperRestoreView>, DeveloperWorkspaceError> {
        self.authorize(actor, project_id).await?;
        let records = self
            .list_records(
                project_id,
                NonZeroUsize::new(100).expect("restore list limit"),
            )
            .await?;
        let mut views = Vec::with_capacity(records.len());
        for mut record in records {
            if !record.orchestrator_job_id.is_empty() {
                let key = ControlKeyspace::operator_recovery_job_key(&record.orchestrator_job_id)?;
                if let Some(bytes) = self.adapter.get(&key).await? {
                    let job: RecoveryJobRecord = serde_json::from_slice(&bytes)?;
                    apply_recovery_progress(&mut record.view, &job);
                }
            }
            // Recovery verification is necessary but not sufficient for
            // developer access. The isolated target must also exist in the
            // project registry and be active; an orchestrator state alone can
            // never turn a restore into an accessible environment.
            if record.view.accessible
                && self
                    .projects
                    .get_environment(
                        record.view.target.project_id(),
                        record.view.target.environment_id(),
                    )
                    .await?
                    .is_none_or(|environment| environment.lifecycle() != LifecycleState::Active)
            {
                record.view.accessible = false;
                record.view.state = DeveloperRestoreState::VerifyingServices;
            }
            views.push(record.view);
        }
        Ok(views)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        project_id: &mako_api::ProjectId,
    ) -> Result<(), DeveloperWorkspaceError> {
        let project = self
            .projects
            .get_project(project_id)
            .await?
            .filter(|project| project.lifecycle() == LifecycleState::Active)
            .ok_or(DeveloperWorkspaceError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?
            .ok_or(DeveloperWorkspaceError::NotFound)?;
        if !membership
            .role()
            .allows_data(ProjectDataPermission::RestoreRequest)
        {
            return Err(DeveloperWorkspaceError::Forbidden);
        }
        Ok(())
    }

    async fn list_records(
        &self,
        project_id: &mako_api::ProjectId,
        limit: NonZeroUsize,
    ) -> Result<Vec<DeveloperRestoreRecord>, DeveloperWorkspaceError> {
        self.adapter
            .scan(ScanRequest::new(
                ControlKeyspace::developer_restores_range(project_id)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?
            .into_iter()
            .map(|entry| serde_json::from_slice(&entry.value).map_err(Into::into))
            .collect()
    }
}

fn apply_recovery_progress(view: &mut DeveloperRestoreView, job: &RecoveryJobRecord) {
    view.updated_at_unix_seconds = job.updated_at_unix_seconds;
    view.accessible = false;
    view.state = match job.state {
        RecoveryJobState::Requested | RecoveryJobState::Preparing => {
            DeveloperRestoreState::Requested
        }
        RecoveryJobState::Restoring => DeveloperRestoreState::Restoring,
        RecoveryJobState::Verifying => DeveloperRestoreState::VerifyingStorage,
        RecoveryJobState::PromotionReady if job.verification_succeeded => {
            view.accessible = true;
            DeveloperRestoreState::Ready
        }
        RecoveryJobState::PromotionReady => DeveloperRestoreState::VerifyingServices,
        RecoveryJobState::Failed | RecoveryJobState::Cancelled | RecoveryJobState::Promoted => {
            DeveloperRestoreState::Failed
        }
    };
}

fn safe_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= maximum
        && !value.chars().any(|character| character.is_control())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[derive(Debug)]
pub enum DeveloperWorkspaceError {
    InvalidConfiguration,
    InvalidRequest,
    StepUpRequired,
    QuotaExceeded,
    NotFound,
    Forbidden,
    Conflict,
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for DeveloperWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "developer workspace request is invalid",
            Self::StepUpRequired => "recent password verification is required",
            Self::QuotaExceeded => "developer restore request quota is exhausted",
            Self::NotFound | Self::Forbidden => "developer workspace resource was not found",
            Self::Conflict => "developer workspace resource changed concurrently",
            _ => "developer workspace service is unavailable",
        })
    }
}

impl Error for DeveloperWorkspaceError {}

macro_rules! workspace_error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for DeveloperWorkspaceError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

workspace_error_from!(ProjectStoreError, Project);
workspace_error_from!(OrganizationStoreError, Organization);
workspace_error_from!(ControlKeyspaceError, Keyspace);
workspace_error_from!(StorageError, Storage);
workspace_error_from!(serde_json::Error, Serialization);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeveloperIdentityId;
    use mako_api::ProjectId;

    #[test]
    fn step_up_is_session_bound_expiring_and_tamper_evident() {
        let security = DeveloperWorkspaceSecurity::new([7; 32]);
        let actor = DeveloperPrincipal::for_test(
            DeveloperIdentityId::parse("dev_abcdefgh").expect("developer"),
            "developer@example.com",
        );
        let grant = security.issue_step_up(&actor, 100).expect("grant");
        security
            .validate_step_up(&actor, &grant.token, 101)
            .expect("valid");
        assert!(
            security
                .validate_step_up(&actor, &format!("{}x", grant.token), 101)
                .is_err()
        );
        assert!(
            security
                .validate_step_up(&actor, &grant.token, grant.expires_at_unix_seconds)
                .is_err()
        );
    }

    #[test]
    fn developer_recovery_never_treats_promotion_as_success() {
        let project = ProjectId::parse("prj_abcdefgh").expect("project");
        let environment = EnvironmentId::parse("env_recovery_abcdefgh").expect("environment");
        let mut view = DeveloperRestoreView {
            request_id: "drr_0123456789abcdef0123456789abcdef".to_owned(),
            backup_id: "backup_abcdefgh".to_owned(),
            target: TenantScope::new(project.clone(), environment),
            state: DeveloperRestoreState::Requested,
            accessible: false,
            overwrite_permitted: false,
            promotion_permitted: false,
            requested_at_unix_seconds: 100,
            updated_at_unix_seconds: 100,
        };
        let mut job = RecoveryJobRecord {
            id: "rcv_dev_0123456789abcdef0123456789abcdef".to_owned(),
            project_id: project,
            backup_id: view.backup_id.clone(),
            target: "developer-isolated:test".to_owned(),
            state: RecoveryJobState::PromotionReady,
            version: 5,
            verification_succeeded: true,
            promotion_permitted: false,
            created_at_unix_seconds: 100,
            updated_at_unix_seconds: 105,
            expires_at_unix_seconds: 1_000,
            last_error_class: None,
        };
        apply_recovery_progress(&mut view, &job);
        assert_eq!(view.state, DeveloperRestoreState::Ready);
        assert!(view.accessible);
        assert!(!view.overwrite_permitted);
        assert!(!view.promotion_permitted);

        job.state = RecoveryJobState::Promoted;
        apply_recovery_progress(&mut view, &job);
        assert_eq!(view.state, DeveloperRestoreState::Failed);
        assert!(!view.accessible);
    }
}
