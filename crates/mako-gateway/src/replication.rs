use std::{error::Error, fmt, num::NonZeroU64, sync::Arc};

use async_trait::async_trait;
use mako_api::{
    ApiError, ApiErrorEnvelope, CollectionId, ErrorCode, RetryAdvice, SafeDetail, TenantScope,
};
use mako_identity::{IdentityStore, VerifiedProjectCredential};
use mako_policy::{
    CompiledPolicySet, DocumentPolicyAuthorizer, DocumentPolicyReadAuthorizer, SafeRequestMetadata,
    SubjectId, VerifiedIdentity, VerifiedRole,
};
use mako_sync::AuthenticatedReplicationContext;
use rand_core::{OsRng, RngCore};
use serde_json::Value;

use crate::{
    AccessTokenVerificationError, GatewayAccessTokenVerifier, GatewayQuotaCharge,
    GatewayQuotaDecision, GatewayQuotaEngine, GatewayQuotaPolicySource, GatewayQuotaResource,
};

const MAX_PUBLIC_KEY_BYTES: usize = 512;
const MAX_ACCESS_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationOperation {
    Pull,
    Push,
    Live,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplicationUsage {
    operation: ReplicationOperation,
    request_bytes: NonZeroU64,
}

impl ReplicationUsage {
    #[must_use]
    pub const fn new(operation: ReplicationOperation, request_bytes: NonZeroU64) -> Self {
        Self {
            operation,
            request_bytes,
        }
    }

    #[must_use]
    pub const fn operation(self) -> ReplicationOperation {
        self.operation
    }

    #[must_use]
    pub const fn request_bytes(self) -> NonZeroU64 {
        self.request_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplicationQuotaDecision {
    Allowed,
    Throttled {
        resource: String,
        retry_after_ms: u64,
    },
    HardLimit {
        resource: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaCheckError {
    Unavailable,
}

impl fmt::Display for QuotaCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("replication quota state is unavailable")
    }
}

impl Error for QuotaCheckError {}

#[async_trait]
pub trait ReplicationQuotaEnforcer: Send + Sync {
    async fn check(
        &self,
        tenant: &TenantScope,
        usage: ReplicationUsage,
        reservation_id: &str,
        now_unix_milliseconds: u64,
    ) -> Result<ReplicationQuotaDecision, QuotaCheckError>;
}

#[derive(Clone)]
pub struct GatewayReplicationQuotaEnforcer {
    engine: Arc<GatewayQuotaEngine>,
    policies: Arc<dyn GatewayQuotaPolicySource>,
}

impl GatewayReplicationQuotaEnforcer {
    #[must_use]
    pub fn new(
        engine: Arc<GatewayQuotaEngine>,
        policies: Arc<dyn GatewayQuotaPolicySource>,
    ) -> Self {
        Self { engine, policies }
    }
}

#[async_trait]
impl ReplicationQuotaEnforcer for GatewayReplicationQuotaEnforcer {
    async fn check(
        &self,
        tenant: &TenantScope,
        usage: ReplicationUsage,
        reservation_id: &str,
        now_unix_milliseconds: u64,
    ) -> Result<ReplicationQuotaDecision, QuotaCheckError> {
        let policy = self
            .policies
            .policy_for(tenant)
            .await
            .map_err(|_| QuotaCheckError::Unavailable)?;
        let charges = [
            GatewayQuotaCharge {
                resource: GatewayQuotaResource::ReplicationRequests,
                amount: NonZeroU64::MIN,
            },
            GatewayQuotaCharge {
                resource: GatewayQuotaResource::ReplicationBytes,
                amount: usage.request_bytes(),
            },
        ];
        match self
            .engine
            .check_and_reserve(
                tenant,
                reservation_id,
                &charges,
                &policy,
                now_unix_milliseconds,
            )
            .await
            .map_err(|_| QuotaCheckError::Unavailable)?
        {
            GatewayQuotaDecision::Allowed => Ok(ReplicationQuotaDecision::Allowed),
            GatewayQuotaDecision::Throttled {
                resource,
                retry_after_milliseconds,
            } => Ok(ReplicationQuotaDecision::Throttled {
                resource: resource.as_str().to_owned(),
                retry_after_ms: retry_after_milliseconds,
            }),
            GatewayQuotaDecision::HardLimit { resource } => {
                Ok(ReplicationQuotaDecision::HardLimit {
                    resource: resource.as_str().to_owned(),
                })
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplicationGatewayRequest {
    tenant: TenantScope,
    collection_id: CollectionId,
    public_project_key: String,
    access_token: String,
    now_unix_seconds: u64,
    usage: ReplicationUsage,
    request_metadata: SafeRequestMetadata,
    request_id: Option<String>,
}

impl ReplicationGatewayRequest {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        tenant: TenantScope,
        collection_id: CollectionId,
        public_project_key: impl Into<String>,
        access_token: impl Into<String>,
        now_unix_seconds: u64,
        usage: ReplicationUsage,
        request_metadata: SafeRequestMetadata,
    ) -> Self {
        Self {
            tenant,
            collection_id,
            public_project_key: public_project_key.into(),
            access_token: access_token.into(),
            now_unix_seconds,
            usage,
            request_metadata,
            request_id: None,
        }
    }

    /// Preserves the transport request identifier across gateway, sync, audit,
    /// and error boundaries. Callers that do not provide one receive a fresh
    /// gateway-generated identifier for backwards compatibility.
    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    fn is_bounded(&self) -> bool {
        !self.public_project_key.is_empty()
            && self.public_project_key.len() <= MAX_PUBLIC_KEY_BYTES
            && !self.access_token.is_empty()
            && self.access_token.len() <= MAX_ACCESS_TOKEN_BYTES
            && self.request_id.as_ref().is_none_or(|request_id| {
                !request_id.is_empty()
                    && request_id.len() <= 256
                    && !request_id.chars().any(char::is_control)
            })
    }
}

#[derive(Clone, Debug)]
pub struct AuthorizedReplicationRequest {
    sync_context: AuthenticatedReplicationContext,
    policy_identity: VerifiedIdentity,
    request_metadata: SafeRequestMetadata,
}

impl AuthorizedReplicationRequest {
    #[must_use]
    pub const fn sync_context(&self) -> &AuthenticatedReplicationContext {
        &self.sync_context
    }

    #[must_use]
    pub fn policy_identity(&self) -> &VerifiedIdentity {
        &self.policy_identity
    }

    #[must_use]
    pub fn request_metadata(&self) -> &SafeRequestMetadata {
        &self.request_metadata
    }

    #[must_use]
    pub fn policy_read_authorizer<'a>(
        &'a self,
        policy: Option<&'a CompiledPolicySet>,
    ) -> DocumentPolicyReadAuthorizer<'a> {
        DocumentPolicyReadAuthorizer::new(policy, &self.policy_identity, &self.request_metadata)
    }

    #[must_use]
    pub fn policy_mutation_authorizer<'a>(
        &'a self,
        policy: Option<&'a CompiledPolicySet>,
    ) -> DocumentPolicyAuthorizer<'a> {
        DocumentPolicyAuthorizer::new(policy, &self.policy_identity, &self.request_metadata)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReplicationGateway;

impl ReplicationGateway {
    pub async fn authorize(
        &self,
        request: ReplicationGatewayRequest,
        credentials: &IdentityStore,
        access_tokens: &GatewayAccessTokenVerifier<'_>,
        quotas: &dyn ReplicationQuotaEnforcer,
    ) -> Result<AuthorizedReplicationRequest, ReplicationGatewayError> {
        let request_id = request
            .request_id
            .clone()
            .unwrap_or_else(generate_request_id);
        if !request.is_bounded() {
            return Err(gateway_error(
                ErrorCode::InvalidRequest,
                "replication request credentials are invalid",
                &request_id,
                RetryAdvice::Never,
            ));
        }
        let credential = credentials
            .verify_project_credential(&request.public_project_key, request.now_unix_seconds)
            .await
            .map_err(|_| {
                gateway_error(
                    ErrorCode::Unavailable,
                    "project credential validation is unavailable",
                    &request_id,
                    RetryAdvice::Immediate,
                )
            })?;
        let valid_public_key = matches!(
            credential,
            Some(VerifiedProjectCredential::Public(ref public))
                if public.scope() == &request.tenant
        );
        if !valid_public_key {
            return Err(gateway_error(
                ErrorCode::Unauthenticated,
                "project credentials are invalid",
                &request_id,
                RetryAdvice::Never,
            ));
        }

        let access_identity = access_tokens
            .verify(
                &request.access_token,
                &request.tenant,
                request.now_unix_seconds,
            )
            .map_err(|error| access_error(error, &request_id))?;

        let now_unix_milliseconds =
            request.now_unix_seconds.checked_mul(1_000).ok_or_else(|| {
                gateway_error(
                    ErrorCode::InvalidRequest,
                    "replication request timestamp is invalid",
                    &request_id,
                    RetryAdvice::Never,
                )
            })?;
        match quotas
            .check(
                &request.tenant,
                request.usage,
                &request_id,
                now_unix_milliseconds,
            )
            .await
        {
            Ok(ReplicationQuotaDecision::Allowed) => {}
            Ok(ReplicationQuotaDecision::Throttled {
                resource,
                retry_after_ms,
            }) => {
                let mut error = gateway_error(
                    ErrorCode::RateLimited,
                    "replication request is throttled",
                    &request_id,
                    RetryAdvice::AfterDelay {
                        after_ms: retry_after_ms,
                    },
                );
                error
                    .api_error
                    .error
                    .details
                    .insert("resource".to_owned(), SafeDetail::String(resource));
                return Err(error);
            }
            Ok(ReplicationQuotaDecision::HardLimit { resource }) => {
                let mut error = gateway_error(
                    ErrorCode::QuotaExceeded,
                    "replication quota is exhausted",
                    &request_id,
                    RetryAdvice::Never,
                );
                error
                    .api_error
                    .error
                    .details
                    .insert("resource".to_owned(), SafeDetail::String(resource));
                return Err(error);
            }
            Err(QuotaCheckError::Unavailable) => {
                return Err(gateway_error(
                    ErrorCode::Unavailable,
                    "replication quota state is unavailable",
                    &request_id,
                    RetryAdvice::AfterDelay { after_ms: 1_000 },
                ));
            }
        }

        let policy_identity = VerifiedIdentity::user(
            SubjectId::parse(access_identity.user_id().as_str()).map_err(|_| {
                gateway_error(
                    ErrorCode::Internal,
                    "verified policy context is invalid",
                    &request_id,
                    RetryAdvice::Never,
                )
            })?,
            VerifiedRole::parse(access_identity.role()).map_err(|_| {
                gateway_error(
                    ErrorCode::Internal,
                    "verified policy context is invalid",
                    &request_id,
                    RetryAdvice::Never,
                )
            })?,
            access_identity.policy_email(),
            Value::Object(access_identity.trusted_claims().clone()),
        )
        .map_err(|_| {
            gateway_error(
                ErrorCode::Internal,
                "verified policy context is invalid",
                &request_id,
                RetryAdvice::Never,
            )
        })?;
        let sync_context = AuthenticatedReplicationContext::new(
            request.tenant,
            request.collection_id,
            access_identity.user_id().clone(),
            access_identity.session_id().clone(),
            access_identity.role(),
            access_identity.authorization_epochs(),
            request_id.clone(),
        )
        .map_err(|_| {
            gateway_error(
                ErrorCode::Internal,
                "verified replication context is invalid",
                &request_id,
                RetryAdvice::Never,
            )
        })?;
        Ok(AuthorizedReplicationRequest {
            sync_context,
            policy_identity,
            request_metadata: request.request_metadata,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplicationGatewayError {
    api_error: Box<ApiErrorEnvelope>,
}

impl ReplicationGatewayError {
    #[must_use]
    pub fn api_error(&self) -> &ApiErrorEnvelope {
        &self.api_error
    }
}

impl fmt::Display for ReplicationGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.api_error.error.message)
    }
}

impl Error for ReplicationGatewayError {}

fn access_error(_: AccessTokenVerificationError, request_id: &str) -> ReplicationGatewayError {
    gateway_error(
        ErrorCode::Unauthenticated,
        "application-user session is invalid",
        request_id,
        RetryAdvice::Never,
    )
}

fn gateway_error(
    code: ErrorCode,
    message: &'static str,
    request_id: &str,
    retry: RetryAdvice,
) -> ReplicationGatewayError {
    ReplicationGatewayError {
        api_error: Box::new(ApiErrorEnvelope::new(ApiError::new(
            code, message, request_id, retry,
        ))),
    }
}

fn generate_request_id() -> String {
    let mut random = [0_u8; 12];
    OsRng.fill_bytes(&mut random);
    let mut request_id = String::from("req_");
    for byte in random {
        use fmt::Write;
        write!(&mut request_id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    request_id
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{
        AccessAuthorizationEpochs, AccessTokenConfig, AccessTokenInput, AccessTokenIssuer,
        AppUserId, KeyEncryptionKey, ProjectCredentialId, ProjectSigningKeyRing, SessionId,
    };
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::{Map, json};

    use super::*;
    use crate::{
        AccessTokenVerificationConfig, GatewayAuthorizationEpochResolver, GatewaySessionResolver,
    };

    struct AccessState {
        active: bool,
    }

    impl GatewaySessionResolver for AccessState {
        fn is_active(&self, _: &TenantScope, _: &AppUserId, _: &SessionId, _: u64) -> bool {
            self.active
        }
    }

    impl GatewayAuthorizationEpochResolver for AccessState {
        fn current_epochs(
            &self,
            _: &TenantScope,
            _: &AppUserId,
        ) -> Option<AccessAuthorizationEpochs> {
            Some(AccessAuthorizationEpochs {
                environment: 2,
                user: 4,
            })
        }
    }

    struct Quota(ReplicationQuotaDecision);

    #[async_trait]
    impl ReplicationQuotaEnforcer for Quota {
        async fn check(
            &self,
            _: &TenantScope,
            _: ReplicationUsage,
            _: &str,
            _: u64,
        ) -> Result<ReplicationQuotaDecision, QuotaCheckError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn authorizes_public_user_context_and_returns_retryable_quota_errors() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let identity_store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("identity store");
            let public_key = identity_store
                .create_public_project_key(
                    ProjectCredentialId::parse("public_gateway").expect("credential id"),
                    100,
                )
                .await
                .expect("public key")
                .credential
                .expose_once()
                .to_owned();
            let encryption_key = KeyEncryptionKey::generate();
            let mut ring = ProjectSigningKeyRing::new(tenant.clone());
            ring.create_initial(&encryption_key, 100).expect("key");
            let token = AccessTokenIssuer::new(
                &ring,
                &encryption_key,
                AccessTokenConfig::new("https://issuer.test", "mako-data", 300).expect("config"),
            )
            .issue(
                AccessTokenInput {
                    tenant: tenant.clone(),
                    user_id: AppUserId::parse("usr_abcdefgh").expect("user"),
                    role: "member".to_owned(),
                    email: "policy@example.test".to_owned(),
                    email_verified: true,
                    session_id: SessionId::parse("ses_abcdefgh").expect("session"),
                    authorization_epochs: AccessAuthorizationEpochs {
                        environment: 2,
                        user: 4,
                    },
                    trusted_claims: Map::from_iter([("team".to_owned(), json!("blue"))]),
                },
                100,
            )
            .expect("token")
            .expose_for_authorization_header()
            .to_owned();
            let jwks = ring.jwks();
            let active = AccessState { active: true };
            let verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &active,
                &active,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-data", 30)
                    .expect("config"),
            );
            let gateway = ReplicationGateway;
            let authorized = gateway
                .authorize(
                    request(&tenant, &public_key, &token)
                        .with_request_id("req_transport_correlation"),
                    &identity_store,
                    &verifier,
                    &Quota(ReplicationQuotaDecision::Allowed),
                )
                .await
                .expect("authorized");
            assert_eq!(authorized.sync_context().tenant(), &tenant);
            assert_eq!(authorized.sync_context().role(), "member");
            assert_eq!(
                authorized.sync_context().request_id(),
                "req_transport_correlation"
            );
            assert_eq!(
                authorized.policy_identity().trusted_claims()["team"],
                "blue"
            );
            let _read_authorizer = authorized.policy_read_authorizer(None);
            let _mutation_authorizer = authorized.policy_mutation_authorizer(None);

            let throttled = gateway
                .authorize(
                    request(&tenant, &public_key, &token),
                    &identity_store,
                    &verifier,
                    &Quota(ReplicationQuotaDecision::Throttled {
                        resource: "replication_requests".to_owned(),
                        retry_after_ms: 2_500,
                    }),
                )
                .await
                .expect_err("throttled");
            assert_eq!(throttled.api_error().error.code, ErrorCode::RateLimited);
            assert_eq!(
                throttled.api_error().error.retry,
                RetryAdvice::AfterDelay { after_ms: 2_500 }
            );
            assert!(throttled.api_error().error.request_id.starts_with("req_"));

            let hard_limit = gateway
                .authorize(
                    request(&tenant, &public_key, &token),
                    &identity_store,
                    &verifier,
                    &Quota(ReplicationQuotaDecision::HardLimit {
                        resource: "replication_bytes".to_owned(),
                    }),
                )
                .await
                .expect_err("hard quota");
            assert_eq!(hard_limit.api_error().error.code, ErrorCode::QuotaExceeded);
            assert_eq!(hard_limit.api_error().error.retry, RetryAdvice::Never);
            assert_eq!(
                hard_limit.api_error().error.details.get("resource"),
                Some(&SafeDetail::String("replication_bytes".to_owned()))
            );

            let revoked = AccessState { active: false };
            let revoked_verifier = GatewayAccessTokenVerifier::new(
                &jwks,
                &revoked,
                &revoked,
                AccessTokenVerificationConfig::new("https://issuer.test", "mako-data", 30)
                    .expect("config"),
            );
            let rejected = gateway
                .authorize(
                    request(&tenant, &public_key, &token),
                    &identity_store,
                    &revoked_verifier,
                    &Quota(ReplicationQuotaDecision::Allowed),
                )
                .await
                .expect_err("revoked session");
            assert_eq!(rejected.api_error().error.code, ErrorCode::Unauthenticated);
            assert_eq!(rejected.api_error().error.retry, RetryAdvice::Never);
        });
    }

    fn request(tenant: &TenantScope, public_key: &str, token: &str) -> ReplicationGatewayRequest {
        ReplicationGatewayRequest::new(
            tenant.clone(),
            CollectionId::parse("todos").expect("collection"),
            public_key,
            token,
            101,
            ReplicationUsage::new(
                ReplicationOperation::Pull,
                NonZeroU64::new(1).expect("usage"),
            ),
            SafeRequestMetadata::new([("transport".to_owned(), "https".to_owned())])
                .expect("metadata"),
        )
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
