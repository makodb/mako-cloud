//! Deterministic document-policy compilation and evaluation.

#![forbid(unsafe_code)]

mod authorization_epoch;
mod compiler;
mod context;
mod document_hook;
mod evaluator;
mod explorer_authorization;
mod model;
mod privileged;
mod store;
mod visibility;

pub use authorization_epoch::{
    AuthorizationEpoch, AuthorizationEpochError, AuthorizationEpochSnapshot,
    AuthorizationEpochStore, AuthorizationInvalidation, AuthorizationInvalidationKind,
    InvalidationSequence,
};
pub use compiler::{
    CompiledPolicySet, PolicyCompilation, PolicyCompileError, PolicyCompiler, PolicyCompilerLimits,
};

pub use context::{
    PolicyContextError, PolicyEvaluationContext, SafeRequestMetadata, SubjectId, VerifiedIdentity,
    VerifiedRole,
};
pub use document_hook::{DocumentPolicyAuthorizer, DocumentPolicyReadAuthorizer};
pub use evaluator::{
    PolicyDecision, PolicyDecisionCode, PolicyEvaluator, PolicyEvaluatorState, PolicyOutcome,
};
pub use explorer_authorization::{
    ExplorerAuthorizationError, ExplorerAuthorizationStore, ExplorerGrantAuthorityRecord,
};

pub use model::{
    DiagnosticSeverity, DocumentOperation, PolicyDiagnostic, PolicyEffect, PolicyModelError,
    PolicyRule, PolicyRuleId, PolicySet, PolicyState, PolicyVersion, SourceSpan,
};
pub use privileged::{
    AuditRequestId, CollectionPrivileges, CredentialPrivileges, OperatorActorId, OperatorGrantId,
    PrivilegedAuditWriteError, PrivilegedBypassAuditContext, PrivilegedBypassAuditEvent,
    PrivilegedBypassAuditSink, PrivilegedBypassAuthorizer, PrivilegedBypassError,
    PrivilegedBypassReason, PrivilegedBypassRequest, PrivilegedPrincipalKind, ServiceCredentialId,
    VerifiedOperatorGrant, VerifiedPrivilegedPrincipal, VerifiedServiceCredential,
};
pub use store::{PolicyStore, PolicyStoreError, PolicyValidation};
pub use visibility::{SyntheticTombstone, VisibilityTransition};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "policy";
