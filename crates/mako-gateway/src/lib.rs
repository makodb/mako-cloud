//! Shared data, management, operator, and edge gateway concerns.

#![forbid(unsafe_code)]

mod quota;
mod replication;
mod revocation_cache;
mod service_bypass;
mod token_verifier;

pub use quota::{
    GatewayQuotaCharge, GatewayQuotaDecision, GatewayQuotaEngine, GatewayQuotaEngineConfig,
    GatewayQuotaError, GatewayQuotaLimit, GatewayQuotaPolicy, GatewayQuotaPolicyError,
    GatewayQuotaPolicySource, GatewayQuotaResource, GatewayQuotaWindow,
    PersistentQuotaPolicySource,
};
pub use replication::{
    AuthorizedReplicationRequest, GatewayReplicationQuotaEnforcer, QuotaCheckError,
    ReplicationGateway, ReplicationGatewayError, ReplicationGatewayRequest, ReplicationOperation,
    ReplicationQuotaDecision, ReplicationQuotaEnforcer, ReplicationUsage,
};
pub use revocation_cache::{
    AuthoritativeSessionState, RevocationCache, RevocationCacheError, RevocationSource,
    RevocationSourceError,
};
pub use service_bypass::{
    PresentedServiceCredential, ServiceBypassGateway, ServiceBypassGatewayError,
    ServiceBypassGatewayRequest,
};
pub use token_verifier::{
    AccessTokenVerificationConfig, AccessTokenVerificationError, GatewayAccessTokenVerifier,
    GatewayAuthorizationEpochResolver, GatewaySessionResolver, VerifiedAccessIdentity,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "gateway";
