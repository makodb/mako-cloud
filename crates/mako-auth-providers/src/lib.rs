//! External sign-in for application users: the provider settings a developer
//! configures, the sealed client secrets that travel with them, the signed
//! state that ties a browser round trip to an environment, and the OAuth 2.0
//! and OpenID Connect exchanges that turn a callback into a verified email
//! and a provider subject.
//!
//! Nothing here mints a session or touches a user record; the data plane
//! does that with what this crate verifies.
mod exchange;
mod settings;
mod state;

pub use exchange::{ProviderClient, ProviderClientConfig, ProviderExchangeError, ProviderIdentity};
pub use settings::{
    AuthProviderSettings, EmailVerificationSettings, MagicLinkSettings, ProviderConfig,
    ProviderConfigPlain, ProviderKind, ProviderSecretKey, SealedSecret, SettingsError,
    sealed_settings_from_plain, valid_provider_name,
};
pub use state::{FlowState, FlowStateError, FlowStateKey, FlowStateVerifier};
