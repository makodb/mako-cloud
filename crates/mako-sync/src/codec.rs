use std::{error::Error, fmt};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::TenantScope;
use mako_identity::AppUserId;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{
    AuthenticatedReplicationContext, OpaqueCheckpoint, OpaqueStreamCursor, ReplicationContractError,
};

const CHECKPOINT_PREFIX: &str = "mcp1.";
const STREAM_CURSOR_PREFIX: &str = "msc1.";
const MAX_PAYLOAD_BYTES: usize = 2_048;

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationTokenKey([u8; 32]);

impl ReplicationTokenKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl fmt::Debug for ReplicationTokenKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplicationTokenKey([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedCheckpoint {
    scanned_position: u64,
    document_tiebreaker: Option<String>,
}

impl VerifiedCheckpoint {
    #[must_use]
    pub const fn scanned_position(&self) -> u64 {
        self.scanned_position
    }

    #[must_use]
    pub fn document_tiebreaker(&self) -> Option<&str> {
        self.document_tiebreaker.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedStreamCursor {
    after_position: u64,
    through_high_water: u64,
    document_tiebreaker: Option<String>,
}

impl VerifiedStreamCursor {
    #[must_use]
    pub const fn after_position(&self) -> u64 {
        self.after_position
    }

    #[must_use]
    pub const fn through_high_water(&self) -> u64 {
        self.through_high_water
    }

    #[must_use]
    pub fn document_tiebreaker(&self) -> Option<&str> {
        self.document_tiebreaker.as_deref()
    }
}

pub struct ReplicationTokenCodec<'a> {
    key: &'a ReplicationTokenKey,
}

impl<'a> ReplicationTokenCodec<'a> {
    #[must_use]
    pub const fn new(key: &'a ReplicationTokenKey) -> Self {
        Self { key }
    }

    pub fn encode_checkpoint(
        &self,
        context: &AuthenticatedReplicationContext,
        schema_version: u64,
        scanned_position: u64,
        document_tiebreaker: Option<&str>,
    ) -> Result<OpaqueCheckpoint, ReplicationTokenCodecError> {
        validate_schema_and_tiebreaker(schema_version, document_tiebreaker)?;
        let claims = CheckpointClaims {
            binding: binding(context, schema_version),
            scanned_position,
            document_tiebreaker: document_tiebreaker.map(str::to_owned),
        };
        OpaqueCheckpoint::new(self.encode(CHECKPOINT_PREFIX, &claims)?)
            .map_err(ReplicationTokenCodecError::Contract)
    }

    pub fn decode_checkpoint(
        &self,
        checkpoint: &OpaqueCheckpoint,
        context: &AuthenticatedReplicationContext,
        schema_version: u64,
    ) -> Result<VerifiedCheckpoint, ReplicationTokenCodecError> {
        if schema_version == 0 {
            return Err(ReplicationTokenCodecError::InvalidClaims);
        }
        let claims: CheckpointClaims = self.decode(CHECKPOINT_PREFIX, checkpoint.as_str())?;
        verify_binding(&claims.binding, context, schema_version)?;
        validate_schema_and_tiebreaker(schema_version, claims.document_tiebreaker.as_deref())?;
        Ok(VerifiedCheckpoint {
            scanned_position: claims.scanned_position,
            document_tiebreaker: claims.document_tiebreaker,
        })
    }

    pub fn encode_stream_cursor(
        &self,
        context: &AuthenticatedReplicationContext,
        schema_version: u64,
        after_position: u64,
        through_high_water: u64,
        document_tiebreaker: Option<&str>,
    ) -> Result<OpaqueStreamCursor, ReplicationTokenCodecError> {
        validate_schema_and_tiebreaker(schema_version, document_tiebreaker)?;
        if after_position > through_high_water {
            return Err(ReplicationTokenCodecError::InvalidClaims);
        }
        let claims = StreamCursorClaims {
            binding: binding(context, schema_version),
            after_position,
            through_high_water,
            document_tiebreaker: document_tiebreaker.map(str::to_owned),
        };
        OpaqueStreamCursor::new(self.encode(STREAM_CURSOR_PREFIX, &claims)?)
            .map_err(ReplicationTokenCodecError::Contract)
    }

    pub fn decode_stream_cursor(
        &self,
        cursor: &OpaqueStreamCursor,
        context: &AuthenticatedReplicationContext,
        schema_version: u64,
    ) -> Result<VerifiedStreamCursor, ReplicationTokenCodecError> {
        if schema_version == 0 {
            return Err(ReplicationTokenCodecError::InvalidClaims);
        }
        let claims: StreamCursorClaims = self.decode(STREAM_CURSOR_PREFIX, cursor.as_str())?;
        verify_binding(&claims.binding, context, schema_version)?;
        validate_schema_and_tiebreaker(schema_version, claims.document_tiebreaker.as_deref())?;
        if claims.after_position > claims.through_high_water {
            return Err(ReplicationTokenCodecError::InvalidClaims);
        }
        Ok(VerifiedStreamCursor {
            after_position: claims.after_position,
            through_high_water: claims.through_high_water,
            document_tiebreaker: claims.document_tiebreaker,
        })
    }

    fn encode<T: Serialize>(
        &self,
        domain: &str,
        value: &T,
    ) -> Result<String, ReplicationTokenCodecError> {
        let payload = serde_json::to_vec(value)?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(ReplicationTokenCodecError::PayloadTooLarge);
        }
        let signature = keyed_signature(&self.key.0, domain, &payload);
        Ok(format!(
            "{domain}{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    fn decode<T: for<'de> Deserialize<'de>>(
        &self,
        domain: &str,
        encoded: &str,
    ) -> Result<T, ReplicationTokenCodecError> {
        let encoded = encoded
            .strip_prefix(domain)
            .ok_or(ReplicationTokenCodecError::Malformed)?;
        let (payload, signature) = encoded
            .split_once('.')
            .ok_or(ReplicationTokenCodecError::Malformed)?;
        if signature.contains('.') {
            return Err(ReplicationTokenCodecError::Malformed);
        }
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| ReplicationTokenCodecError::Malformed)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| ReplicationTokenCodecError::Malformed)?;
        if payload.len() > MAX_PAYLOAD_BYTES
            || !constant_time_matches(&keyed_signature(&self.key.0, domain, &payload), &signature)
        {
            return Err(ReplicationTokenCodecError::InvalidSignature);
        }
        serde_json::from_slice(&payload).map_err(Into::into)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TokenBinding {
    tenant: TenantScope,
    collection_id: String,
    user_id: AppUserId,
    schema_version: u64,
    environment_authorization_epoch: u64,
    user_authorization_epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CheckpointClaims {
    binding: TokenBinding,
    scanned_position: u64,
    document_tiebreaker: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StreamCursorClaims {
    binding: TokenBinding,
    after_position: u64,
    through_high_water: u64,
    document_tiebreaker: Option<String>,
}

fn binding(context: &AuthenticatedReplicationContext, schema_version: u64) -> TokenBinding {
    let epochs = context.authorization_epochs();
    TokenBinding {
        tenant: context.tenant().clone(),
        collection_id: context.collection_id().as_str().to_owned(),
        user_id: context.user_id().clone(),
        schema_version,
        environment_authorization_epoch: epochs.environment,
        user_authorization_epoch: epochs.user,
    }
}

fn verify_binding(
    binding: &TokenBinding,
    context: &AuthenticatedReplicationContext,
    schema_version: u64,
) -> Result<(), ReplicationTokenCodecError> {
    if binding != &self::binding(context, schema_version) {
        return Err(ReplicationTokenCodecError::BindingMismatch);
    }
    Ok(())
}

fn validate_schema_and_tiebreaker(
    schema_version: u64,
    document_tiebreaker: Option<&str>,
) -> Result<(), ReplicationTokenCodecError> {
    if schema_version == 0
        || document_tiebreaker.is_some_and(|value| {
            value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
        })
    {
        return Err(ReplicationTokenCodecError::InvalidClaims);
    }
    Ok(())
}

fn keyed_signature(key: &[u8; 32], domain: &str, payload: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(payload);
    *hasher.finalize().as_bytes()
}

fn constant_time_matches(expected: &[u8], actual: &[u8]) -> bool {
    expected.len() == actual.len()
        && expected
            .iter()
            .zip(actual)
            .fold(0_u8, |difference, (expected, actual)| {
                difference | (expected ^ actual)
            })
            == 0
}

#[derive(Debug)]
pub enum ReplicationTokenCodecError {
    Malformed,
    InvalidSignature,
    InvalidClaims,
    BindingMismatch,
    PayloadTooLarge,
    Contract(ReplicationContractError),
    Json(serde_json::Error),
}

impl fmt::Display for ReplicationTokenCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("replication token is malformed"),
            Self::InvalidSignature => formatter.write_str("replication token signature is invalid"),
            Self::InvalidClaims => formatter.write_str("replication token claims are invalid"),
            Self::BindingMismatch => {
                formatter.write_str("replication token is bound to another request context")
            }
            Self::PayloadTooLarge => formatter.write_str("replication token payload is too large"),
            Self::Contract(error) => error.fmt(formatter),
            Self::Json(_) => formatter.write_str("replication token payload is invalid"),
        }
    }
}

impl Error for ReplicationTokenCodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for ReplicationTokenCodecError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, EnvironmentId, ProjectId};
    use mako_identity::{AccessAuthorizationEpochs, SessionId};

    use super::*;

    #[test]
    fn signed_tokens_round_trip_and_reject_tampering_or_binding_changes() {
        let key = ReplicationTokenKey::from_bytes([7; 32]);
        let codec = ReplicationTokenCodec::new(&key);
        let request_context = context("prj_abcdefgh", "usr_abcdefgh", 3);
        let checkpoint = codec
            .encode_checkpoint(&request_context, 4, 91, Some("doc-9"))
            .expect("checkpoint");
        let verified = codec
            .decode_checkpoint(&checkpoint, &request_context, 4)
            .expect("verify");
        assert_eq!(verified.scanned_position(), 91);
        assert_eq!(verified.document_tiebreaker(), Some("doc-9"));
        assert!(!format!("{checkpoint:?}").contains(checkpoint.as_str()));

        let mut tampered = checkpoint.as_str().to_owned();
        tampered.push('a');
        let tampered = OpaqueCheckpoint::new(tampered).expect("shape");
        assert!(matches!(
            codec.decode_checkpoint(&tampered, &request_context, 4),
            Err(ReplicationTokenCodecError::InvalidSignature)
                | Err(ReplicationTokenCodecError::Malformed)
        ));
        assert!(matches!(
            codec.decode_checkpoint(&checkpoint, &context("prj_abcdefgh", "usr_otheruser", 3), 4),
            Err(ReplicationTokenCodecError::BindingMismatch)
        ));
        assert!(matches!(
            codec.decode_checkpoint(&checkpoint, &request_context, 5),
            Err(ReplicationTokenCodecError::BindingMismatch)
        ));

        let cursor = codec
            .encode_stream_cursor(&request_context, 4, 91, 120, Some("doc-9"))
            .expect("cursor");
        let cursor = codec
            .decode_stream_cursor(&cursor, &request_context, 4)
            .expect("verify cursor");
        assert_eq!(cursor.after_position(), 91);
        assert_eq!(cursor.through_high_water(), 120);
    }

    fn context(project: &str, user: &str, user_epoch: u64) -> AuthenticatedReplicationContext {
        AuthenticatedReplicationContext::new(
            TenantScope::new(
                ProjectId::parse(project).expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            CollectionId::parse("todos").expect("collection"),
            AppUserId::parse(user).expect("user"),
            SessionId::parse("ses_abcdefgh").expect("session"),
            "member",
            AccessAuthorizationEpochs {
                environment: 2,
                user: user_epoch,
            },
            "req_abcdefgh",
        )
        .expect("context")
    }
}
