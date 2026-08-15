use std::{collections::BTreeSet, error::Error, fmt};

use mako_api::{CollectionId, CollectionScope, TenantScope};
use mako_documents::{
    DocumentMutationAuthorizer, DocumentReadAuthorizer, MutationAuthorizationContext,
    MutationAuthorizationDecision, MutationAuthorizationOperation, ReadAuthorizationContext,
};

use crate::DocumentOperation;

const BYPASS_ALLOWED: &str = "policy_privileged_bypass";
const BYPASS_SCOPE_MISMATCH: &str = "privileged_bypass_scope_mismatch";

macro_rules! validated_identifier {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, PrivilegedBypassError> {
                let value = value.into();
                validate_identifier($field, &value)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

validated_identifier!(ServiceCredentialId, "service credential id");
validated_identifier!(OperatorGrantId, "operator grant id");
validated_identifier!(OperatorActorId, "operator actor id");
validated_identifier!(AuditRequestId, "audit request id");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegedBypassReason(String);

impl PrivilegedBypassReason {
    pub fn parse(value: impl Into<String>) -> Result<Self, PrivilegedBypassError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 512
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(PrivilegedBypassError::InvalidField {
                field: "bypass reason",
                reason: "must contain 1-512 printable characters without surrounding whitespace",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialPrivileges {
    collections: CollectionPrivileges,
    operations: BTreeSet<DocumentOperation>,
}

impl CredentialPrivileges {
    pub fn new(
        collections: CollectionPrivileges,
        operations: impl IntoIterator<Item = DocumentOperation>,
    ) -> Result<Self, PrivilegedBypassError> {
        let operations = operations.into_iter().collect::<BTreeSet<_>>();
        if operations.is_empty() {
            return Err(PrivilegedBypassError::EmptyOperations);
        }
        Ok(Self {
            collections,
            operations,
        })
    }

    fn allows(&self, collection: &CollectionId, operation: DocumentOperation) -> bool {
        self.collections.allows(collection) && self.operations.contains(&operation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollectionPrivileges {
    All,
    Only(BTreeSet<CollectionId>),
}

impl CollectionPrivileges {
    #[must_use]
    pub const fn all() -> Self {
        Self::All
    }

    pub fn only(
        collections: impl IntoIterator<Item = CollectionId>,
    ) -> Result<Self, PrivilegedBypassError> {
        let collections = collections.into_iter().collect::<BTreeSet<_>>();
        if collections.is_empty() {
            return Err(PrivilegedBypassError::EmptyCollections);
        }
        Ok(Self::Only(collections))
    }

    fn allows(&self, collection: &CollectionId) -> bool {
        match self {
            Self::All => true,
            Self::Only(collections) => collections.contains(collection),
        }
    }
}

/// A credential identity produced only after secret-service credential
/// verification. Public project credentials and user sessions have no path to
/// this type or to `VerifiedPrivilegedPrincipal`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedServiceCredential {
    id: ServiceCredentialId,
    tenant: TenantScope,
    privileges: CredentialPrivileges,
}

impl VerifiedServiceCredential {
    #[must_use]
    pub fn from_verified_secret(
        id: ServiceCredentialId,
        tenant: TenantScope,
        privileges: CredentialPrivileges,
    ) -> Self {
        Self {
            id,
            tenant,
            privileges,
        }
    }
}

/// A separately authorized, least-privilege, time-bounded support grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedOperatorGrant {
    id: OperatorGrantId,
    actor: OperatorActorId,
    tenant: TenantScope,
    privileges: CredentialPrivileges,
    expires_at_unix_seconds: u64,
}

impl VerifiedOperatorGrant {
    #[must_use]
    pub fn from_verified_grant(
        id: OperatorGrantId,
        actor: OperatorActorId,
        tenant: TenantScope,
        privileges: CredentialPrivileges,
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self {
            id,
            actor,
            tenant,
            privileges,
            expires_at_unix_seconds,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifiedPrivilegedPrincipal {
    Service(VerifiedServiceCredential),
    Operator(VerifiedOperatorGrant),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegedBypassAuditContext {
    request_id: AuditRequestId,
    reason: PrivilegedBypassReason,
}

impl PrivilegedBypassAuditContext {
    #[must_use]
    pub const fn new(request_id: AuditRequestId, reason: PrivilegedBypassReason) -> Self {
        Self { request_id, reason }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivilegedPrincipalKind {
    ServiceCredential,
    PlatformOperator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegedBypassAuditEvent {
    principal_kind: PrivilegedPrincipalKind,
    principal_id: String,
    actor_id: String,
    scope: CollectionScope,
    operation: DocumentOperation,
    request_id: AuditRequestId,
    reason: PrivilegedBypassReason,
    outcome: &'static str,
}

impl PrivilegedBypassAuditEvent {
    #[must_use]
    pub const fn principal_kind(&self) -> PrivilegedPrincipalKind {
        self.principal_kind
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    #[must_use]
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    #[must_use]
    pub const fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    #[must_use]
    pub const fn operation(&self) -> DocumentOperation {
        self.operation
    }

    #[must_use]
    pub fn request_id(&self) -> &AuditRequestId {
        &self.request_id
    }

    #[must_use]
    pub fn reason(&self) -> &PrivilegedBypassReason {
        &self.reason
    }

    #[must_use]
    pub const fn outcome(&self) -> &'static str {
        self.outcome
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivilegedAuditWriteError {
    safe_message: String,
}

impl PrivilegedAuditWriteError {
    #[must_use]
    pub fn new(safe_message: impl Into<String>) -> Self {
        Self {
            safe_message: safe_message.into(),
        }
    }
}

impl fmt::Display for PrivilegedAuditWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.safe_message)
    }
}

impl Error for PrivilegedAuditWriteError {}

pub trait PrivilegedBypassAuditSink: Send + Sync {
    /// Must durably append the event. Returning an error prevents bypass from
    /// being established.
    fn record(&self, event: &PrivilegedBypassAuditEvent) -> Result<(), PrivilegedAuditWriteError>;
}

#[derive(Clone, Debug)]
pub struct PrivilegedBypassRequest {
    principal: VerifiedPrivilegedPrincipal,
    scope: CollectionScope,
    operation: DocumentOperation,
    audit: PrivilegedBypassAuditContext,
    verified_at_unix_seconds: u64,
}

impl PrivilegedBypassRequest {
    #[must_use]
    pub const fn new(
        principal: VerifiedPrivilegedPrincipal,
        scope: CollectionScope,
        operation: DocumentOperation,
        audit: PrivilegedBypassAuditContext,
        verified_at_unix_seconds: u64,
    ) -> Self {
        Self {
            principal,
            scope,
            operation,
            audit,
            verified_at_unix_seconds,
        }
    }
}

/// An authorizer that can exist only after scope checks and a successful audit
/// append. It is separate from the ordinary user policy authorizer.
#[derive(Clone, Debug)]
pub struct PrivilegedBypassAuthorizer {
    scope: CollectionScope,
    operation: DocumentOperation,
    audit_event: PrivilegedBypassAuditEvent,
}

impl PrivilegedBypassAuthorizer {
    pub fn establish(
        request: PrivilegedBypassRequest,
        audit_sink: &dyn PrivilegedBypassAuditSink,
    ) -> Result<Self, PrivilegedBypassError> {
        let (principal_kind, principal_id, actor_id, tenant, privileges) = match &request.principal
        {
            VerifiedPrivilegedPrincipal::Service(credential) => (
                PrivilegedPrincipalKind::ServiceCredential,
                credential.id.as_str(),
                credential.id.as_str(),
                &credential.tenant,
                &credential.privileges,
            ),
            VerifiedPrivilegedPrincipal::Operator(grant) => {
                if request.verified_at_unix_seconds >= grant.expires_at_unix_seconds {
                    return Err(PrivilegedBypassError::OperatorGrantExpired);
                }
                (
                    PrivilegedPrincipalKind::PlatformOperator,
                    grant.id.as_str(),
                    grant.actor.as_str(),
                    &grant.tenant,
                    &grant.privileges,
                )
            }
        };
        if tenant != request.scope.tenant()
            || !privileges.allows(request.scope.collection_id(), request.operation)
        {
            return Err(PrivilegedBypassError::OutsideCredentialScope);
        }
        let audit_event = PrivilegedBypassAuditEvent {
            principal_kind,
            principal_id: principal_id.to_owned(),
            actor_id: actor_id.to_owned(),
            scope: request.scope.clone(),
            operation: request.operation,
            request_id: request.audit.request_id,
            reason: request.audit.reason,
            outcome: "allowed",
        };
        audit_sink.record(&audit_event)?;
        Ok(Self {
            scope: request.scope,
            operation: request.operation,
            audit_event,
        })
    }

    #[must_use]
    pub const fn audit_event(&self) -> &PrivilegedBypassAuditEvent {
        &self.audit_event
    }

    #[must_use]
    pub fn authorizes(&self, scope: &CollectionScope, operation: DocumentOperation) -> bool {
        &self.scope == scope && self.operation == operation
    }

    fn decision(
        &self,
        scope: &CollectionScope,
        operation: DocumentOperation,
    ) -> MutationAuthorizationDecision {
        if self.authorizes(scope, operation) {
            MutationAuthorizationDecision::allow(BYPASS_ALLOWED)
        } else {
            MutationAuthorizationDecision::deny(BYPASS_SCOPE_MISMATCH)
        }
    }
}

impl DocumentMutationAuthorizer for PrivilegedBypassAuthorizer {
    fn authorize(
        &self,
        context: MutationAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision {
        let operation = match context.operation() {
            MutationAuthorizationOperation::Create => DocumentOperation::Create,
            MutationAuthorizationOperation::Update => DocumentOperation::Update,
            MutationAuthorizationOperation::Delete => DocumentOperation::Delete,
        };
        self.decision(context.scope(), operation)
    }
}

impl DocumentReadAuthorizer for PrivilegedBypassAuthorizer {
    fn authorize_read(
        &self,
        context: ReadAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision {
        self.decision(context.scope(), DocumentOperation::Read)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PrivilegedBypassError {
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    EmptyCollections,
    EmptyOperations,
    OutsideCredentialScope,
    OperatorGrantExpired,
    AuditWrite(PrivilegedAuditWriteError),
}

impl fmt::Display for PrivilegedBypassError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::EmptyCollections => formatter.write_str("credential collection scope is empty"),
            Self::EmptyOperations => formatter.write_str("credential operation scope is empty"),
            Self::OutsideCredentialScope => {
                formatter.write_str("requested bypass is outside credential scope")
            }
            Self::OperatorGrantExpired => formatter.write_str("operator grant is expired"),
            Self::AuditWrite(error) => write!(formatter, "privileged audit append failed: {error}"),
        }
    }
}

impl Error for PrivilegedBypassError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AuditWrite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PrivilegedAuditWriteError> for PrivilegedBypassError {
    fn from(error: PrivilegedAuditWriteError) -> Self {
        Self::AuditWrite(error)
    }
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), PrivilegedBypassError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PrivilegedBypassError::InvalidField {
            field,
            reason: "must contain 1-128 ASCII letters, digits, hyphens, or underscores",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[derive(Default)]
    struct CapturingAuditSink {
        events: Mutex<Vec<PrivilegedBypassAuditEvent>>,
        fail: bool,
    }

    impl PrivilegedBypassAuditSink for CapturingAuditSink {
        fn record(
            &self,
            event: &PrivilegedBypassAuditEvent,
        ) -> Result<(), PrivilegedAuditWriteError> {
            if self.fail {
                return Err(PrivilegedAuditWriteError::new("injected failure"));
            }
            self.events.lock().expect("audit lock").push(event.clone());
            Ok(())
        }
    }

    #[test]
    fn service_bypass_is_scoped_and_requires_a_successful_audit_append() {
        let target_tenant = tenant("env_abcdefgh");
        let todos = scope(target_tenant.clone(), "todos");
        let privileges = CredentialPrivileges::new(
            CollectionPrivileges::only([todos.collection_id().clone()]).expect("collections"),
            [DocumentOperation::Read],
        )
        .expect("privileges");
        let principal =
            VerifiedPrivilegedPrincipal::Service(VerifiedServiceCredential::from_verified_secret(
                ServiceCredentialId::parse("svc_backend").expect("id"),
                target_tenant,
                privileges,
            ));
        let bypass_request = request(
            principal.clone(),
            todos.clone(),
            DocumentOperation::Read,
            10,
        );
        let sink = CapturingAuditSink::default();
        let authorizer =
            PrivilegedBypassAuthorizer::establish(bypass_request, &sink).expect("bypass");

        assert!(authorizer.authorizes(&todos, DocumentOperation::Read));
        assert!(!authorizer.authorizes(&todos, DocumentOperation::Update));
        assert_eq!(sink.events.lock().expect("events").len(), 1);
        assert_eq!(authorizer.audit_event().principal_id(), "svc_backend");

        let outside = scope(tenant("env_ijklmnop"), "todos");
        assert!(matches!(
            PrivilegedBypassAuthorizer::establish(
                request(principal.clone(), outside, DocumentOperation::Read, 10),
                &sink,
            ),
            Err(PrivilegedBypassError::OutsideCredentialScope)
        ));
        assert_eq!(sink.events.lock().expect("events").len(), 1);

        let failing = CapturingAuditSink {
            events: Mutex::new(Vec::new()),
            fail: true,
        };
        assert!(matches!(
            PrivilegedBypassAuthorizer::establish(
                request(principal, todos, DocumentOperation::Read, 10),
                &failing,
            ),
            Err(PrivilegedBypassError::AuditWrite(_))
        ));
    }

    #[test]
    fn operator_bypass_requires_a_live_explicit_grant() {
        let tenant = tenant("env_abcdefgh");
        let scope = scope(tenant.clone(), "todos");
        let privileges =
            CredentialPrivileges::new(CollectionPrivileges::all(), [DocumentOperation::Delete])
                .expect("privileges");
        let principal =
            VerifiedPrivilegedPrincipal::Operator(VerifiedOperatorGrant::from_verified_grant(
                OperatorGrantId::parse("grant_support").expect("grant"),
                OperatorActorId::parse("operator_alice").expect("actor"),
                tenant,
                privileges,
                100,
            ));
        let sink = CapturingAuditSink::default();
        assert!(matches!(
            PrivilegedBypassAuthorizer::establish(
                request(principal, scope, DocumentOperation::Delete, 100),
                &sink,
            ),
            Err(PrivilegedBypassError::OperatorGrantExpired)
        ));
        assert!(sink.events.lock().expect("events").is_empty());
    }

    fn request(
        principal: VerifiedPrivilegedPrincipal,
        scope: CollectionScope,
        operation: DocumentOperation,
        now: u64,
    ) -> PrivilegedBypassRequest {
        PrivilegedBypassRequest::new(
            principal,
            scope,
            operation,
            PrivilegedBypassAuditContext::new(
                AuditRequestId::parse("req_abcdefgh").expect("request"),
                PrivilegedBypassReason::parse("scheduled maintenance").expect("reason"),
            ),
            now,
        )
    }

    fn tenant(environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse(environment).expect("environment"),
        )
    }

    fn scope(tenant: TenantScope, collection: &str) -> CollectionScope {
        CollectionScope::new(tenant, CollectionId::parse(collection).expect("collection"))
    }
}
