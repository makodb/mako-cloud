use std::{error::Error, fmt};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

const STATE_KEY_CONTEXT: &str = "mako/auth-providers/flow-state/v1";
pub const FLOW_STATE_LIFETIME_SECONDS: u64 = 10 * 60;

/// What a browser carries through the provider and back: which environment
/// and provider started the flow, where to return, and a nonce the id token
/// must echo. Signed, so the callback trusts nothing it did not issue.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FlowState {
    pub project_id: String,
    pub environment_id: String,
    pub provider: String,
    pub redirect_url: String,
    pub nonce: String,
    pub issued_at_unix_seconds: u64,
}

#[derive(Clone)]
pub struct FlowStateKey([u8; 32]);

impl fmt::Debug for FlowStateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FlowStateKey([REDACTED])")
    }
}

impl FlowStateKey {
    #[must_use]
    pub fn derive(internal_secret: &[u8]) -> Self {
        Self(blake3::derive_key(STATE_KEY_CONTEXT, internal_secret))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowStateError {
    Malformed,
    BadSignature,
    Expired,
    /// The state names a different environment than the callback route.
    ScopeMismatch,
}

impl fmt::Display for FlowStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "flow state is malformed",
            Self::BadSignature => "flow state signature is invalid",
            Self::Expired => "flow state has expired",
            Self::ScopeMismatch => "flow state belongs to another environment",
        })
    }
}

impl Error for FlowStateError {}

#[derive(Clone, Debug)]
pub struct FlowStateVerifier {
    key: FlowStateKey,
}

impl FlowStateVerifier {
    #[must_use]
    pub const fn new(key: FlowStateKey) -> Self {
        Self { key }
    }

    /// A fresh state for a flow; the returned string goes to the provider as `state`.
    pub fn issue(
        &self,
        tenant: &TenantScope,
        provider: &str,
        redirect_url: &str,
        now_unix_seconds: u64,
    ) -> Result<(FlowState, String), FlowStateError> {
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let state = FlowState {
            project_id: tenant.project_id().as_str().to_owned(),
            environment_id: tenant.environment_id().as_str().to_owned(),
            provider: provider.to_owned(),
            redirect_url: redirect_url.to_owned(),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            issued_at_unix_seconds: now_unix_seconds,
        };
        let payload = serde_json::to_vec(&state).map_err(|_| FlowStateError::Malformed)?;
        let signature = blake3::keyed_hash(&self.key.0, &payload);
        let token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(signature.as_bytes())
        );
        Ok((state, token))
    }

    /// The state a callback presented, if it is ours, fresh, and for `tenant`.
    pub fn verify(
        &self,
        tenant: &TenantScope,
        token: &str,
        now_unix_seconds: u64,
    ) -> Result<FlowState, FlowStateError> {
        if token.len() > 8192 {
            return Err(FlowStateError::Malformed);
        }
        let (payload, signature) = token.split_once('.').ok_or(FlowStateError::Malformed)?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| FlowStateError::Malformed)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| FlowStateError::Malformed)?;
        let expected = blake3::keyed_hash(&self.key.0, &payload);
        if signature.len() != 32 || !constant_time_eq(expected.as_bytes(), &signature) {
            return Err(FlowStateError::BadSignature);
        }
        let state: FlowState =
            serde_json::from_slice(&payload).map_err(|_| FlowStateError::Malformed)?;
        if state.project_id != tenant.project_id().as_str()
            || state.environment_id != tenant.environment_id().as_str()
        {
            return Err(FlowStateError::ScopeMismatch);
        }
        if now_unix_seconds < state.issued_at_unix_seconds
            || now_unix_seconds - state.issued_at_unix_seconds > FLOW_STATE_LIFETIME_SECONDS
        {
            return Err(FlowStateError::Expired);
        }
        Ok(state)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}
