//! Durable, idempotent project provisioning workflows.

#![forbid(unsafe_code)]

mod bootstrap;
mod workflow;

pub use bootstrap::{
    KvProvisioningBackend, ProvisionedEndpointSet, ProvisionedResourceState,
    ProvisioningBackendConfig,
};
pub use workflow::{
    MAX_WORKFLOWS_PER_PASS, OperatorRepair, OperatorRepairAction, Provisioner, ProvisioningBackend,
    ProvisioningComponent, ProvisioningDiagnostic, ProvisioningFailure, ProvisioningOperation,
    ProvisioningResource, ProvisioningState, ProvisioningStepStatus, ProvisioningStore,
    ProvisioningWorkflow, ProvisioningWorkflowError, ProvisioningWorkflowId, StrandedAction,
    StrandedPolicy, reconciliation_workflow_id, stranded_actions,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "provisioning";
