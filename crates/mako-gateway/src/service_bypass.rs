use std::{error::Error, fmt};

use mako_api::{CollectionId, CollectionScope, TenantScope};
use mako_identity::{
    IdentityStore, IdentityStoreError, ServiceCredentialOperation, VerifiedProjectCredential,
};
use mako_policy::{
    AuditRequestId, CollectionPrivileges, CredentialPrivileges, DocumentOperation,
    PrivilegedBypassAuditContext, PrivilegedBypassAuditSink, PrivilegedBypassAuthorizer,
    PrivilegedBypassError, PrivilegedBypassReason, PrivilegedBypassRequest, ServiceCredentialId,
    VerifiedPrivilegedPrincipal, VerifiedServiceCredential as PolicyServiceCredential,
};

/// A presented credential whose debug representation cannot expose its value.
#[derive(Clone, Eq, PartialEq)]
pub struct PresentedServiceCredential(String);

impl PresentedServiceCredential {
    pub fn parse(value: impl Into<String>) -> Result<Self, ServiceBypassGatewayError> {
        let value = value.into();
        if !value.starts_with("mako_sk.")
            || value.len() < 32
            || value.len() > 512
            || value.chars().any(char::is_control)
        {
            return Err(ServiceBypassGatewayError::InvalidRequest);
        }
        Ok(Self(value))
    }
}

impl fmt::Debug for PresentedServiceCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PresentedServiceCredential([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct ServiceBypassGatewayRequest {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub operation: DocumentOperation,
    pub credential: PresentedServiceCredential,
    pub request_id: AuditRequestId,
    pub reason: PrivilegedBypassReason,
    pub now_unix_seconds: u64,
}

/// Verifies the explicit service credential, narrows it to one operation, and
/// establishes the fail-closed audited policy bypass.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceBypassGateway;

impl ServiceBypassGateway {
    pub async fn authorize(
        &self,
        request: ServiceBypassGatewayRequest,
        credentials: &IdentityStore,
        audit: &dyn PrivilegedBypassAuditSink,
    ) -> Result<PrivilegedBypassAuthorizer, ServiceBypassGatewayError> {
        if credentials.tenant() != &request.tenant {
            return Err(ServiceBypassGatewayError::Unauthenticated);
        }
        let verified = credentials
            .verify_project_credential(&request.credential.0, request.now_unix_seconds)
            .await
            .map_err(ServiceBypassGatewayError::CredentialUnavailable)?;
        let Some(VerifiedProjectCredential::Service(verified)) = verified else {
            return Err(ServiceBypassGatewayError::Unauthenticated);
        };
        let operation = identity_operation(request.operation);
        if verified.scope() != &request.tenant
            || !verified.permits(request.collection_id.as_str(), operation)
        {
            return Err(ServiceBypassGatewayError::Forbidden);
        }
        let scope = CollectionScope::new(request.tenant.clone(), request.collection_id.clone());
        let privileges = CredentialPrivileges::new(
            CollectionPrivileges::only([request.collection_id])?,
            [request.operation],
        )?;
        let principal =
            VerifiedPrivilegedPrincipal::Service(PolicyServiceCredential::from_verified_secret(
                ServiceCredentialId::parse(verified.credential_id().as_str())?,
                request.tenant,
                privileges,
            ));
        Ok(PrivilegedBypassAuthorizer::establish(
            PrivilegedBypassRequest::new(
                principal,
                scope,
                request.operation,
                PrivilegedBypassAuditContext::new(request.request_id, request.reason),
                request.now_unix_seconds,
            ),
            audit,
        )?)
    }
}

const fn identity_operation(operation: DocumentOperation) -> ServiceCredentialOperation {
    match operation {
        DocumentOperation::Create => ServiceCredentialOperation::Create,
        DocumentOperation::Read => ServiceCredentialOperation::Read,
        DocumentOperation::Update => ServiceCredentialOperation::Update,
        DocumentOperation::Delete => ServiceCredentialOperation::Delete,
    }
}

#[derive(Debug)]
pub enum ServiceBypassGatewayError {
    InvalidRequest,
    Unauthenticated,
    Forbidden,
    CredentialUnavailable(IdentityStoreError),
    Bypass(PrivilegedBypassError),
}

impl fmt::Display for ServiceBypassGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "service bypass request is invalid",
            Self::Unauthenticated => "service credential is invalid",
            Self::Forbidden => "service credential does not permit the operation",
            Self::CredentialUnavailable(_) => "service credential validation is unavailable",
            Self::Bypass(_) => "privileged bypass could not be established",
        })
    }
}

impl Error for ServiceBypassGatewayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CredentialUnavailable(error) => Some(error),
            Self::Bypass(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PrivilegedBypassError> for ServiceBypassGatewayError {
    fn from(error: PrivilegedBypassError) -> Self {
        Self::Bypass(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{ProjectCredentialId, ServiceCredentialScope};
    use mako_policy::{PrivilegedAuditWriteError, PrivilegedBypassAuditEvent};
    use mako_storage::{Durability, MemoryAdapter};

    use super::*;

    #[derive(Default)]
    struct Audit {
        events: Mutex<Vec<PrivilegedBypassAuditEvent>>,
    }

    impl PrivilegedBypassAuditSink for Audit {
        fn record(
            &self,
            event: &PrivilegedBypassAuditEvent,
        ) -> Result<(), PrivilegedAuditWriteError> {
            self.events.lock().expect("events").push(event.clone());
            Ok(())
        }
    }

    #[test]
    fn only_scoped_service_credentials_establish_an_audited_bypass() {
        futures::executor::block_on(async {
            let tenant = TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            let store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let issued = store
                .create_service_credential(
                    ProjectCredentialId::parse("service_edge").expect("credential id"),
                    ServiceCredentialScope::new(
                        ["todos".to_owned()],
                        [ServiceCredentialOperation::Read],
                    )
                    .expect("scope"),
                    1,
                )
                .await
                .expect("service credential");
            let credential =
                PresentedServiceCredential::parse(issued.credential.expose_once().to_owned())
                    .expect("presented credential");
            assert!(!format!("{credential:?}").contains(issued.credential.expose_once()));
            let audit = Audit::default();
            let request = ServiceBypassGatewayRequest {
                tenant: tenant.clone(),
                collection_id: CollectionId::parse("todos").expect("collection"),
                operation: DocumentOperation::Read,
                credential: credential.clone(),
                request_id: AuditRequestId::parse("req_abcdefgh").expect("request"),
                reason: PrivilegedBypassReason::parse("scheduled maintenance").expect("reason"),
                now_unix_seconds: 2,
            };
            let authorizer = ServiceBypassGateway
                .authorize(request.clone(), &store, &audit)
                .await
                .expect("authorized bypass");
            assert_eq!(authorizer.audit_event().principal_id(), "service_edge");
            assert_eq!(
                authorizer.audit_event().reason().as_str(),
                "scheduled maintenance"
            );
            assert_eq!(audit.events.lock().expect("events").len(), 1);

            let forbidden = ServiceBypassGatewayRequest {
                operation: DocumentOperation::Update,
                ..request
            };
            assert!(matches!(
                ServiceBypassGateway
                    .authorize(forbidden, &store, &audit)
                    .await,
                Err(ServiceBypassGatewayError::Forbidden)
            ));
            assert_eq!(audit.events.lock().expect("events").len(), 1);
        });
    }
}
