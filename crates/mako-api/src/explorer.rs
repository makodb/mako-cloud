use std::{collections::BTreeSet, error::Error, fmt};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::{CollectionId, TenantScope};

pub const EXPLORER_CAPABILITY_ISSUER: &str = "mako-control-plane";
pub const EXPLORER_CAPABILITY_AUDIENCE: &str = "mako-data-plane-explorer";
const TOKEN_VERSION: &str = "mx1";
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_GRANT_LIFETIME_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerAccessMode {
    PolicyPreview,
    Administrative,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerOperation {
    Get,
    Browse,
    Query,
    Plan,
    History,
    Simulate,
    Mutate,
    Import,
    Export,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExplorerCapabilityClaims {
    pub issuer: String,
    pub audience: String,
    pub developer_identity_id: String,
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub mode: ExplorerAccessMode,
    pub operations: BTreeSet<ExplorerOperation>,
    pub reason_hash: Option<String>,
    pub application_user_id: Option<String>,
    pub nonce: String,
    pub authorization_epoch: u64,
    pub issued_at_unix_seconds: u64,
    pub expires_at_unix_seconds: u64,
    pub key_id: String,
}

impl ExplorerCapabilityClaims {
    pub fn validate_shape(&self) -> Result<(), ExplorerCapabilityError> {
        if self.issuer != EXPLORER_CAPABILITY_ISSUER
            || self.audience != EXPLORER_CAPABILITY_AUDIENCE
        {
            return Err(ExplorerCapabilityError::WrongAuthority);
        }
        if self.developer_identity_id.is_empty()
            || self.developer_identity_id.len() > 128
            || self.nonce.len() < 16
            || self.nonce.len() > 128
            || self.key_id.is_empty()
            || self.key_id.len() > 128
            || self.authorization_epoch == 0
            || self.operations.is_empty()
            || self.expires_at_unix_seconds <= self.issued_at_unix_seconds
            || self.expires_at_unix_seconds - self.issued_at_unix_seconds
                > MAX_GRANT_LIFETIME_SECONDS
        {
            return Err(ExplorerCapabilityError::InvalidClaims);
        }
        match self.mode {
            ExplorerAccessMode::PolicyPreview => {
                if self
                    .application_user_id
                    .as_deref()
                    .is_none_or(str::is_empty)
                    || self.reason_hash.is_some()
                    || self.operations.iter().any(|operation| {
                        !matches!(
                            operation,
                            ExplorerOperation::Get
                                | ExplorerOperation::Browse
                                | ExplorerOperation::Query
                                | ExplorerOperation::Plan
                                | ExplorerOperation::Simulate
                        )
                    })
                {
                    return Err(ExplorerCapabilityError::ModeConfusion);
                }
            }
            ExplorerAccessMode::Administrative => {
                if self.application_user_id.is_some()
                    || self.reason_hash.as_deref().is_none_or(str::is_empty)
                {
                    return Err(ExplorerCapabilityError::ModeConfusion);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ExplorerCapabilityKey {
    key_id: String,
    secret: [u8; 32],
}

impl fmt::Debug for ExplorerCapabilityKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplorerCapabilityKey")
            .field("key_id", &self.key_id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl ExplorerCapabilityKey {
    pub fn new(key_id: impl Into<String>, secret: &[u8]) -> Result<Self, ExplorerCapabilityError> {
        let key_id = key_id.into();
        if key_id.is_empty() || key_id.len() > 128 || secret.len() < 32 {
            return Err(ExplorerCapabilityError::InvalidKey);
        }
        let mut material = [0_u8; 32];
        material.copy_from_slice(blake3::hash(secret).as_bytes());
        Ok(Self {
            key_id,
            secret: material,
        })
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[derive(Clone, Debug)]
pub struct ExplorerCapabilityKeyRing {
    current: ExplorerCapabilityKey,
    previous: Vec<ExplorerCapabilityKey>,
}

impl ExplorerCapabilityKeyRing {
    pub fn new(
        current: ExplorerCapabilityKey,
        previous: Vec<ExplorerCapabilityKey>,
    ) -> Result<Self, ExplorerCapabilityError> {
        if previous.len() > 2 || previous.iter().any(|key| key.key_id == current.key_id) {
            return Err(ExplorerCapabilityError::InvalidKey);
        }
        Ok(Self { current, previous })
    }

    pub fn issue(
        &self,
        mut claims: ExplorerCapabilityClaims,
    ) -> Result<ExplorerCapabilityToken, ExplorerCapabilityError> {
        claims.key_id.clone_from(&self.current.key_id);
        claims.validate_shape()?;
        let payload =
            serde_json::to_vec(&claims).map_err(|_| ExplorerCapabilityError::InvalidClaims)?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{TOKEN_VERSION}.{}.{payload}", self.current.key_id);
        let signature = blake3::keyed_hash(&self.current.secret, signing_input.as_bytes());
        Ok(ExplorerCapabilityToken(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_bytes())
        )))
    }

    pub fn validate(
        &self,
        token: &ExplorerCapabilityToken,
        expected: &ExplorerCapabilityValidation<'_>,
    ) -> Result<ExplorerCapabilityClaims, ExplorerCapabilityError> {
        let claims = self.verify(token, expected.now_unix_seconds)?;
        if claims.developer_identity_id != expected.developer_identity_id
            || &claims.tenant != expected.tenant
            || &claims.collection_id != expected.collection_id
            || claims.mode != expected.mode
            || claims.authorization_epoch != expected.authorization_epoch
            || !claims.operations.contains(&expected.operation)
        {
            return Err(ExplorerCapabilityError::ScopeMismatch);
        }
        if (expected.nonce_is_revoked)(&claims.nonce) {
            return Err(ExplorerCapabilityError::Revoked);
        }
        Ok(claims)
    }

    pub fn verify(
        &self,
        token: &ExplorerCapabilityToken,
        now_unix_seconds: u64,
    ) -> Result<ExplorerCapabilityClaims, ExplorerCapabilityError> {
        if token.0.len() > MAX_TOKEN_BYTES {
            return Err(ExplorerCapabilityError::Malformed);
        }
        let mut parts = token.0.split('.');
        let (Some(version), Some(key_id), Some(payload), Some(signature), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(ExplorerCapabilityError::Malformed);
        };
        if version != TOKEN_VERSION {
            return Err(ExplorerCapabilityError::Malformed);
        }
        let key = std::iter::once(&self.current)
            .chain(self.previous.iter())
            .find(|key| key.key_id == key_id)
            .ok_or(ExplorerCapabilityError::UnknownKey)?;
        let decoded_signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| ExplorerCapabilityError::Malformed)?;
        let expected_signature = blake3::keyed_hash(
            &key.secret,
            format!("{version}.{key_id}.{payload}").as_bytes(),
        );
        if !constant_time_eq(&decoded_signature, expected_signature.as_bytes()) {
            return Err(ExplorerCapabilityError::InvalidSignature);
        }
        let claims: ExplorerCapabilityClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| ExplorerCapabilityError::Malformed)?,
        )
        .map_err(|_| ExplorerCapabilityError::Malformed)?;
        claims.validate_shape()?;
        if claims.key_id != key_id {
            return Err(ExplorerCapabilityError::InvalidSignature);
        }
        if now_unix_seconds < claims.issued_at_unix_seconds.saturating_sub(30)
            || now_unix_seconds >= claims.expires_at_unix_seconds
        {
            return Err(ExplorerCapabilityError::Expired);
        }
        Ok(claims)
    }
}

pub struct ExplorerCapabilityValidation<'a> {
    pub developer_identity_id: &'a str,
    pub tenant: &'a TenantScope,
    pub collection_id: &'a CollectionId,
    pub mode: ExplorerAccessMode,
    pub operation: ExplorerOperation,
    pub authorization_epoch: u64,
    pub now_unix_seconds: u64,
    pub nonce_is_revoked: &'a dyn Fn(&str) -> bool,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ExplorerCapabilityToken(String);

impl ExplorerCapabilityToken {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExplorerCapabilityError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_TOKEN_BYTES || value.contains(char::is_whitespace)
        {
            return Err(ExplorerCapabilityError::Malformed);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_for_authorization_header(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExplorerCapabilityToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExplorerCapabilityToken([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExplorerCapabilityError {
    InvalidKey,
    InvalidClaims,
    ModeConfusion,
    Malformed,
    UnknownKey,
    InvalidSignature,
    WrongAuthority,
    Expired,
    ScopeMismatch,
    Revoked,
}

impl fmt::Display for ExplorerCapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("explorer capability was not accepted")
    }
}

impl Error for ExplorerCapabilityError {}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentId, ProjectId};

    fn claims(now: u64) -> ExplorerCapabilityClaims {
        ExplorerCapabilityClaims {
            issuer: EXPLORER_CAPABILITY_ISSUER.to_owned(),
            audience: EXPLORER_CAPABILITY_AUDIENCE.to_owned(),
            developer_identity_id: "dev_abcdefgh".to_owned(),
            tenant: TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            collection_id: CollectionId::parse("messages").expect("collection"),
            mode: ExplorerAccessMode::Administrative,
            operations: BTreeSet::from([ExplorerOperation::Get, ExplorerOperation::Mutate]),
            reason_hash: Some(blake3::hash(b"support case 42").to_hex().to_string()),
            application_user_id: None,
            nonce: "nonce_abcdefghijklmnop".to_owned(),
            authorization_epoch: 7,
            issued_at_unix_seconds: now,
            expires_at_unix_seconds: now + 120,
            key_id: String::new(),
        }
    }

    fn ring() -> ExplorerCapabilityKeyRing {
        ExplorerCapabilityKeyRing::new(
            ExplorerCapabilityKey::new("current", b"01234567890123456789012345678901")
                .expect("key"),
            vec![
                ExplorerCapabilityKey::new("previous", b"abcdefghijklmnopqrstuvwxyzABCDEF")
                    .expect("key"),
            ],
        )
        .expect("ring")
    }

    fn validation<'a>(
        claims: &'a ExplorerCapabilityClaims,
        operation: ExplorerOperation,
        now: u64,
    ) -> ExplorerCapabilityValidation<'a> {
        ExplorerCapabilityValidation {
            developer_identity_id: &claims.developer_identity_id,
            tenant: &claims.tenant,
            collection_id: &claims.collection_id,
            mode: claims.mode,
            operation,
            authorization_epoch: claims.authorization_epoch,
            now_unix_seconds: now,
            nonce_is_revoked: &|_| false,
        }
    }

    #[test]
    fn validates_a_narrow_capability_without_exposing_it_in_debug() {
        let source = claims(1_000);
        let token = ring().issue(source.clone()).expect("issue");
        assert_eq!(format!("{token:?}"), "ExplorerCapabilityToken([REDACTED])");
        let accepted = ring()
            .validate(&token, &validation(&source, ExplorerOperation::Get, 1_030))
            .expect("validate");
        assert_eq!(accepted.collection_id, source.collection_id);
    }

    #[test]
    fn rejects_tampering_expiry_epoch_scope_operation_and_revocation() {
        let source = claims(1_000);
        let token = ring().issue(source.clone()).expect("issue");
        let tampered =
            ExplorerCapabilityToken::parse(format!("{}x", token.expose_for_authorization_header()))
                .expect("parse");
        assert_eq!(
            ring().validate(
                &tampered,
                &validation(&source, ExplorerOperation::Get, 1_030)
            ),
            Err(ExplorerCapabilityError::InvalidSignature)
        );
        assert_eq!(
            ring().validate(&token, &validation(&source, ExplorerOperation::Get, 1_120)),
            Err(ExplorerCapabilityError::Expired)
        );
        let mut wrong = claims(1_000);
        wrong.authorization_epoch += 1;
        assert_eq!(
            ring().validate(&token, &validation(&wrong, ExplorerOperation::Get, 1_030)),
            Err(ExplorerCapabilityError::ScopeMismatch)
        );
        assert_eq!(
            ring().validate(
                &token,
                &validation(&source, ExplorerOperation::Export, 1_030)
            ),
            Err(ExplorerCapabilityError::ScopeMismatch)
        );
        let revoked = ExplorerCapabilityValidation {
            nonce_is_revoked: &|nonce| nonce == "nonce_abcdefghijklmnop",
            ..validation(&source, ExplorerOperation::Get, 1_030)
        };
        assert_eq!(
            ring().validate(&token, &revoked),
            Err(ExplorerCapabilityError::Revoked)
        );
    }

    #[test]
    fn previous_rotation_key_remains_valid_during_overlap() {
        let source = claims(1_000);
        let previous_ring = ExplorerCapabilityKeyRing::new(
            ExplorerCapabilityKey::new("previous", b"abcdefghijklmnopqrstuvwxyzABCDEF")
                .expect("key"),
            Vec::new(),
        )
        .expect("ring");
        let token = previous_ring.issue(source.clone()).expect("issue");
        ring()
            .validate(&token, &validation(&source, ExplorerOperation::Get, 1_030))
            .expect("rotation overlap");
    }

    #[test]
    fn preview_mode_cannot_gain_mutation_or_admin_reason() {
        let mut source = claims(1_000);
        source.mode = ExplorerAccessMode::PolicyPreview;
        source.application_user_id = Some("usr_abcdefgh".to_owned());
        source.reason_hash = None;
        assert_eq!(
            ring().issue(source.clone()),
            Err(ExplorerCapabilityError::ModeConfusion)
        );
        source.operations = BTreeSet::from([ExplorerOperation::Get, ExplorerOperation::Simulate]);
        ring().issue(source).expect("bounded preview");
    }
}
