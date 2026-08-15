//! Shared public and internal API contracts.

#![forbid(unsafe_code)]

mod error;
mod explorer;
mod explorer_contract;
mod scope;

pub use error::{
    API_ERROR_VERSION, ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice, SafeDetail, SafeDetails,
};
pub use explorer::{
    EXPLORER_CAPABILITY_AUDIENCE, EXPLORER_CAPABILITY_ISSUER, ExplorerAccessMode,
    ExplorerCapabilityClaims, ExplorerCapabilityError, ExplorerCapabilityKey,
    ExplorerCapabilityKeyRing, ExplorerCapabilityToken, ExplorerCapabilityValidation,
    ExplorerOperation,
};
pub use explorer_contract::*;
pub use scope::{CollectionId, CollectionScope, EnvironmentId, ProjectId, ScopeError, TenantScope};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "api";
