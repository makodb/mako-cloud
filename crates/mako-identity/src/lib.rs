//! Project application-user identity, credentials, and sessions.

#![forbid(unsafe_code)]

mod access_token;
mod admin;
mod email;
mod lifecycle;
mod password;
mod project_credentials;
mod rate_limit;
mod records;
mod recovery;
mod refresh;
mod session_store;
mod signin;
mod signing_key_store;
mod signing_keys;
mod signup;
mod store;

pub use access_token::{
    AccessAuthorizationEpochs, AccessToken, AccessTokenClaims, AccessTokenConfig, AccessTokenError,
    AccessTokenInput, AccessTokenIssuer,
};
pub use admin::{
    AdminAuditOutcome, AdminCreateUserRequest, AdminRequestContext, AdminSessionSummary,
    AdminUpdateUserMetadataRequest, AdminUserAction, AdminUserApiError, AdminUserAuditError,
    AdminUserAuditEvent, AdminUserAuditSink, AdminUserPermission, AdminUserSearchPage,
    AdminUserService, AdminUserSummary, AdminUserView, ApplicationUserInvitation,
    ApplicationUserInvitationSink, TrustedMetadataInvalidationError,
    TrustedMetadataInvalidationSink,
};
pub use email::NormalizedEmail;
pub use lifecycle::{IdentityRevocationEvent, IdentityRevocationKind};
pub use password::{
    Argon2idParameters, PasswordError, PasswordPolicy, PasswordPolicyViolation, PasswordService,
    PasswordVerification, StoredPasswordHash,
};
pub use project_credentials::{
    IssuedProjectCredential, PreparedProjectCredentialCreate, PreparedProjectCredentialRotation,
    ProjectCredential, ProjectCredentialId, ProjectCredentialKind, ProjectCredentialMetadata,
    ProjectCredentialState, ServiceCredentialOperation, ServiceCredentialScope,
    VerifiedProjectCredential, VerifiedPublicCredential, VerifiedServiceCredential,
};
pub use rate_limit::{
    FixedWindowRateLimitConfig, FixedWindowSignInThrottle, PersistentSignInThrottle, RateLimitError,
};
pub use recovery::{
    PasswordChangeOutcome, PasswordRecoveryEmail, PasswordRecoveryEmailProvider,
    PasswordRecoveryError, PasswordRecoveryResponse, PasswordRecoveryService,
    PasswordRecoveryToken,
};
pub use refresh::{
    RefreshCredential, RefreshError, RefreshExchange, RefreshExchangeOutcome, RefreshFamilyState,
    RefreshTokenFamily,
};
pub use session_store::{
    ApplicationSessionGrant, ApplicationSessionStore, ApplicationSessionStoreError,
    RefreshSessionOutcome,
};

pub use records::{
    AppUserId, AppUserRecord, AppUserStatus, CredentialDigest, IdentityProvider,
    IdentityRecordError, ProviderName, SessionId, SessionRecord, SessionStatus, TokenFamilyId,
    TokenFamilyRecord, TokenFamilyStatus, TrustedAppMetadata, UserCredentialId, UserCredentialKind,
    UserCredentialRecord, UserIdentityId, UserIdentityRecord, UserProfileMetadata,
};
pub use signin::{
    AuthenticationAuditError, AuthenticationAuditEvent, AuthenticationAuditOutcome,
    AuthenticationAuditSink, SignInAttemptOutcome, SignInError, SignInRequestMetadata,
    SignInResponse, SignInService, SignInThrottle, SignInThrottleDecision, SignInThrottleKey,
};
pub use signing_key_store::{ProjectSigningKeyStore, ProjectSigningKeyStoreError};
pub use signing_keys::{
    JsonWebKey, JsonWebKeySet, KeyEncryptionKey, ProjectSigningKeyRecord, ProjectSigningKeyRing,
    SigningKeyError, SigningKeyState,
};
pub use signup::{
    EmailSignupConfig, SignupError, SignupResponse, SignupService, TransactionalEmailProvider,
    VerificationEmail, VerificationToken,
};
pub use store::{
    AppUserMetadataUpdate, EmailVerificationOutcome, IdentityStore, IdentityStoreError,
    MagicLinkOutcome, PasswordLinkOutcome, PasswordResetOutcome,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "identity";
