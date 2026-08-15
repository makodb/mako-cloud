use std::{error::Error, fmt};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use ed25519_dalek::SigningKey;
use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq)]
pub struct KeyEncryptionKey([u8; 32]);

impl KeyEncryptionKey {
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

    pub(crate) fn encrypt_secret(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<(Vec<u8>, [u8; 24]), SigningKeyError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let cipher =
            XChaCha20Poly1305::new_from_slice(&self.0).map_err(|_| SigningKeyError::Encryption)?;
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| SigningKeyError::Encryption)?;
        Ok((encrypted, nonce))
    }

    pub(crate) fn decrypt_secret(
        &self,
        encrypted: &[u8],
        nonce: &[u8; 24],
        aad: &[u8],
    ) -> Result<Vec<u8>, SigningKeyError> {
        XChaCha20Poly1305::new_from_slice(&self.0)
            .map_err(|_| SigningKeyError::Decryption)?
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: encrypted,
                    aad,
                },
            )
            .map_err(|_| SigningKeyError::Decryption)
    }
}

impl fmt::Debug for KeyEncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyEncryptionKey([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SigningKeyState {
    Active,
    Retiring,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectSigningKeyRecord {
    tenant: TenantScope,
    key_id: String,
    state: SigningKeyState,
    public_key: [u8; 32],
    encrypted_private_key: Vec<u8>,
    encryption_nonce: [u8; 24],
    created_at_unix_seconds: u64,
    retire_at_unix_seconds: Option<u64>,
}

impl ProjectSigningKeyRecord {
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    #[must_use]
    pub const fn state(&self) -> SigningKeyState {
        self.state
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn retire_at_unix_seconds(&self) -> Option<u64> {
        self.retire_at_unix_seconds
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JsonWebKeySet {
    pub keys: Vec<JsonWebKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JsonWebKey {
    pub kty: &'static str,
    pub crv: &'static str,
    #[serde(rename = "use")]
    pub key_use: &'static str,
    pub alg: &'static str,
    pub kid: String,
    pub x: String,
}

#[derive(Clone, Debug)]
pub struct ProjectSigningKeyRing {
    tenant: TenantScope,
    records: Vec<ProjectSigningKeyRecord>,
}

impl ProjectSigningKeyRing {
    #[must_use]
    pub fn new(tenant: TenantScope) -> Self {
        Self {
            tenant,
            records: Vec::new(),
        }
    }

    pub fn from_records(
        tenant: TenantScope,
        records: Vec<ProjectSigningKeyRecord>,
    ) -> Result<Self, SigningKeyError> {
        if records.iter().any(|record| record.tenant != tenant)
            || records
                .iter()
                .filter(|record| record.state == SigningKeyState::Active)
                .count()
                > 1
        {
            return Err(SigningKeyError::CorruptKeyRing);
        }
        Ok(Self { tenant, records })
    }

    pub fn create_initial(
        &mut self,
        encryption_key: &KeyEncryptionKey,
        now_unix_seconds: u64,
    ) -> Result<&ProjectSigningKeyRecord, SigningKeyError> {
        if !self.records.is_empty() {
            return Err(SigningKeyError::ActiveKeyAlreadyExists);
        }
        self.records.push(generate_record(
            &self.tenant,
            encryption_key,
            now_unix_seconds,
        )?);
        Ok(self.records.last().expect("record was inserted"))
    }

    pub fn rotate(
        &mut self,
        encryption_key: &KeyEncryptionKey,
        now_unix_seconds: u64,
        overlap_seconds: u64,
    ) -> Result<&ProjectSigningKeyRecord, SigningKeyError> {
        if overlap_seconds == 0 {
            return Err(SigningKeyError::InvalidOverlap);
        }
        let retire_at = now_unix_seconds
            .checked_add(overlap_seconds)
            .ok_or(SigningKeyError::TimeOverflow)?;
        let active = self
            .records
            .iter_mut()
            .find(|record| record.state == SigningKeyState::Active)
            .ok_or(SigningKeyError::NoActiveKey)?;
        active.state = SigningKeyState::Retiring;
        active.retire_at_unix_seconds = Some(retire_at);
        self.records.push(generate_record(
            &self.tenant,
            encryption_key,
            now_unix_seconds,
        )?);
        Ok(self.records.last().expect("record was inserted"))
    }

    pub fn retire_due(&mut self, now_unix_seconds: u64) -> usize {
        let mut retired = 0;
        for record in &mut self.records {
            if record.state == SigningKeyState::Retiring
                && record
                    .retire_at_unix_seconds
                    .is_some_and(|deadline| now_unix_seconds >= deadline)
            {
                record.state = SigningKeyState::Retired;
                retired += 1;
            }
        }
        retired
    }

    pub fn active_signing_key(
        &self,
        encryption_key: &KeyEncryptionKey,
    ) -> Result<(String, SigningKey), SigningKeyError> {
        let record = self
            .records
            .iter()
            .find(|record| record.state == SigningKeyState::Active)
            .ok_or(SigningKeyError::NoActiveKey)?;
        Ok((
            record.key_id.clone(),
            decrypt_signing_key(record, encryption_key)?,
        ))
    }

    #[must_use]
    pub fn jwks(&self) -> JsonWebKeySet {
        let mut keys = self
            .records
            .iter()
            .filter(|record| record.state != SigningKeyState::Retired)
            .map(|record| JsonWebKey {
                kty: "OKP",
                crv: "Ed25519",
                key_use: "sig",
                alg: "EdDSA",
                kid: record.key_id.clone(),
                x: URL_SAFE_NO_PAD.encode(record.public_key),
            })
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| left.kid.cmp(&right.kid));
        JsonWebKeySet { keys }
    }

    #[must_use]
    pub fn records(&self) -> &[ProjectSigningKeyRecord] {
        &self.records
    }
}

fn generate_record(
    tenant: &TenantScope,
    encryption_key: &KeyEncryptionKey,
    now_unix_seconds: u64,
) -> Result<ProjectSigningKeyRecord, SigningKeyError> {
    let mut secret = [0_u8; 32];
    OsRng.fill_bytes(&mut secret);
    let signing_key = SigningKey::from_bytes(&secret);
    let public_key = signing_key.verifying_key().to_bytes();
    let key_id = format!("sig_{}", &URL_SAFE_NO_PAD.encode(public_key)[..22]);
    let aad = key_aad(tenant, &key_id);
    let (encrypted_private_key, nonce) = encryption_key.encrypt_secret(&secret, &aad)?;
    secret.fill(0);
    Ok(ProjectSigningKeyRecord {
        tenant: tenant.clone(),
        key_id,
        state: SigningKeyState::Active,
        public_key,
        encrypted_private_key,
        encryption_nonce: nonce,
        created_at_unix_seconds: now_unix_seconds,
        retire_at_unix_seconds: None,
    })
}

fn decrypt_signing_key(
    record: &ProjectSigningKeyRecord,
    encryption_key: &KeyEncryptionKey,
) -> Result<SigningKey, SigningKeyError> {
    let plaintext = encryption_key.decrypt_secret(
        &record.encrypted_private_key,
        &record.encryption_nonce,
        &key_aad(&record.tenant, &record.key_id),
    )?;
    let secret: [u8; 32] = plaintext
        .try_into()
        .map_err(|_| SigningKeyError::Decryption)?;
    let key = SigningKey::from_bytes(&secret);
    if key.verifying_key().to_bytes() != record.public_key {
        return Err(SigningKeyError::Decryption);
    }
    Ok(key)
}

fn key_aad(tenant: &TenantScope, key_id: &str) -> Vec<u8> {
    format!(
        "mako-signing-key\0{}\0{}\0{}",
        tenant.project_id().as_str(),
        tenant.environment_id().as_str(),
        key_id
    )
    .into_bytes()
}

#[derive(Debug, Eq, PartialEq)]
pub enum SigningKeyError {
    ActiveKeyAlreadyExists,
    NoActiveKey,
    InvalidOverlap,
    TimeOverflow,
    Encryption,
    Decryption,
    CorruptKeyRing,
}

impl fmt::Display for SigningKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ActiveKeyAlreadyExists => "an active signing key already exists",
            Self::NoActiveKey => "no active signing key exists",
            Self::InvalidOverlap => "signing-key overlap must be positive",
            Self::TimeOverflow => "signing-key retirement time overflowed",
            Self::Encryption => "private signing-key encryption failed",
            Self::Decryption => "private signing-key decryption failed",
            Self::CorruptKeyRing => "project signing-key ring is corrupt",
        })
    }
}

impl Error for SigningKeyError {}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, Verifier};
    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[test]
    fn encrypted_keys_rotate_with_jwks_overlap_and_retirement() {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        let encryption_key = KeyEncryptionKey::generate();
        let mut ring = ProjectSigningKeyRing::new(tenant);
        let first_id = ring
            .create_initial(&encryption_key, 10)
            .expect("initial")
            .key_id()
            .to_owned();
        let (active_id, signing_key) = ring
            .active_signing_key(&encryption_key)
            .expect("active key");
        assert_eq!(active_id, first_id);
        let signature = signing_key.sign(b"payload");
        signing_key
            .verifying_key()
            .verify(b"payload", &signature)
            .expect("signature");
        assert_eq!(ring.jwks().keys.len(), 1);

        let second_id = ring
            .rotate(&encryption_key, 20, 30)
            .expect("rotate")
            .key_id()
            .to_owned();
        assert_ne!(first_id, second_id);
        assert_eq!(ring.jwks().keys.len(), 2);
        assert_eq!(ring.retire_due(49), 0);
        assert_eq!(ring.retire_due(50), 1);
        assert_eq!(ring.jwks().keys.len(), 1);
        assert_eq!(ring.jwks().keys[0].kid, second_id);
        assert!(!format!("{ring:?}").contains(&URL_SAFE_NO_PAD.encode(signing_key.to_bytes())));

        let wrong_key = KeyEncryptionKey::generate();
        assert_eq!(
            ring.active_signing_key(&wrong_key).expect_err("wrong KEK"),
            SigningKeyError::Decryption
        );
    }
}
