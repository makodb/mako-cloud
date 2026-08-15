use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use mako_api::{ProjectId, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Deserializer, Serialize, de};

const WORKFLOW_DOMAIN: &[u8] = b"provisioning/workflows";

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
}
