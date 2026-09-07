use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use mako_api::{ProjectId, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KeyRange, KvAdapter,
    ScanDirection, ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Deserializer, Serialize, de};

const WORKFLOW_DOMAIN: &[u8] = b"provisioning/workflows";
/// The most workflows one worker pass reads before leaving the rest to the
/// next pass, so a pass stays bounded however long the store has lived.
pub const MAX_WORKFLOWS_PER_PASS: usize = 10_000;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProvisioningWorkflowId(String);

impl ProvisioningWorkflowId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ProvisioningWorkflowError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("wf_") else {
            return Err(ProvisioningWorkflowError::InvalidWorkflow);
        };
        if !(8..=96).contains(&suffix.len())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ProvisioningWorkflowError::InvalidWorkflow);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProvisioningWorkflowId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "scope")]
pub enum ProvisioningResource {
    Project(ProjectId),
    Environment(TenantScope),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningOperation {
    Create,
    Suspend,
    Restore,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningComponent {
    Storage,
    Identity,
    Policy,
    Replication,
    Functions,
    Quotas,
    Observability,
}

impl ProvisioningComponent {
    pub(crate) const ORDERED: [Self; 7] = [
        Self::Storage,
        Self::Identity,
        Self::Policy,
        Self::Replication,
        Self::Functions,
        Self::Quotas,
        Self::Observability,
    ];
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningState {
    Queued,
    Running,
    Compensating,
    Active,
    Suspended,
    Failed,
    RepairRequired,
    Deleting,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ProvisioningStepStatus {
    Pending,
    Running {
        attempt: u32,
    },
    Ready {
        attempt: u32,
    },
    Failed {
        attempt: u32,
        failure: ProvisioningFailure,
    },
    Compensating {
        attempt: u32,
    },
    Compensated {
        attempt: u32,
    },
    CompensationFailed {
        attempt: u32,
        failure: ProvisioningFailure,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProvisioningFailure {
    pub code: String,
    pub safe_diagnostic: String,
    pub retryable: bool,
}

impl ProvisioningFailure {
    pub fn new(
        code: impl Into<String>,
        safe_diagnostic: impl Into<String>,
        retryable: bool,
    ) -> Result<Self, ProvisioningWorkflowError> {
        let code = code.into();
        let safe_diagnostic = safe_diagnostic.into();
        if code.is_empty()
            || code.len() > 128
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || safe_diagnostic.is_empty()
            || safe_diagnostic.len() > 1_024
            || safe_diagnostic.chars().any(char::is_control)
        {
            return Err(ProvisioningWorkflowError::InvalidFailure);
        }
        Ok(Self {
            code,
            safe_diagnostic,
            retryable,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProvisioningDiagnostic {
    pub component: ProvisioningComponent,
    pub operation: ProvisioningOperation,
    pub compensation: bool,
    pub failure: ProvisioningFailure,
    pub at_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRepairAction {
    Requeue,
    RetryCompensation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperatorRepair {
    pub operator_id: String,
    pub reason: String,
    pub action: OperatorRepairAction,
    pub at_unix_seconds: u64,
    #[serde(default)]
    pub operation_key: Option<String>,
}

/// What a reconciliation pass should do about a project or environment whose
/// record is still `provisioning` after the time a healthy creation needs.
///
/// Two things leave a record there for good. A creation request that wrote
/// the record and then failed before it enqueued the workflow -- the data
/// plane was down for the second it took -- leaves nothing for the worker to
/// run at all. And a workflow that failed on a retryable step waits for a
/// retry nobody is obliged to send. Neither is the developer's fault, and
/// neither should need an operator: the pass repairs the first by enqueueing
/// the missing workflow under an id derived from the resource, so every pass
/// that finds the same gap names the same workflow, and the second by
/// retrying, with a pause between attempts and a ceiling on them so a data
/// plane that is really broken ends up in front of an operator rather than
/// in a loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrandedAction {
    /// No workflow describes the record: enqueue one.
    Enqueue {
        id: ProvisioningWorkflowId,
        resource: ProvisioningResource,
    },
    /// The record's workflow failed on a retryable step and has waited long enough.
    Retry(ProvisioningWorkflowId),
}

/// How long a record may sit in `provisioning` before it counts as stranded,
/// how long a failed workflow rests before a retry, and how many attempts a
/// workflow gets before it is left for an operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrandedPolicy {
    pub grace_seconds: u64,
    pub retry_after_seconds: u64,
    pub maximum_attempts: usize,
}

impl Default for StrandedPolicy {
    fn default() -> Self {
        Self {
            grace_seconds: 120,
            retry_after_seconds: 60,
            maximum_attempts: 12,
        }
    }
}

/// The workflow id a reconciliation pass gives a record it found without one.
/// Derived from the resource alone, so a gap found twice is one workflow.
///
/// `None` when the two identifiers together are longer than a workflow id
/// may be. No writer today makes ids that long, but the worker thread has no
/// panic recovery, so a record that cannot be named is skipped, not fatal.
#[must_use]
pub fn reconciliation_workflow_id(
    resource: &ProvisioningResource,
) -> Option<ProvisioningWorkflowId> {
    let suffix = match resource {
        ProvisioningResource::Project(project) => format!("reconcile-{}", project.as_str()),
        ProvisioningResource::Environment(scope) => format!(
            "reconcile-{}-{}",
            scope.project_id().as_str(),
            scope.environment_id().as_str()
        ),
    };
    ProvisioningWorkflowId::parse(format!("wf_{suffix}")).ok()
}

/// Decide the reconciliation actions for every record still `provisioning`:
/// `records` pairs each resource with when its record was created, and
/// `workflows` is every workflow the store holds. Records younger than the
/// grace period are left to the ordinary path.
#[must_use]
pub fn stranded_actions(
    records: &[(ProvisioningResource, u64)],
    workflows: &[ProvisioningWorkflow],
    now_unix_seconds: u64,
    policy: &StrandedPolicy,
) -> Vec<StrandedAction> {
    let mut actions = Vec::new();
    for (resource, created_at) in records {
        if now_unix_seconds < created_at.saturating_add(policy.grace_seconds) {
            continue;
        }
        // Creation and restoration both end in `active`; a newer workflow for
        // the resource speaks for it, whatever an older one did.
        let Some(workflow) = workflows
            .iter()
            .filter(|workflow| {
                workflow.resource() == resource
                    && matches!(
                        workflow.operation(),
                        ProvisioningOperation::Create | ProvisioningOperation::Restore
                    )
            })
            .max_by_key(|workflow| {
                (
                    workflow.updated_at_unix_seconds(),
                    workflow.created_at_unix_seconds(),
                )
            })
        else {
            if let Some(id) = reconciliation_workflow_id(resource) {
                actions.push(StrandedAction::Enqueue {
                    id,
                    resource: resource.clone(),
                });
            }
            continue;
        };
        if workflow.state() != ProvisioningState::Failed {
            // Queued and running work is the worker's; a finished one is the
            // record's own activation, which the worker reconciles; repair
            // required is an operator's.
            continue;
        }
        let attempts = workflow
            .diagnostics()
            .iter()
            .filter(|diagnostic| !diagnostic.compensation)
            .count();
        let retryable = workflow
            .diagnostics()
            .iter()
            .rev()
            .find(|diagnostic| !diagnostic.compensation)
            .is_some_and(|diagnostic| diagnostic.failure.retryable);
        if retryable
            && attempts < policy.maximum_attempts
            && now_unix_seconds
                >= workflow
                    .updated_at_unix_seconds()
                    .saturating_add(policy.retry_after_seconds)
        {
            actions.push(StrandedAction::Retry(workflow.id().clone()));
        }
    }
    actions
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProvisioningWorkflow {
    id: ProvisioningWorkflowId,
    resource: ProvisioningResource,
    operation: ProvisioningOperation,
    state: ProvisioningState,
    steps: BTreeMap<ProvisioningComponent, ProvisioningStepStatus>,
    diagnostics: Vec<ProvisioningDiagnostic>,
    operator_repairs: Vec<OperatorRepair>,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl ProvisioningWorkflow {
    #[must_use]
    pub fn id(&self) -> &ProvisioningWorkflowId {
        &self.id
    }

    #[must_use]
    pub const fn state(&self) -> ProvisioningState {
        self.state
    }

    #[must_use]
    pub fn resource(&self) -> &ProvisioningResource {
        &self.resource
    }

    #[must_use]
    pub const fn operation(&self) -> ProvisioningOperation {
        self.operation
    }

    #[must_use]
    pub fn steps(&self) -> &BTreeMap<ProvisioningComponent, ProvisioningStepStatus> {
        &self.steps
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[ProvisioningDiagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn operator_repairs(&self) -> &[OperatorRepair] {
        &self.operator_repairs
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }
}

#[async_trait]
pub trait ProvisioningBackend: Send + Sync {
    async fn apply_step(
        &self,
        resource: &ProvisioningResource,
        operation: ProvisioningOperation,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure>;

    async fn compensate_step(
        &self,
        resource: &ProvisioningResource,
        operation: ProvisioningOperation,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure>;
}

#[derive(Clone)]
pub struct ProvisioningStore {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for ProvisioningStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisioningStore")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl ProvisioningStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, ProvisioningWorkflowError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(ProvisioningWorkflowError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    pub async fn create(
        &self,
        workflow: &ProvisioningWorkflow,
    ) -> Result<(), ProvisioningWorkflowError> {
        let key = workflow_key(workflow.id())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(workflow)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await
    }

    pub async fn get(
        &self,
        id: &ProvisioningWorkflowId,
    ) -> Result<Option<ProvisioningWorkflow>, ProvisioningWorkflowError> {
        self.adapter
            .get(&workflow_key(id)?)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(ProvisioningWorkflowError::from))
            .transpose()
    }

    pub async fn list(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<ProvisioningWorkflow>, ProvisioningWorkflowError> {
        if limit.get() > 100 {
            return Err(ProvisioningWorkflowError::InvalidWorkflow);
        }
        self.adapter
            .scan(ScanRequest::new(
                TenantKeyspace::system_domain_range(WORKFLOW_DOMAIN)?,
                ScanDirection::Forward,
                limit,
            ))
            .await?
            .into_iter()
            .map(|record| serde_json::from_slice(&record.value).map_err(Into::into))
            .collect()
    }

    /// The workflows whose ids sort after `after` (every workflow when it is
    /// `None`), in id order, at most `limit` of them. Pages through the whole
    /// store where `list` shows only its first hundred rows.
    pub async fn list_after(
        &self,
        after: Option<&ProvisioningWorkflowId>,
        limit: NonZeroUsize,
    ) -> Result<Vec<ProvisioningWorkflow>, ProvisioningWorkflowError> {
        if limit.get() > 100 {
            return Err(ProvisioningWorkflowError::InvalidWorkflow);
        }
        let domain = TenantKeyspace::system_domain_range(WORKFLOW_DOMAIN)?;
        let range = match after {
            None => domain,
            Some(id) => {
                // The shortest key that sorts strictly after the last one seen.
                let mut start = workflow_key(id)?;
                start.push(0);
                if start >= domain.end_exclusive {
                    return Ok(Vec::new());
                }
                KeyRange::new(start, domain.end_exclusive)?
            }
        };
        self.adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?
            .into_iter()
            .map(|record| serde_json::from_slice(&record.value).map_err(Into::into))
            .collect()
    }

    async fn replace(
        &self,
        previous: &ProvisioningWorkflow,
        next: &ProvisioningWorkflow,
    ) -> Result<(), ProvisioningWorkflowError> {
        let key = workflow_key(previous.id())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), ProvisioningWorkflowError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(ProvisioningWorkflowError::Conflict),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Provisioner {
    store: ProvisioningStore,
}

impl Provisioner {
    #[must_use]
    pub fn new(store: ProvisioningStore) -> Self {
        Self { store }
    }

    pub async fn enqueue(
        &self,
        id: ProvisioningWorkflowId,
        resource: ProvisioningResource,
        operation: ProvisioningOperation,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        let workflow = ProvisioningWorkflow {
            id: id.clone(),
            resource,
            operation,
            state: ProvisioningState::Queued,
            steps: ProvisioningComponent::ORDERED
                .into_iter()
                .map(|component| (component, ProvisioningStepStatus::Pending))
                .collect(),
            diagnostics: Vec::new(),
            operator_repairs: Vec::new(),
            created_at_unix_seconds: now_unix_seconds,
            updated_at_unix_seconds: now_unix_seconds,
        };
        match self.store.create(&workflow).await {
            Ok(()) => Ok(workflow),
            Err(ProvisioningWorkflowError::Conflict) => {
                let existing = self.inspect(&id).await?;
                if existing.resource == workflow.resource
                    && existing.operation == workflow.operation
                {
                    Ok(existing)
                } else {
                    Err(ProvisioningWorkflowError::IdempotencyConflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn inspect(
        &self,
        id: &ProvisioningWorkflowId,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        self.store
            .get(id)
            .await?
            .ok_or(ProvisioningWorkflowError::NotFound)
    }

    pub async fn list(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<ProvisioningWorkflow>, ProvisioningWorkflowError> {
        self.store.list(limit).await
    }

    /// Every workflow in the store, in id order, read a page at a time.
    ///
    /// Workflows are never deleted, so a worker that only ever read the first
    /// page would stop seeing new ones once a hundred had accumulated -- and
    /// would never see a reconciliation workflow at all, since those ids sort
    /// after every hashed one. The pass is still bounded: past
    /// [`MAX_WORKFLOWS_PER_PASS`] the rest wait for the next one.
    pub async fn list_all(&self) -> Result<Vec<ProvisioningWorkflow>, ProvisioningWorkflowError> {
        let page = NonZeroUsize::new(100).expect("page is positive");
        let mut workflows = Vec::new();
        let mut after: Option<ProvisioningWorkflowId> = None;
        loop {
            let batch = self.store.list_after(after.as_ref(), page).await?;
            let last_page = batch.len() < page.get();
            after = batch.last().map(|workflow| workflow.id.clone());
            workflows.extend(batch);
            if last_page || workflows.len() >= MAX_WORKFLOWS_PER_PASS {
                return Ok(workflows);
            }
        }
    }

    pub async fn run(
        &self,
        id: &ProvisioningWorkflowId,
        backend: &dyn ProvisioningBackend,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        let mut workflow = self.inspect(id).await?;
        if workflow.state == ProvisioningState::RepairRequired {
            return Err(ProvisioningWorkflowError::RepairRequired);
        }
        if !matches!(
            workflow.state,
            ProvisioningState::Queued | ProvisioningState::Running
        ) {
            return Ok(workflow);
        }
        self.set_state(&mut workflow, ProvisioningState::Running, now_unix_seconds)
            .await?;
        for component in ProvisioningComponent::ORDERED {
            let status = workflow
                .steps
                .get(&component)
                .cloned()
                .ok_or(ProvisioningWorkflowError::CorruptWorkflow)?;
            if matches!(status, ProvisioningStepStatus::Ready { .. }) {
                continue;
            }
            let attempt = next_attempt(&status);
            self.set_step(
                &mut workflow,
                component,
                ProvisioningStepStatus::Running { attempt },
                now_unix_seconds,
            )
            .await?;
            match backend
                .apply_step(&workflow.resource, workflow.operation, component)
                .await
            {
                Ok(()) => {
                    self.set_step(
                        &mut workflow,
                        component,
                        ProvisioningStepStatus::Ready { attempt },
                        now_unix_seconds,
                    )
                    .await?;
                }
                Err(failure) => {
                    self.record_failure(
                        &mut workflow,
                        component,
                        ProvisioningStepStatus::Failed { attempt, failure },
                        false,
                        now_unix_seconds,
                    )
                    .await?;
                    return self.compensate(workflow, backend, now_unix_seconds).await;
                }
            }
        }
        let complete = match workflow.operation {
            ProvisioningOperation::Create | ProvisioningOperation::Restore => {
                ProvisioningState::Active
            }
            ProvisioningOperation::Suspend => ProvisioningState::Suspended,
            ProvisioningOperation::Delete => ProvisioningState::Deleted,
        };
        self.set_state(&mut workflow, complete, now_unix_seconds)
            .await?;
        Ok(workflow)
    }

    pub async fn retry(
        &self,
        id: &ProvisioningWorkflowId,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        let mut workflow = self.inspect(id).await?;
        if workflow.state != ProvisioningState::Failed
            || workflow
                .diagnostics
                .iter()
                .rev()
                .find(|diagnostic| !diagnostic.compensation)
                .is_none_or(|diagnostic| !diagnostic.failure.retryable)
        {
            return Err(ProvisioningWorkflowError::NotRetryable);
        }
        self.reset_for_retry(&mut workflow, now_unix_seconds)
            .await?;
        Ok(workflow)
    }

    pub async fn operator_repair(
        &self,
        id: &ProvisioningWorkflowId,
        operator_id: impl Into<String>,
        reason: impl Into<String>,
        action: OperatorRepairAction,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        self.operator_repair_guarded(id, operator_id, reason, action, None, now_unix_seconds)
            .await
    }

    pub async fn operator_repair_guarded(
        &self,
        id: &ProvisioningWorkflowId,
        operator_id: impl Into<String>,
        reason: impl Into<String>,
        action: OperatorRepairAction,
        operation_key: Option<String>,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        let mut workflow = self.inspect(id).await?;
        let operator_id = operator_id.into();
        let reason = reason.into();
        if let Some(operation_key) = operation_key.as_deref() {
            validate_repair_text("operation key", operation_key, 16, 128)?;
            if let Some(previous) = workflow
                .operator_repairs
                .iter()
                .find(|repair| repair.operation_key.as_deref() == Some(operation_key))
            {
                return if previous.operator_id == operator_id
                    && previous.reason == reason
                    && previous.action == action
                {
                    Ok(workflow)
                } else {
                    Err(ProvisioningWorkflowError::Conflict)
                };
            }
        }
        if !matches!(
            workflow.state,
            ProvisioningState::Failed | ProvisioningState::RepairRequired
        ) {
            return Err(ProvisioningWorkflowError::RepairNotApplicable);
        }
        validate_repair_text("operator id", &operator_id, 3, 256)?;
        validate_repair_text("repair reason", &reason, 8, 1_024)?;
        let previous = workflow.clone();
        workflow.operator_repairs.push(OperatorRepair {
            operator_id,
            reason,
            action,
            at_unix_seconds: now_unix_seconds,
            operation_key,
        });
        match action {
            OperatorRepairAction::Requeue => {
                for status in workflow.steps.values_mut() {
                    if !matches!(status, ProvisioningStepStatus::Ready { .. }) {
                        *status = ProvisioningStepStatus::Pending;
                    }
                }
                workflow.state = ProvisioningState::Queued;
            }
            OperatorRepairAction::RetryCompensation => {
                for status in workflow.steps.values_mut() {
                    if matches!(status, ProvisioningStepStatus::CompensationFailed { .. }) {
                        *status = ProvisioningStepStatus::Ready { attempt: 1 };
                    }
                }
                workflow.state = ProvisioningState::Compensating;
            }
        }
        workflow.updated_at_unix_seconds = now_unix_seconds;
        self.store.replace(&previous, &workflow).await?;
        Ok(workflow)
    }

    pub async fn retry_compensation(
        &self,
        id: &ProvisioningWorkflowId,
        backend: &dyn ProvisioningBackend,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        let workflow = self.inspect(id).await?;
        if workflow.state != ProvisioningState::Compensating {
            return Err(ProvisioningWorkflowError::RepairNotApplicable);
        }
        self.compensate(workflow, backend, now_unix_seconds).await
    }

    async fn compensate(
        &self,
        mut workflow: ProvisioningWorkflow,
        backend: &dyn ProvisioningBackend,
        now_unix_seconds: u64,
    ) -> Result<ProvisioningWorkflow, ProvisioningWorkflowError> {
        self.set_state(
            &mut workflow,
            ProvisioningState::Compensating,
            now_unix_seconds,
        )
        .await?;
        for component in ProvisioningComponent::ORDERED.into_iter().rev() {
            let status = workflow
                .steps
                .get(&component)
                .cloned()
                .ok_or(ProvisioningWorkflowError::CorruptWorkflow)?;
            let attempt = match status {
                ProvisioningStepStatus::Ready { attempt }
                | ProvisioningStepStatus::CompensationFailed { attempt, .. } => attempt,
                _ => continue,
            };
            self.set_step(
                &mut workflow,
                component,
                ProvisioningStepStatus::Compensating { attempt },
                now_unix_seconds,
            )
            .await?;
            match backend
                .compensate_step(&workflow.resource, workflow.operation, component)
                .await
            {
                Ok(()) => {
                    self.set_step(
                        &mut workflow,
                        component,
                        ProvisioningStepStatus::Compensated { attempt },
                        now_unix_seconds,
                    )
                    .await?;
                }
                Err(failure) => {
                    self.record_failure(
                        &mut workflow,
                        component,
                        ProvisioningStepStatus::CompensationFailed { attempt, failure },
                        true,
                        now_unix_seconds,
                    )
                    .await?;
                    self.set_state(
                        &mut workflow,
                        ProvisioningState::RepairRequired,
                        now_unix_seconds,
                    )
                    .await?;
                    return Ok(workflow);
                }
            }
        }
        self.set_state(&mut workflow, ProvisioningState::Failed, now_unix_seconds)
            .await?;
        Ok(workflow)
    }

    async fn reset_for_retry(
        &self,
        workflow: &mut ProvisioningWorkflow,
        now_unix_seconds: u64,
    ) -> Result<(), ProvisioningWorkflowError> {
        let previous = workflow.clone();
        for status in workflow.steps.values_mut() {
            if !matches!(status, ProvisioningStepStatus::Ready { .. }) {
                *status = ProvisioningStepStatus::Pending;
            }
        }
        workflow.state = ProvisioningState::Queued;
        workflow.updated_at_unix_seconds = now_unix_seconds;
        self.store.replace(&previous, workflow).await
    }

    async fn set_state(
        &self,
        workflow: &mut ProvisioningWorkflow,
        state: ProvisioningState,
        now_unix_seconds: u64,
    ) -> Result<(), ProvisioningWorkflowError> {
        let previous = workflow.clone();
        workflow.state = state;
        workflow.updated_at_unix_seconds = now_unix_seconds;
        self.store.replace(&previous, workflow).await
    }

    async fn set_step(
        &self,
        workflow: &mut ProvisioningWorkflow,
        component: ProvisioningComponent,
        status: ProvisioningStepStatus,
        now_unix_seconds: u64,
    ) -> Result<(), ProvisioningWorkflowError> {
        let previous = workflow.clone();
        workflow.steps.insert(component, status);
        workflow.updated_at_unix_seconds = now_unix_seconds;
        self.store.replace(&previous, workflow).await
    }

    async fn record_failure(
        &self,
        workflow: &mut ProvisioningWorkflow,
        component: ProvisioningComponent,
        status: ProvisioningStepStatus,
        compensation: bool,
        now_unix_seconds: u64,
    ) -> Result<(), ProvisioningWorkflowError> {
        let failure = match &status {
            ProvisioningStepStatus::Failed { failure, .. }
            | ProvisioningStepStatus::CompensationFailed { failure, .. } => failure.clone(),
            _ => return Err(ProvisioningWorkflowError::CorruptWorkflow),
        };
        let previous = workflow.clone();
        workflow.diagnostics.push(ProvisioningDiagnostic {
            component,
            operation: workflow.operation,
            compensation,
            failure,
            at_unix_seconds: now_unix_seconds,
        });
        workflow.steps.insert(component, status);
        workflow.updated_at_unix_seconds = now_unix_seconds;
        self.store.replace(&previous, workflow).await
    }
}

#[derive(Debug)]
pub enum ProvisioningWorkflowError {
    InvalidWorkflow,
    InvalidFailure,
    InvalidRepair,
    UnsupportedDurability,
    NotFound,
    NotRetryable,
    RepairRequired,
    RepairNotApplicable,
    CorruptWorkflow,
    Conflict,
    IdempotencyConflict,
    KeyEncoding(mako_storage::KeyCodecError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ProvisioningWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWorkflow => "provisioning workflow is invalid",
            Self::InvalidFailure => "provisioning failure is invalid",
            Self::InvalidRepair => "operator repair is invalid",
            Self::UnsupportedDurability => "provisioning durability is unsupported",
            Self::NotFound => "provisioning workflow was not found",
            Self::NotRetryable => "provisioning workflow is not retryable",
            Self::RepairRequired => "provisioning workflow requires operator repair",
            Self::RepairNotApplicable => "operator repair is not applicable",
            Self::CorruptWorkflow => "provisioning workflow is corrupt",
            Self::Conflict => "provisioning workflow changed concurrently",
            Self::IdempotencyConflict => "workflow id is bound to another request",
            Self::KeyEncoding(_) => "provisioning key encoding failed",
            Self::Storage(_) => "provisioning storage operation failed",
            Self::Serialization(_) => "provisioning serialization failed",
        })
    }
}

impl Error for ProvisioningWorkflowError {}

impl From<mako_storage::KeyCodecError> for ProvisioningWorkflowError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyEncoding(error)
    }
}

impl From<StorageError> for ProvisioningWorkflowError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for ProvisioningWorkflowError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

fn workflow_key(id: &ProvisioningWorkflowId) -> Result<Vec<u8>, ProvisioningWorkflowError> {
    Ok(TenantKeyspace::system_key(WORKFLOW_DOMAIN, id.as_str())?)
}

fn next_attempt(status: &ProvisioningStepStatus) -> u32 {
    match status {
        ProvisioningStepStatus::Running { attempt }
        | ProvisioningStepStatus::Ready { attempt }
        | ProvisioningStepStatus::Failed { attempt, .. }
        | ProvisioningStepStatus::Compensating { attempt }
        | ProvisioningStepStatus::Compensated { attempt }
        | ProvisioningStepStatus::CompensationFailed { attempt, .. } => attempt.saturating_add(1),
        ProvisioningStepStatus::Pending => 1,
    }
}

fn validate_repair_text(
    _field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ProvisioningWorkflowError> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ProvisioningWorkflowError::InvalidRepair);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_storage::MemoryAdapter;

    use super::*;

    struct FakeBackend {
        apply_failure: Mutex<Option<(ProvisioningComponent, ProvisioningFailure)>>,
        compensation_failure: Mutex<Option<(ProvisioningComponent, ProvisioningFailure)>>,
        calls: Mutex<Vec<(bool, ProvisioningComponent)>>,
    }

    #[async_trait]
    impl ProvisioningBackend for FakeBackend {
        async fn apply_step(
            &self,
            _resource: &ProvisioningResource,
            _operation: ProvisioningOperation,
            component: ProvisioningComponent,
        ) -> Result<(), ProvisioningFailure> {
            self.calls.lock().expect("calls").push((false, component));
            let mut failure = self.apply_failure.lock().expect("failure");
            if failure
                .as_ref()
                .is_some_and(|(target, _)| *target == component)
            {
                return Err(failure.take().expect("present failure").1);
            }
            Ok(())
        }

        async fn compensate_step(
            &self,
            _resource: &ProvisioningResource,
            _operation: ProvisioningOperation,
            component: ProvisioningComponent,
        ) -> Result<(), ProvisioningFailure> {
            self.calls.lock().expect("calls").push((true, component));
            let mut failure = self.compensation_failure.lock().expect("failure");
            if failure
                .as_ref()
                .is_some_and(|(target, _)| *target == component)
            {
                return Err(failure.take().expect("present failure").1);
            }
            Ok(())
        }
    }

    fn provisioner() -> Provisioner {
        let store = ProvisioningStore::new(Arc::new(MemoryAdapter::new()), Durability::Memory)
            .expect("store");
        Provisioner::new(store)
    }

    fn failure(code: &str, retryable: bool) -> ProvisioningFailure {
        ProvisioningFailure::new(code, "safe operator diagnostic", retryable).expect("failure")
    }

    #[test]
    fn retryable_failure_is_compensated_and_can_resume_idempotently() {
        futures::executor::block_on(async {
            let provisioner = provisioner();
            let id = ProvisioningWorkflowId::parse("wf_example000").expect("workflow id");
            let resource =
                ProvisioningResource::Project(ProjectId::parse("prj_example00").expect("project"));
            let first = provisioner
                .enqueue(
                    id.clone(),
                    resource.clone(),
                    ProvisioningOperation::Create,
                    10,
                )
                .await
                .expect("enqueue");
            let duplicate = provisioner
                .enqueue(id.clone(), resource, ProvisioningOperation::Create, 10)
                .await
                .expect("idempotent enqueue");
            assert_eq!(first, duplicate);
            let backend = FakeBackend {
                apply_failure: Mutex::new(Some((
                    ProvisioningComponent::Policy,
                    failure("policy_unavailable", true),
                ))),
                compensation_failure: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            };
            let failed = provisioner.run(&id, &backend, 11).await.expect("run");
            assert_eq!(failed.state(), ProvisioningState::Failed);
            assert_eq!(failed.diagnostics().len(), 1);
            assert!(matches!(
                failed.steps().get(&ProvisioningComponent::Storage),
                Some(ProvisioningStepStatus::Compensated { .. })
            ));
            provisioner.retry(&id, 12).await.expect("retry");
            let active = provisioner.run(&id, &backend, 13).await.expect("rerun");
            assert_eq!(active.state(), ProvisioningState::Active);
        });
    }

    #[test]
    fn compensation_failure_requires_a_reasoned_operator_repair() {
        futures::executor::block_on(async {
            let provisioner = provisioner();
            let id = ProvisioningWorkflowId::parse("wf_repair0000").expect("workflow id");
            provisioner
                .enqueue(
                    id.clone(),
                    ProvisioningResource::Project(
                        ProjectId::parse("prj_example00").expect("project"),
                    ),
                    ProvisioningOperation::Create,
                    10,
                )
                .await
                .expect("enqueue");
            let backend = FakeBackend {
                apply_failure: Mutex::new(Some((
                    ProvisioningComponent::Policy,
                    failure("policy_invalid", false),
                ))),
                compensation_failure: Mutex::new(Some((
                    ProvisioningComponent::Identity,
                    failure("cleanup_unavailable", true),
                ))),
                calls: Mutex::new(Vec::new()),
            };
            let repair_required = provisioner.run(&id, &backend, 11).await.expect("run");
            assert_eq!(repair_required.state(), ProvisioningState::RepairRequired);
            let repair = provisioner
                .operator_repair(
                    &id,
                    "operator-123",
                    "retry cleanup after identity recovery",
                    OperatorRepairAction::RetryCompensation,
                    12,
                )
                .await
                .expect("repair");
            assert_eq!(repair.operator_repairs().len(), 1);
            let failed = provisioner
                .retry_compensation(&id, &backend, 13)
                .await
                .expect("compensation retry");
            assert_eq!(failed.state(), ProvisioningState::Failed);
            assert!(matches!(
                provisioner.retry(&id, 14).await,
                Err(ProvisioningWorkflowError::NotRetryable)
            ));
        });
    }

    #[test]
    fn guarded_operator_repair_is_idempotent_and_rejects_conflicting_keys() {
        futures::executor::block_on(async {
            let provisioner = provisioner();
            let id = ProvisioningWorkflowId::parse("wf_guarded000").expect("workflow id");
            provisioner
                .enqueue(
                    id.clone(),
                    ProvisioningResource::Project(
                        ProjectId::parse("prj_example00").expect("project"),
                    ),
                    ProvisioningOperation::Create,
                    10,
                )
                .await
                .expect("enqueue");
            let backend = FakeBackend {
                apply_failure: Mutex::new(Some((
                    ProvisioningComponent::Policy,
                    failure("policy_invalid", false),
                ))),
                compensation_failure: Mutex::new(Some((
                    ProvisioningComponent::Identity,
                    failure("cleanup_unavailable", true),
                ))),
                calls: Mutex::new(Vec::new()),
            };
            let failed = provisioner.run(&id, &backend, 11).await.expect("run");
            assert_eq!(failed.state(), ProvisioningState::RepairRequired);
            let operation_key = Some("repair-operation-0001".to_owned());
            let repaired = provisioner
                .operator_repair_guarded(
                    &id,
                    "operator-123",
                    "retry cleanup after identity recovery",
                    OperatorRepairAction::RetryCompensation,
                    operation_key.clone(),
                    12,
                )
                .await
                .expect("repair");
            let replay = provisioner
                .operator_repair_guarded(
                    &id,
                    "operator-123",
                    "retry cleanup after identity recovery",
                    OperatorRepairAction::RetryCompensation,
                    operation_key.clone(),
                    13,
                )
                .await
                .expect("idempotent replay");
            assert_eq!(repaired, replay);
            assert_eq!(replay.operator_repairs().len(), 1);
            assert!(matches!(
                provisioner
                    .operator_repair_guarded(
                        &id,
                        "operator-123",
                        "different reviewed repair reason",
                        OperatorRepairAction::RetryCompensation,
                        operation_key,
                        14,
                    )
                    .await,
                Err(ProvisioningWorkflowError::Conflict)
            ));
        });
    }
    /// A record left in `provisioning` with no workflow -- the creation request
    /// failed between writing the record and enqueueing -- gets a workflow
    /// under an id derived from the resource, so every pass names the same
    /// one; a failed retryable workflow is retried after a rest and only so
    /// many times; queued, running, active, and repair-required workflows, and
    /// records still inside the grace period, are left alone (finding #49).
    #[test]
    fn stranded_records_are_enqueued_and_retryable_failures_retried_with_a_ceiling() {
        futures::executor::block_on(async {
            let provisioner = provisioner();
            let project =
                ProvisioningResource::Project(ProjectId::parse("prj_stranded00").expect("project"));
            let policy = StrandedPolicy {
                grace_seconds: 120,
                retry_after_seconds: 60,
                maximum_attempts: 2,
            };
            let no_workflows: Vec<ProvisioningWorkflow> = Vec::new();

            // Inside the grace period nothing happens; after it the missing
            // workflow is enqueued under the same id every time.
            assert!(
                stranded_actions(&[(project.clone(), 1_000)], &no_workflows, 1_100, &policy)
                    .is_empty()
            );
            let first =
                stranded_actions(&[(project.clone(), 1_000)], &no_workflows, 1_200, &policy);
            let second =
                stranded_actions(&[(project.clone(), 1_000)], &no_workflows, 1_500, &policy);
            assert_eq!(first, second);
            let StrandedAction::Enqueue { id, resource } = &first[0] else {
                panic!("expected an enqueue, got {first:?}");
            };
            assert_eq!(resource, &project);
            assert_eq!(Some(id), reconciliation_workflow_id(&project).as_ref());
            assert_eq!(id.as_str(), "wf_reconcile-prj_stranded00");

            // A queued workflow for the record is the worker's business.
            let queued = provisioner
                .enqueue(
                    id.clone(),
                    project.clone(),
                    ProvisioningOperation::Create,
                    1_200,
                )
                .await
                .expect("enqueue");
            assert!(
                stranded_actions(&[(project.clone(), 1_000)], &[queued], 1_500, &policy).is_empty()
            );

            // A retryable failure rests, then is retried -- but not forever.
            let backend = FakeBackend {
                apply_failure: Mutex::new(Some((
                    ProvisioningComponent::Storage,
                    failure("data_plane_unavailable", true),
                ))),
                compensation_failure: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            };
            let failed = provisioner.run(id, &backend, 1_210).await.expect("run");
            assert_eq!(failed.state(), ProvisioningState::Failed);
            assert!(
                stranded_actions(
                    &[(project.clone(), 1_000)],
                    std::slice::from_ref(&failed),
                    1_250,
                    &policy
                )
                .is_empty()
            );
            assert_eq!(
                stranded_actions(
                    &[(project.clone(), 1_000)],
                    std::slice::from_ref(&failed),
                    1_270,
                    &policy
                ),
                vec![StrandedAction::Retry(id.clone())]
            );
            // A second retryable failure reaches the ceiling of two attempts.
            provisioner.retry(id, 1_270).await.expect("retry");
            *backend.apply_failure.lock().expect("failure") = Some((
                ProvisioningComponent::Storage,
                failure("data_plane_unavailable", true),
            ));
            let failed_twice = provisioner.run(id, &backend, 1_280).await.expect("rerun");
            assert_eq!(failed_twice.state(), ProvisioningState::Failed);
            assert!(
                stranded_actions(&[(project.clone(), 1_000)], &[failed_twice], 9_999, &policy)
                    .is_empty()
            );

            // A failure that is not retryable, and a workflow that needs an
            // operator, are never retried by the pass.
            let fixed_id = ProvisioningWorkflowId::parse("wf_notretry00").expect("id");
            let other =
                ProvisioningResource::Project(ProjectId::parse("prj_stranded01").expect("project"));
            provisioner
                .enqueue(
                    fixed_id.clone(),
                    other.clone(),
                    ProvisioningOperation::Create,
                    1_200,
                )
                .await
                .expect("enqueue");
            let backend = FakeBackend {
                apply_failure: Mutex::new(Some((
                    ProvisioningComponent::Policy,
                    failure("policy_invalid", false),
                ))),
                compensation_failure: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            };
            let not_retryable = provisioner
                .run(&fixed_id, &backend, 1_210)
                .await
                .expect("run");
            assert_eq!(not_retryable.state(), ProvisioningState::Failed);
            assert!(
                stranded_actions(&[(other, 1_000)], &[not_retryable], 9_999, &policy).is_empty()
            );
        });
    }
    /// The worker reads every workflow, not the first hundred: a
    /// reconciliation id sorts after every hashed id, and workflows are never
    /// deleted, so the first page alone would hide it forever (finding #49).
    #[test]
    fn listing_pages_through_every_workflow_including_reconciliation_ids() {
        futures::executor::block_on(async {
            let provisioner = provisioner();
            for index in 0..120 {
                let id = ProvisioningWorkflowId::parse(format!("wf_{index:032x}")).expect("id");
                let project = ProjectId::parse(format!("prj_{index:08}")).expect("project");
                provisioner
                    .enqueue(
                        id,
                        ProvisioningResource::Project(project),
                        ProvisioningOperation::Create,
                        1,
                    )
                    .await
                    .expect("enqueue");
            }
            let stranded =
                ProvisioningResource::Project(ProjectId::parse("prj_stranded00").expect("project"));
            let reconcile = reconciliation_workflow_id(&stranded).expect("reconciliation id");
            provisioner
                .enqueue(
                    reconcile.clone(),
                    stranded,
                    ProvisioningOperation::Create,
                    2,
                )
                .await
                .expect("enqueue");

            let first_page = provisioner
                .list(NonZeroUsize::new(100).expect("limit"))
                .await
                .expect("first page");
            assert_eq!(first_page.len(), 100);
            assert!(
                first_page
                    .iter()
                    .all(|workflow| workflow.id() != &reconcile)
            );

            let all = provisioner.list_all().await.expect("every workflow");
            assert_eq!(all.len(), 121);
            assert_eq!(all.last().map(ProvisioningWorkflow::id), Some(&reconcile));
            let mut ids: Vec<_> = all.iter().map(|workflow| workflow.id().as_str()).collect();
            let sorted = ids.clone();
            ids.sort_unstable();
            assert_eq!(ids, sorted, "pages arrive in id order without repeats");
        });
    }
}
