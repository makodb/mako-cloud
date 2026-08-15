use std::{
    array,
    collections::BTreeSet,
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use base64::Engine as _;
use mako_api::{
    ArtifactGrantView, DataJobCreateRequest, DataJobKind, DataJobManifest, DataJobProgress,
    DataJobState, DataJobView, EXPLORER_ARTIFACT_GRANT_SECONDS,
    EXPLORER_ARTIFACT_RETENTION_SECONDS, EXPLORER_MAX_ACTIVE_JOBS_PER_TENANT,
    EXPLORER_MAX_DOCUMENT_BYTES,
};
use mako_internal_rpc::{
    ControlToDataClient, DataJobExportPageInput, DataJobExportPageOutput, DataJobImportBatchInput,
    DataJobImportBatchOutput, IdentityAdminCommand, IdentityAdminOperation,
    IdentityAdminPermission, InternalClientError,
};
use mako_object_store::{DataJobArtifactKind, ObjectAddress, ObjectStore, ObjectStoreError};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    CollectionAdminError, CollectionAdminService, ControlAuditAction, ControlAuditEvent,
    ControlAuditOutcome, ControlAuditSink, ControlKeyspace, ControlKeyspaceError,
    DeveloperPrincipal, LifecycleState, OrganizationStore, OrganizationStoreError,
    ProjectDataPermission, ProjectStore, ProjectStoreError,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DataJobRecord {
    view: DataJobView,
    upload_digest: Option<String>,
    output_digest: Option<String>,
    cancel_requested: bool,
    #[serde(default)]
    execution_attempt: u64,
    #[serde(default)]
    worker_failures: u8,
}

const DATA_JOB_STATE_COUNT: usize = 10;

/// Fixed-cardinality transition counters for data jobs. The dimensions are a
/// closed enum; tenant, actor, artifact, document, and email values can never
/// become metric labels.
#[derive(Default)]
struct DataJobMetrics {
    transitions_by_state: [AtomicU64; DATA_JOB_STATE_COUNT],
}

impl DataJobMetrics {
    fn observe_state(&self, state: DataJobState) {
        self.transitions_by_state[data_job_state_index(state)].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> DataJobMetricsSnapshot {
        DataJobMetricsSnapshot {
            transitions_by_state: self
                .transitions_by_state
                .each_ref()
                .map(|value| value.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataJobMetricsSnapshot {
    pub transitions_by_state: [u64; DATA_JOB_STATE_COUNT],
}

#[derive(Clone)]
pub struct DataJobService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    collections: CollectionAdminService,
    objects: Arc<dyn ObjectStore>,
    artifact_signing_key: [u8; 32],
    public_origin: String,
    data_plane: ControlToDataClient,
    audit: Arc<dyn ControlAuditSink>,
    metrics: Arc<DataJobMetrics>,
}

impl fmt::Debug for DataJobService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DataJobService")
            .field("durability", &self.durability)
            .field("public_origin", &self.public_origin)
            .finish_non_exhaustive()
    }
}

impl DataJobService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        collections: CollectionAdminService,
        objects: Arc<dyn ObjectStore>,
        artifact_signing_key: [u8; 32],
        public_origin: impl Into<String>,
        data_plane: ControlToDataClient,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Result<Self, DataJobError> {
        let public_origin = public_origin.into();
        if adapter.capabilities().strongest_durability < durability
            || !public_origin.starts_with("https://") && !public_origin.starts_with("http://")
        {
            return Err(DataJobError::InvalidConfiguration);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            collections,
            objects,
            artifact_signing_key,
            public_origin: public_origin.trim_end_matches('/').to_owned(),
            data_plane,
            audit,
            metrics: Arc::new(DataJobMetrics {
                transitions_by_state: array::from_fn(|_| AtomicU64::new(0)),
            }),
        })
    }

    #[must_use]
    pub fn metrics_snapshot(&self) -> DataJobMetricsSnapshot {
        self.metrics.snapshot()
    }

    pub async fn create(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        request: DataJobCreateRequest,
        idempotency_key: &str,
        now_unix_seconds: u64,
    ) -> Result<DataJobView, DataJobError> {
        let role = self
            .authorize(actor, tenant, job_permission(request.kind))
            .await?;
        if request.kind == DataJobKind::Import && request.conflict_strategy.is_none()
            || request.kind == DataJobKind::Export && request.conflict_strategy.is_some()
            || idempotency_key.len() < 16
            || idempotency_key.len() > 200
        {
            return Err(DataJobError::InvalidRequest);
        }
        self.collections
            .get_collection(actor, tenant, &request.collection_id, now_unix_seconds)
            .await
            .map_err(|error| match error {
                CollectionAdminError::NotFound | CollectionAdminError::Forbidden => {
                    DataJobError::NotFound
                }
                other => DataJobError::Collection(other),
            })?;
        let active = self
            .list_records(tenant, NonZeroUsize::new(1_000).expect("job list limit"))
            .await?
            .into_iter()
            .filter(|record| !job_is_terminal(record.view.state))
            .count();
        if active >= EXPLORER_MAX_ACTIVE_JOBS_PER_TENANT as usize {
            return Err(DataJobError::QuotaExceeded);
        }
        let job_id = format!(
            "djob_{}",
            &blake3::hash(
                format!(
                    "{}:{}:{}:{}:{}",
                    tenant.project_id(),
                    tenant.environment_id(),
                    actor.identity_id().as_str(),
                    idempotency_key,
                    request.collection_id
                )
                .as_bytes()
            )
            .to_hex()[..32]
        );
        let state = match request.kind {
            DataJobKind::Import => DataJobState::AwaitingUpload,
            DataJobKind::Export => DataJobState::Queued,
        };
        let record = DataJobRecord {
            view: DataJobView {
                job_id: job_id.clone(),
                kind: request.kind,
                state,
                tenant: tenant.clone(),
                collection_id: request.collection_id,
                creator_id: actor.identity_id().as_str().to_owned(),
                conflict_strategy: request.conflict_strategy,
                progress: empty_progress(),
                errors: Vec::new(),
                manifest: None,
                created_at_unix_seconds: now_unix_seconds,
                updated_at_unix_seconds: now_unix_seconds,
                expires_at_unix_seconds: now_unix_seconds
                    .saturating_add(EXPLORER_ARTIFACT_RETENTION_SECONDS),
            },
            upload_digest: None,
            output_digest: None,
            cancel_requested: false,
            execution_attempt: 0,
            worker_failures: 0,
        };
        let key =
            ControlKeyspace::data_job_key(tenant.project_id(), tenant.environment_id(), &job_id)?;
        if let Some(existing) = self.adapter.get(&key).await? {
            let existing: DataJobRecord = serde_json::from_slice(&existing)?;
            if existing.view.kind == request.kind
                && existing.view.collection_id == record.view.collection_id
                && existing.view.creator_id == record.view.creator_id
            {
                return Ok(existing.view);
            }
            return Err(DataJobError::Conflict);
        }
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        for index_key in index_keys(&record.view)? {
            batch.put(index_key, job_id.as_bytes());
        }
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => {
                self.metrics.observe_state(record.view.state);
                let _ = role;
                self.audit_success(
                    actor,
                    tenant,
                    ControlAuditAction::DataJobCreate,
                    &format!("{}:{}", record.view.collection_id, record.view.job_id),
                    now_unix_seconds,
                )
                .await?;
                Ok(record.view)
            }
            CompareAndWriteResult::Conflict { .. } => Err(DataJobError::Conflict),
        }
    }

    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
    ) -> Result<DataJobView, DataJobError> {
        self.authorize(actor, tenant, ProjectDataPermission::DataRead)
            .await?;
        let record = self.get_record(tenant, job_id).await?;
        Ok(record.view)
    }

    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        limit: NonZeroUsize,
    ) -> Result<Vec<DataJobView>, DataJobError> {
        self.authorize(actor, tenant, ProjectDataPermission::DataRead)
            .await?;
        if limit.get() > 100 {
            return Err(DataJobError::InvalidRequest);
        }
        self.list_records(tenant, limit)
            .await
            .map(|records| records.into_iter().map(|record| record.view).collect())
    }

    pub async fn artifact_grant(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        method: ArtifactMethod,
        now_unix_seconds: u64,
    ) -> Result<ArtifactGrantView, DataJobError> {
        let record = self.get_record(tenant, job_id).await?;
        self.authorize(actor, tenant, job_permission(record.view.kind))
            .await?;
        if now_unix_seconds >= record.view.expires_at_unix_seconds {
            return Err(DataJobError::InvalidState);
        }
        let digest = match method {
            ArtifactMethod::Upload if record.view.state == DataJobState::AwaitingUpload => None,
            ArtifactMethod::Download if record.view.state == DataJobState::Succeeded => {
                record.output_digest.clone()
            }
            _ => return Err(DataJobError::InvalidState),
        };
        let expires = now_unix_seconds.saturating_add(EXPLORER_ARTIFACT_GRANT_SECONDS);
        let claims = ArtifactClaims {
            tenant: tenant.clone(),
            job_id: job_id.to_owned(),
            actor_id: actor.identity_id().as_str().to_owned(),
            method,
            digest: digest.clone(),
            expires_at_unix_seconds: expires,
        };
        let token = self.sign_artifact_claims(&claims)?;
        let view = ArtifactGrantView {
            job_id: job_id.to_owned(),
            method: method.as_str().to_owned(),
            url: format!(
                "{}/v1/projects/{}/environments/{}/data-jobs/{job_id}/artifact?grant={token}",
                self.public_origin,
                tenant.project_id(),
                tenant.environment_id()
            ),
            digest,
            expires_at_unix_seconds: expires,
        };
        self.audit_success(
            actor,
            tenant,
            match method {
                ArtifactMethod::Upload => ControlAuditAction::DataJobArtifactUpload,
                ArtifactMethod::Download => ControlAuditAction::DataJobArtifactDownload,
            },
            &format!("{}:{job_id}:grant", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(view)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upload_import(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        grant: &str,
        digest: &str,
        bytes: Arc<[u8]>,
        now_unix_seconds: u64,
    ) -> Result<DataJobView, DataJobError> {
        self.authorize(actor, tenant, ProjectDataPermission::Import)
            .await?;
        self.validate_artifact_grant(
            actor,
            tenant,
            job_id,
            ArtifactMethod::Upload,
            grant,
            now_unix_seconds,
        )?;
        if bytes.is_empty()
            || bytes.len() > mako_object_store::MAX_DATA_JOB_ARTIFACT_BYTES
            || sha256_digest(&bytes) != digest
        {
            return Err(DataJobError::InvalidArtifact);
        }
        validate_json_lines(&bytes)?;
        let address = ObjectAddress::data_job_artifact(
            tenant.clone(),
            job_id,
            DataJobArtifactKind::ImportUpload,
            digest,
        )?;
        self.objects.put_immutable(tenant, &address, bytes).await?;
        let record = self
            .update_record(tenant, job_id, |record| {
                if record.view.state != DataJobState::AwaitingUpload {
                    return Err(DataJobError::InvalidState);
                }
                record.upload_digest = Some(digest.to_owned());
                record.view.state = DataJobState::DryRun;
                record.view.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
        self.audit_success(
            actor,
            tenant,
            ControlAuditAction::DataJobArtifactUpload,
            &format!("{}:{job_id}:stored", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(record.view)
    }

    pub async fn download_export(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        grant: &str,
        now_unix_seconds: u64,
    ) -> Result<Arc<[u8]>, DataJobError> {
        self.authorize(actor, tenant, ProjectDataPermission::Export)
            .await?;
        let claims = self.validate_artifact_grant(
            actor,
            tenant,
            job_id,
            ArtifactMethod::Download,
            grant,
            now_unix_seconds,
        )?;
        let digest = claims.digest.ok_or(DataJobError::InvalidArtifactGrant)?;
        let record = self.get_record(tenant, job_id).await?;
        if now_unix_seconds >= record.view.expires_at_unix_seconds
            || record.view.state != DataJobState::Succeeded
            || record.output_digest.as_deref() != Some(digest.as_str())
        {
            return Err(DataJobError::InvalidState);
        }
        let address = ObjectAddress::data_job_artifact(
            tenant.clone(),
            job_id,
            DataJobArtifactKind::ExportOutput,
            &digest,
        )?;
        let bytes = self
            .objects
            .get(tenant, &address)
            .await?
            .ok_or(DataJobError::InvalidArtifact)?;
        if sha256_digest(&bytes) != digest {
            return Err(DataJobError::InvalidArtifact);
        }
        self.audit_success(
            actor,
            tenant,
            ControlAuditAction::DataJobArtifactDownload,
            &format!("{}:{job_id}:read", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(bytes)
    }

    pub async fn dry_run_import(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        expected_upload_digest: &str,
        schema_version: u64,
        now_unix_seconds: u64,
    ) -> Result<DataJobView, DataJobError> {
        self.authorize(actor, tenant, ProjectDataPermission::Import)
            .await?;
        let record = self.get_record(tenant, job_id).await?;
        if record.view.state != DataJobState::DryRun || schema_version == 0 {
            return Err(DataJobError::InvalidState);
        }
        let digest = record
            .upload_digest
            .as_deref()
            .ok_or(DataJobError::InvalidState)?;
        if digest != expected_upload_digest {
            return Err(DataJobError::Conflict);
        }
        let address = ObjectAddress::data_job_artifact(
            tenant.clone(),
            job_id,
            DataJobArtifactKind::ImportUpload,
            digest,
        )?;
        let bytes = self
            .objects
            .get(tenant, &address)
            .await?
            .ok_or(DataJobError::InvalidArtifact)?;
        let (rows, byte_count) = validate_json_lines(&bytes)?;
        let record = self
            .update_record(tenant, job_id, |record| {
                record.view.state = DataJobState::AwaitingConfirmation;
                record.view.progress.processed = rows;
                record.view.manifest = Some(DataJobManifest {
                    format_version: 1,
                    tenant: tenant.clone(),
                    collection_id: record.view.collection_id.clone(),
                    schema_version,
                    snapshot: None,
                    row_count: rows,
                    byte_count,
                    digest: digest.to_owned(),
                    finalized_at_unix_seconds: now_unix_seconds,
                });
                record.view.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
        self.audit_success(
            actor,
            tenant,
            ControlAuditAction::DataJobDryRun,
            &format!("{}:{job_id}", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(record.view)
    }

    pub async fn confirm(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        expected_manifest_digest: &str,
        acknowledge_partial_import_cancellation: bool,
        now_unix_seconds: u64,
    ) -> Result<DataJobView, DataJobError> {
        let record = self.get_record(tenant, job_id).await?;
        self.authorize(actor, tenant, job_permission(record.view.kind))
            .await?;
        let record = self
            .update_record(tenant, job_id, |record| {
                if record.view.state != DataJobState::AwaitingConfirmation
                    || record.view.kind == DataJobKind::Import
                        && !acknowledge_partial_import_cancellation
                    || record
                        .view
                        .manifest
                        .as_ref()
                        .is_none_or(|manifest| manifest.digest != expected_manifest_digest)
                {
                    return Err(DataJobError::InvalidState);
                }
                record.view.state = DataJobState::Queued;
                if record.view.kind == DataJobKind::Import {
                    record.view.progress = empty_progress();
                }
                record.view.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
        self.audit_success(
            actor,
            tenant,
            ControlAuditAction::DataJobConfirm,
            &format!("{}:{job_id}", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(record.view)
    }

    pub async fn cancel(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        now_unix_seconds: u64,
    ) -> Result<DataJobView, DataJobError> {
        let record = self.get_record(tenant, job_id).await?;
        self.authorize(actor, tenant, job_permission(record.view.kind))
            .await?;
        let record = self
            .update_record(tenant, job_id, |record| {
                if job_is_terminal(record.view.state) {
                    return Err(DataJobError::InvalidState);
                }
                record.cancel_requested = true;
                record.view.state = if matches!(
                    record.view.state,
                    DataJobState::Queued | DataJobState::Running
                ) {
                    DataJobState::Cancelling
                } else {
                    DataJobState::Cancelled
                };
                record.view.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
        self.audit_success(
            actor,
            tenant,
            ControlAuditAction::DataJobCancel,
            &format!("{}:{job_id}", record.view.collection_id),
            now_unix_seconds,
        )
        .await?;
        Ok(record.view)
    }

    /// Execute a bounded pass over queued jobs. Running imports resume from
    /// durable row progress; running exports restart from a new snapshot and
    /// publish only after the complete immutable artifact is finalized.
    pub async fn run_worker_once(
        &self,
        now_unix_seconds: u64,
    ) -> Result<DataJobWorkerReport, DataJobError> {
        let mut report = DataJobWorkerReport::default();
        let records = self
            .list_all_records(NonZeroUsize::new(1_000).expect("worker limit"))
            .await?;
        for record in records.into_iter().take(16) {
            if now_unix_seconds >= record.view.expires_at_unix_seconds
                && record.view.state != DataJobState::Expired
            {
                if self.cleanup_artifacts(&record).await.is_err() {
                    report.deferred = report.deferred.saturating_add(1);
                    continue;
                }
                self.update_record(&record.view.tenant, &record.view.job_id, |record| {
                    record.view.state = DataJobState::Expired;
                    record.view.updated_at_unix_seconds = now_unix_seconds;
                    Ok(())
                })
                .await?;
                report.expired = report.expired.saturating_add(1);
                continue;
            }
            if record.view.state == DataJobState::Cancelling || record.cancel_requested {
                self.mark_cancelled(&record.view.tenant, &record.view.job_id, now_unix_seconds)
                    .await?;
                report.cancelled = report.cancelled.saturating_add(1);
                continue;
            }
            if !matches!(
                record.view.state,
                DataJobState::Queued | DataJobState::Running
            ) {
                continue;
            }
            let record = if record.view.state == DataJobState::Queued {
                self.update_record(&record.view.tenant, &record.view.job_id, |record| {
                    if record.view.state != DataJobState::Queued {
                        return Err(DataJobError::Conflict);
                    }
                    record.view.state = DataJobState::Running;
                    record.execution_attempt = record.execution_attempt.saturating_add(1);
                    record.view.updated_at_unix_seconds = now_unix_seconds;
                    Ok(())
                })
                .await?
            } else if record.view.kind == DataJobKind::Export {
                self.update_record(&record.view.tenant, &record.view.job_id, |record| {
                    if record.view.state != DataJobState::Running {
                        return Err(DataJobError::Conflict);
                    }
                    record.execution_attempt = record.execution_attempt.saturating_add(1);
                    record.view.updated_at_unix_seconds = now_unix_seconds;
                    Ok(())
                })
                .await?
            } else {
                record
            };
            report.claimed = report.claimed.saturating_add(1);
            let result = match record.view.kind {
                DataJobKind::Import => self.execute_import(&record, now_unix_seconds).await,
                DataJobKind::Export => self.execute_export(&record, now_unix_seconds).await,
            };
            match result {
                Ok(DataJobState::Succeeded) => {
                    report.succeeded = report.succeeded.saturating_add(1);
                }
                Ok(DataJobState::Cancelled) => {
                    report.cancelled = report.cancelled.saturating_add(1);
                }
                Ok(_) => {}
                Err(error) => {
                    let terminal =
                        !worker_error_is_retryable(&error) || record.worker_failures >= 2;
                    self.record_worker_failure(
                        &record.view.tenant,
                        &record.view.job_id,
                        now_unix_seconds,
                        terminal,
                    )
                    .await?;
                    if terminal {
                        report.failed = report.failed.saturating_add(1);
                    } else {
                        report.deferred = report.deferred.saturating_add(1);
                    }
                }
            }
        }
        Ok(report)
    }

    async fn cleanup_artifacts(&self, record: &DataJobRecord) -> Result<(), DataJobError> {
        if let Some(digest) = record.upload_digest.as_deref() {
            let address = ObjectAddress::data_job_artifact(
                record.view.tenant.clone(),
                &record.view.job_id,
                DataJobArtifactKind::ImportUpload,
                digest,
            )?;
            self.objects.delete(&record.view.tenant, &address).await?;
        }
        if let Some(digest) = record.output_digest.as_deref() {
            let address = ObjectAddress::data_job_artifact(
                record.view.tenant.clone(),
                &record.view.job_id,
                DataJobArtifactKind::ExportOutput,
                digest,
            )?;
            self.objects.delete(&record.view.tenant, &address).await?;
        }
        if let Some(digest) = record
            .view
            .manifest
            .as_ref()
            .map(|manifest| manifest.digest.as_str())
        {
            let address = ObjectAddress::data_job_artifact(
                record.view.tenant.clone(),
                &record.view.job_id,
                DataJobArtifactKind::Manifest,
                digest,
            )?;
            self.objects.delete(&record.view.tenant, &address).await?;
        }
        Ok(())
    }

    async fn execute_import(
        &self,
        record: &DataJobRecord,
        now_unix_seconds: u64,
    ) -> Result<DataJobState, DataJobError> {
        let digest = record
            .upload_digest
            .as_deref()
            .ok_or(DataJobError::InvalidState)?;
        let manifest = record
            .view
            .manifest
            .as_ref()
            .ok_or(DataJobError::InvalidState)?;
        let conflict_strategy = record
            .view
            .conflict_strategy
            .ok_or(DataJobError::InvalidState)?;
        let address = ObjectAddress::data_job_artifact(
            record.view.tenant.clone(),
            &record.view.job_id,
            DataJobArtifactKind::ImportUpload,
            digest,
        )?;
        let bytes = self
            .objects
            .get(&record.view.tenant, &address)
            .await?
            .ok_or(DataJobError::InvalidArtifact)?;
        if sha256_digest(&bytes) != digest {
            return Err(DataJobError::InvalidArtifact);
        }
        let rows = std::str::from_utf8(&bytes)
            .map_err(|_| DataJobError::InvalidArtifact)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        let mut offset = usize::try_from(record.view.progress.processed)
            .map_err(|_| DataJobError::InvalidState)?;
        if offset > rows.len() {
            return Err(DataJobError::InvalidState);
        }
        while offset < rows.len() {
            let latest = self
                .get_record(&record.view.tenant, &record.view.job_id)
                .await?;
            if latest.cancel_requested || latest.view.state == DataJobState::Cancelling {
                self.mark_cancelled(&record.view.tenant, &record.view.job_id, now_unix_seconds)
                    .await?;
                return Ok(DataJobState::Cancelled);
            }
            let end = offset.saturating_add(32).min(rows.len());
            let input = DataJobImportBatchInput {
                job_id: record.view.job_id.clone(),
                collection_id: record.view.collection_id.as_str().to_owned(),
                schema_version: manifest.schema_version,
                conflict_strategy,
                start_row: offset as u64,
                rows: rows[offset..end].to_vec(),
            };
            let result: DataJobImportBatchOutput = self.data_plane.administer(
                &record.view.tenant,
                &worker_request_id(&record.view.job_id, offset as u64),
                &worker_idempotency_key(&record.view.job_id, offset as u64),
                &IdentityAdminCommand {
                    operation: IdentityAdminOperation::ImportDataJobBatch,
                    actor_id: "system/data-job-worker".to_owned(),
                    permissions: BTreeSet::from([IdentityAdminPermission::ExecuteDataJobs]),
                    input: serde_json::to_value(input)?,
                },
            )?;
            self.update_record(&record.view.tenant, &record.view.job_id, |record| {
                if record.view.state != DataJobState::Running || record.cancel_requested {
                    return Err(DataJobError::InvalidState);
                }
                record.view.progress.processed = record
                    .view
                    .progress
                    .processed
                    .saturating_add(result.processed);
                record.view.progress.committed = record
                    .view
                    .progress
                    .committed
                    .saturating_add(result.committed);
                record.view.progress.failed =
                    record.view.progress.failed.saturating_add(result.failed);
                record.view.progress.skipped =
                    record.view.progress.skipped.saturating_add(result.skipped);
                for error in &result.errors {
                    if record.view.errors.len() < 100 {
                        record
                            .view
                            .errors
                            .push(format!("row_{}:{}", error.row, error.code));
                    }
                }
                record.view.updated_at_unix_seconds = now_unix_seconds;
                Ok(())
            })
            .await?;
            offset = end;
        }
        self.update_record(&record.view.tenant, &record.view.job_id, |record| {
            if record.view.state != DataJobState::Running {
                return Err(DataJobError::InvalidState);
            }
            record.view.state = DataJobState::Succeeded;
            record.view.progress.bytes = bytes.len() as u64;
            record.view.updated_at_unix_seconds = now_unix_seconds;
            Ok(())
        })
        .await?;
        Ok(DataJobState::Succeeded)
    }

    async fn execute_export(
        &self,
        record: &DataJobRecord,
        now_unix_seconds: u64,
    ) -> Result<DataJobState, DataJobError> {
        let mut bytes = Vec::new();
        let mut cursor = None;
        let mut row_count = 0_u64;
        let mut snapshot = None;
        let mut schema_version = None;
        let mut page_number = 0_u64;
        loop {
            let latest = self
                .get_record(&record.view.tenant, &record.view.job_id)
                .await?;
            if latest.cancel_requested || latest.view.state == DataJobState::Cancelling {
                self.mark_cancelled(&record.view.tenant, &record.view.job_id, now_unix_seconds)
                    .await?;
                return Ok(DataJobState::Cancelled);
            }
            let input = DataJobExportPageInput {
                job_id: record.view.job_id.clone(),
                collection_id: record.view.collection_id.as_str().to_owned(),
                cursor: cursor.clone(),
                limit: 64,
            };
            let page: DataJobExportPageOutput = self.data_plane.administer(
                &record.view.tenant,
                &worker_request_id(
                    &format!(
                        "{}:attempt:{}",
                        record.view.job_id, record.execution_attempt
                    ),
                    page_number,
                ),
                &worker_idempotency_key(
                    &format!(
                        "{}:attempt:{}",
                        record.view.job_id, record.execution_attempt
                    ),
                    page_number,
                ),
                &IdentityAdminCommand {
                    operation: IdentityAdminOperation::ExportDataJobPage,
                    actor_id: "system/data-job-worker".to_owned(),
                    permissions: BTreeSet::from([IdentityAdminPermission::ExecuteDataJobs]),
                    input: serde_json::to_value(input)?,
                },
            )?;
            if snapshot
                .as_ref()
                .is_some_and(|value| value != &page.snapshot)
                || schema_version.is_some_and(|value| value != page.schema_version)
            {
                return Err(DataJobError::InvalidState);
            }
            snapshot.get_or_insert(page.snapshot.clone());
            schema_version.get_or_insert(page.schema_version);
            for row in page.rows {
                serde_json::to_writer(&mut bytes, &row)?;
                bytes.push(b'\n');
                row_count = row_count.saturating_add(1);
                if bytes.len() > mako_object_store::MAX_DATA_JOB_ARTIFACT_BYTES {
                    return Err(DataJobError::QuotaExceeded);
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
            page_number = page_number
                .checked_add(1)
                .ok_or(DataJobError::QuotaExceeded)?;
            if page_number > 100_000 {
                return Err(DataJobError::QuotaExceeded);
            }
        }
        if bytes.is_empty() {
            bytes.extend_from_slice(b"\n");
        }
        let digest = sha256_digest(&bytes);
        let address = ObjectAddress::data_job_artifact(
            record.view.tenant.clone(),
            &record.view.job_id,
            DataJobArtifactKind::ExportOutput,
            &digest,
        )?;
        self.objects
            .put_immutable(&record.view.tenant, &address, Arc::from(bytes.clone()))
            .await?;
        self.update_record(&record.view.tenant, &record.view.job_id, |record| {
            if record.view.state != DataJobState::Running || record.cancel_requested {
                return Err(DataJobError::InvalidState);
            }
            record.output_digest = Some(digest.clone());
            record.view.state = DataJobState::Succeeded;
            record.view.progress.processed = row_count;
            record.view.progress.exported = row_count;
            record.view.progress.bytes = bytes.len() as u64;
            record.view.manifest = Some(DataJobManifest {
                format_version: 1,
                tenant: record.view.tenant.clone(),
                collection_id: record.view.collection_id.clone(),
                schema_version: schema_version.unwrap_or(1),
                snapshot,
                row_count,
                byte_count: bytes.len() as u64,
                digest: digest.clone(),
                finalized_at_unix_seconds: now_unix_seconds,
            });
            record.view.updated_at_unix_seconds = now_unix_seconds;
            Ok(())
        })
        .await?;
        Ok(DataJobState::Succeeded)
    }

    async fn mark_cancelled(
        &self,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        now_unix_seconds: u64,
    ) -> Result<(), DataJobError> {
        self.update_record(tenant, job_id, |record| {
            record.cancel_requested = true;
            record.view.state = DataJobState::Cancelled;
            record.view.updated_at_unix_seconds = now_unix_seconds;
            Ok(())
        })
        .await
        .map(|_| ())
    }

    async fn record_worker_failure(
        &self,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        now_unix_seconds: u64,
        terminal: bool,
    ) -> Result<(), DataJobError> {
        self.update_record(tenant, job_id, |record| {
            if !job_is_terminal(record.view.state) {
                record.worker_failures = record.worker_failures.saturating_add(1);
                record.view.state = if terminal {
                    DataJobState::Failed
                } else {
                    DataJobState::Queued
                };
                if record.view.errors.len() < 100 {
                    record.view.errors.push(if terminal {
                        "worker_failure_terminal".to_owned()
                    } else {
                        "worker_dependency_retry_scheduled".to_owned()
                    });
                }
                record.view.updated_at_unix_seconds = now_unix_seconds;
            }
            Ok(())
        })
        .await
        .map(|_| ())
    }

    async fn audit_success(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        action: ControlAuditAction,
        target_suffix: &str,
        now_unix_seconds: u64,
    ) -> Result<(), DataJobError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(DataJobError::NotFound)?;
        self.audit.record(ControlAuditEvent {
            organization_id: project.organization_id().clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}:{target_suffix}",
                tenant.project_id(),
                tenant.environment_id()
            ),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds: now_unix_seconds,
        });
        Ok(())
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        permission: ProjectDataPermission,
    ) -> Result<crate::OrganizationRole, DataJobError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .filter(|project| project.lifecycle() == LifecycleState::Active)
            .ok_or(DataJobError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .filter(|environment| environment.lifecycle() == LifecycleState::Active)
            .ok_or(DataJobError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?
            .ok_or(DataJobError::NotFound)?;
        if !membership.role().allows_data(permission) {
            return Err(DataJobError::Forbidden);
        }
        Ok(membership.role())
    }

    async fn get_record(
        &self,
        tenant: &mako_api::TenantScope,
        job_id: &str,
    ) -> Result<DataJobRecord, DataJobError> {
        validate_job_id(job_id)?;
        let key =
            ControlKeyspace::data_job_key(tenant.project_id(), tenant.environment_id(), job_id)?;
        let record: DataJobRecord = serde_json::from_slice(
            &self
                .adapter
                .get(&key)
                .await?
                .ok_or(DataJobError::NotFound)?,
        )?;
        if record.view.tenant != *tenant || record.view.job_id != job_id {
            return Err(DataJobError::NotFound);
        }
        Ok(record)
    }

    async fn list_records(
        &self,
        tenant: &mako_api::TenantScope,
        limit: NonZeroUsize,
    ) -> Result<Vec<DataJobRecord>, DataJobError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::data_jobs_range(tenant.project_id(), tenant.environment_id())?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        entries
            .into_iter()
            .map(|entry| serde_json::from_slice(&entry.value).map_err(Into::into))
            .collect()
    }

    async fn list_all_records(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<DataJobRecord>, DataJobError> {
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::all_data_jobs_range()?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        Ok(entries
            .into_iter()
            .filter_map(|entry| serde_json::from_slice::<DataJobRecord>(&entry.value).ok())
            .collect())
    }

    async fn update_record(
        &self,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        update: impl FnOnce(&mut DataJobRecord) -> Result<(), DataJobError>,
    ) -> Result<DataJobRecord, DataJobError> {
        let key =
            ControlKeyspace::data_job_key(tenant.project_id(), tenant.environment_id(), job_id)?;
        let previous = self
            .adapter
            .get(&key)
            .await?
            .ok_or(DataJobError::NotFound)?;
        let mut record: DataJobRecord = serde_json::from_slice(&previous)?;
        if record.view.tenant != *tenant {
            return Err(DataJobError::NotFound);
        }
        let prior_indexes = index_keys(&record.view)?;
        update(&mut record)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        for index in prior_indexes {
            batch.delete(index);
        }
        for index in index_keys(&record.view)? {
            batch.put(index, job_id.as_bytes());
        }
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key,
                    value: previous,
                }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => {
                self.metrics.observe_state(record.view.state);
                Ok(record)
            }
            CompareAndWriteResult::Conflict { .. } => Err(DataJobError::Conflict),
        }
    }

    fn sign_artifact_claims(&self, claims: &ArtifactClaims) -> Result<String, DataJobError> {
        let payload = serde_json::to_vec(claims)?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let signature = blake3::keyed_hash(&self.artifact_signing_key, encoded.as_bytes());
        Ok(format!(
            "daj1_{encoded}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_bytes())
        ))
    }

    fn validate_artifact_grant(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &mako_api::TenantScope,
        job_id: &str,
        method: ArtifactMethod,
        token: &str,
        now_unix_seconds: u64,
    ) -> Result<ArtifactClaims, DataJobError> {
        use base64::Engine as _;
        let (payload, signature) = token
            .strip_prefix("daj1_")
            .and_then(|value| value.split_once('.'))
            .ok_or(DataJobError::InvalidArtifactGrant)?;
        let actual = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| DataJobError::InvalidArtifactGrant)?;
        let expected = blake3::keyed_hash(&self.artifact_signing_key, payload.as_bytes());
        if !constant_time_eq(&actual, expected.as_bytes()) {
            return Err(DataJobError::InvalidArtifactGrant);
        }
        let claims: ArtifactClaims = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| DataJobError::InvalidArtifactGrant)?,
        )?;
        if claims.tenant != *tenant
            || claims.job_id != job_id
            || claims.actor_id != actor.identity_id().as_str()
            || claims.method != method
            || now_unix_seconds >= claims.expires_at_unix_seconds
        {
            return Err(DataJobError::InvalidArtifactGrant);
        }
        Ok(claims)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactMethod {
    Upload,
    Download,
}

impl ArtifactMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "PUT",
            Self::Download => "GET",
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ArtifactClaims {
    tenant: mako_api::TenantScope,
    job_id: String,
    actor_id: String,
    method: ArtifactMethod,
    digest: Option<String>,
    expires_at_unix_seconds: u64,
}

const fn job_permission(kind: DataJobKind) -> ProjectDataPermission {
    match kind {
        DataJobKind::Import => ProjectDataPermission::Import,
        DataJobKind::Export => ProjectDataPermission::Export,
    }
}

const fn job_is_terminal(state: DataJobState) -> bool {
    matches!(
        state,
        DataJobState::Succeeded
            | DataJobState::Failed
            | DataJobState::Cancelled
            | DataJobState::Expired
    )
}

const fn data_job_state_index(state: DataJobState) -> usize {
    match state {
        DataJobState::AwaitingUpload => 0,
        DataJobState::DryRun => 1,
        DataJobState::AwaitingConfirmation => 2,
        DataJobState::Queued => 3,
        DataJobState::Running => 4,
        DataJobState::Cancelling => 5,
        DataJobState::Succeeded => 6,
        DataJobState::Failed => 7,
        DataJobState::Cancelled => 8,
        DataJobState::Expired => 9,
    }
}

fn empty_progress() -> DataJobProgress {
    DataJobProgress {
        processed: 0,
        committed: 0,
        failed: 0,
        skipped: 0,
        exported: 0,
        bytes: 0,
    }
}

fn validate_job_id(value: &str) -> Result<(), DataJobError> {
    if value.starts_with("djob_")
        && (13..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(DataJobError::NotFound)
    }
}

fn index_keys(view: &DataJobView) -> Result<Vec<Vec<u8>>, DataJobError> {
    let values = [
        ("collection", view.collection_id.as_str()),
        ("creator", view.creator_id.as_str()),
        (
            "kind",
            match view.kind {
                DataJobKind::Import => "import",
                DataJobKind::Export => "export",
            },
        ),
        (
            "state",
            match view.state {
                DataJobState::AwaitingUpload => "awaiting_upload",
                DataJobState::DryRun => "dry_run",
                DataJobState::AwaitingConfirmation => "awaiting_confirmation",
                DataJobState::Queued => "queued",
                DataJobState::Running => "running",
                DataJobState::Cancelling => "cancelling",
                DataJobState::Succeeded => "succeeded",
                DataJobState::Failed => "failed",
                DataJobState::Cancelled => "cancelled",
                DataJobState::Expired => "expired",
            },
        ),
    ];
    values
        .into_iter()
        .map(|(index, value)| {
            ControlKeyspace::data_job_index_key(
                view.tenant.project_id(),
                view.tenant.environment_id(),
                index,
                value,
                view.created_at_unix_seconds,
                &view.job_id,
            )
            .map_err(Into::into)
        })
        .collect()
}

fn validate_json_lines(bytes: &[u8]) -> Result<(u64, u64), DataJobError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DataJobError::InvalidArtifact)?;
    let mut rows = 0_u64;
    for line in text.lines() {
        if line.is_empty() || line.len() > EXPLORER_MAX_DOCUMENT_BYTES as usize {
            return Err(DataJobError::InvalidArtifact);
        }
        if !serde_json::from_str::<Value>(line)
            .ok()
            .is_some_and(|value| value.is_object())
        {
            return Err(DataJobError::InvalidArtifact);
        }
        rows = rows.checked_add(1).ok_or(DataJobError::InvalidArtifact)?;
    }
    if rows == 0 {
        return Err(DataJobError::InvalidArtifact);
    }
    Ok((rows, bytes.len() as u64))
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn worker_request_id(job_id: &str, offset: u64) -> String {
    format!(
        "req_{}",
        &blake3::hash(format!("request:{job_id}:{offset}").as_bytes()).to_hex()[..32]
    )
}

fn worker_idempotency_key(job_id: &str, offset: u64) -> String {
    format!(
        "idem_{}",
        &blake3::hash(format!("data-job:{job_id}:{offset}").as_bytes()).to_hex()[..32]
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DataJobWorkerReport {
    pub claimed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub expired: u64,
    pub deferred: u64,
}

const fn worker_error_is_retryable(error: &DataJobError) -> bool {
    matches!(
        error,
        DataJobError::Conflict
            | DataJobError::Storage(_)
            | DataJobError::Object(_)
            | DataJobError::Internal(_)
    )
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[derive(Debug)]
pub enum DataJobError {
    InvalidConfiguration,
    InvalidRequest,
    InvalidState,
    InvalidArtifact,
    InvalidArtifactGrant,
    NotFound,
    Forbidden,
    QuotaExceeded,
    Conflict,
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Collection(CollectionAdminError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Object(ObjectStoreError),
    Serialization(serde_json::Error),
    Internal(InternalClientError),
}

impl fmt::Display for DataJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest | Self::InvalidArtifact => "data-job input is invalid",
            Self::InvalidState => "data-job state does not permit this action",
            Self::InvalidArtifactGrant => "data-job artifact grant is invalid or expired",
            Self::NotFound | Self::Forbidden => "data job was not found",
            Self::QuotaExceeded => "data-job quota is exhausted",
            Self::Conflict => "data job changed concurrently",
            _ => "data-job service is unavailable",
        })
    }
}

impl Error for DataJobError {}

macro_rules! data_job_error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for DataJobError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

data_job_error_from!(ProjectStoreError, Project);
data_job_error_from!(OrganizationStoreError, Organization);
data_job_error_from!(CollectionAdminError, Collection);
data_job_error_from!(ControlKeyspaceError, Keyspace);
data_job_error_from!(StorageError, Storage);
data_job_error_from!(ObjectStoreError, Object);
data_job_error_from!(serde_json::Error, Serialization);
data_job_error_from!(InternalClientError, Internal);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lines_parser_is_object_only_bounded_and_exact() {
        let valid = br#"{"id":"one"}
{"id":"two","nested":{"ok":true}}
"#;
        assert_eq!(
            validate_json_lines(valid).expect("valid JSON Lines"),
            (2, valid.len() as u64)
        );
        for invalid in [
            b"".as_slice(),
            b"[]\n".as_slice(),
            b"{bad}\n".as_slice(),
            b"{}\n\n{}\n".as_slice(),
            b"\xff".as_slice(),
        ] {
            assert!(matches!(
                validate_json_lines(invalid),
                Err(DataJobError::InvalidArtifact)
            ));
        }
        let oversized = format!(
            "{{\"value\":\"{}\"}}",
            "a".repeat(EXPLORER_MAX_DOCUMENT_BYTES as usize)
        );
        assert!(matches!(
            validate_json_lines(oversized.as_bytes()),
            Err(DataJobError::InvalidArtifact)
        ));
    }

    #[test]
    fn worker_retry_and_idempotency_labels_are_closed_and_deterministic() {
        assert!(worker_error_is_retryable(&DataJobError::Conflict));
        assert!(!worker_error_is_retryable(&DataJobError::InvalidArtifact));
        assert_eq!(
            worker_request_id("djob_abcdefgh", 42),
            worker_request_id("djob_abcdefgh", 42)
        );
        assert_ne!(
            worker_idempotency_key("djob_abcdefgh", 42),
            worker_idempotency_key("djob_abcdefgh", 43)
        );
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"different"));
    }

    #[test]
    fn job_state_metrics_have_only_closed_state_dimensions() {
        let metrics = DataJobMetrics::default();
        metrics.observe_state(DataJobState::Queued);
        metrics.observe_state(DataJobState::Running);
        metrics.observe_state(DataJobState::Succeeded);
        metrics.observe_state(DataJobState::Succeeded);

        let snapshot = metrics.snapshot();
        assert_eq!(
            snapshot.transitions_by_state[data_job_state_index(DataJobState::Queued)],
            1
        );
        assert_eq!(
            snapshot.transitions_by_state[data_job_state_index(DataJobState::Running)],
            1
        );
        assert_eq!(
            snapshot.transitions_by_state[data_job_state_index(DataJobState::Succeeded)],
            2
        );
        assert_eq!(snapshot.transitions_by_state.iter().sum::<u64>(), 4);
    }
}
