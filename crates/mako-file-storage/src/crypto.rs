use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use mako_api::TenantScope;
use rand_core::{OsRng, RngCore};

use crate::FileStorageError;

const NONCE_BYTES: usize = 24;
const TENANT_KEY_CONTEXT: &str = "mako/file-storage/object-key/v1";

/// The data plane's root for object keys; every tenant's key is derived from it.
#[derive(Clone)]
pub struct ObjectKeyRoot([u8; 32]);

impl ObjectKeyRoot {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for ObjectKeyRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ObjectKeyRoot([REDACTED])")
    }
}

/// Seals and opens one tenant's objects. The key is derived from the root and
/// the tenant, so bytes sealed for one environment never open for another.
#[derive(Clone)]
pub struct ObjectCipher {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for ObjectCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ObjectCipher([REDACTED])")
    }
}

impl ObjectCipher {
    #[must_use]
    pub fn for_tenant(root: &ObjectKeyRoot, tenant: &TenantScope) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(TENANT_KEY_CONTEXT);
        hasher.update(&root.0);
        hasher.update(&[0]);
        hasher.update(tenant.project_id().as_str().as_bytes());
        hasher.update(&[0]);
        hasher.update(tenant.environment_id().as_str().as_bytes());
        let key: [u8; 32] = *hasher.finalize().as_bytes();
        Self {
            cipher: XChaCha20Poly1305::new((&key).into()),
        }
    }

    /// Nonce followed by ciphertext; `aad` binds the bytes to their location.
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, FileStorageError> {
        let mut nonce = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let sealed = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| FileStorageError::Corrupt)?;
        let mut output = Vec::with_capacity(NONCE_BYTES + sealed.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&sealed);
        Ok(output)
    }

    pub fn open(&self, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, FileStorageError> {
        if sealed.len() <= NONCE_BYTES {
            return Err(FileStorageError::Corrupt);
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_BYTES);
        self.cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| FileStorageError::Corrupt)
    }
}
