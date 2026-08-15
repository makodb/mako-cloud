//! Durable, idempotent project provisioning workflows.

#![forbid(unsafe_code)]

mod bootstrap;
mod workflow;

pub use bootstrap::{
    KvProvisioningBackend, ProvisionedEndpointSet, ProvisionedResourceState,
    ProvisioningBackendConfig,
};
pub use workflow::{
    OperatorRepair, OperatorRepairAction, Provisioner, ProvisioningBackend, ProvisioningComponent,
    ProvisioningDiagnostic, ProvisioningFailure, ProvisioningOperation, ProvisioningResource,
    ProvisioningState, ProvisioningStepStatus, ProvisioningStore, ProvisioningWorkflow,
    ProvisioningWorkflowError, ProvisioningWorkflowId,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "provisioning";
