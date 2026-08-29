//! Tenant-scoped immutable object storage boundary.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use mako_api::TenantScope;
use sha2::{Digest, Sha256};

mod s3;

pub use s3::{S3Credentials, S3ObjectStore, S3ObjectStoreConfig};

pub const MAX_DATA_JOB_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = MAX_DATA_JOB_ARTIFACT_BYTES;

/// An object address that retains the trusted tenant independently of its path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectAddress {
    tenant: TenantScope,
    path: String,
    digest: [u8; 32],
}

impl ObjectAddress {
    pub fn function_bundle(tenant: TenantScope, digest: &str) -> Result<Self, ObjectStoreError> {
        if !valid_digest(digest) {
            return Err(ObjectStoreError::InvalidAddress);
        }
        let digest_bytes = decode_digest(digest).ok_or(ObjectStoreError::InvalidAddress)?;
        let path = format!(
            "projects/{}/environments/{}/function-bundles/{digest}.bundle",
            tenant.project_id(),
            tenant.environment_id(),
        );
        Ok(Self {
            tenant,
            path,
            digest: digest_bytes,
        })
    }

    /// An application object: bytes an application stored under a bucket, addressed by
    /// the digest of what is actually stored (ciphertext, when the caller encrypts) so
    /// the store's integrity check holds. Metadata keeps the logical path; the address
    /// changes whenever the content does, which is what makes an immutable store serve
    /// a mutable path.
    pub fn application_object(
        tenant: TenantScope,
        bucket: &str,
        digest: &str,
    ) -> Result<Self, ObjectStoreError> {
        if !valid_digest(digest) || !valid_bucket_name(bucket) {
            return Err(ObjectStoreError::InvalidAddress);
        }
        let digest_bytes = decode_digest(digest).ok_or(ObjectStoreError::InvalidAddress)?;
        let path = format!(
            "projects/{}/environments/{}/buckets/{bucket}/objects/{digest}.blob",
            tenant.project_id(),
            tenant.environment_id(),
        );
        Ok(Self {
            tenant,
            path,
            digest: digest_bytes,
        })
    }

    pub fn data_job_artifact(
        tenant: TenantScope,
        job_id: &str,
        kind: DataJobArtifactKind,
        digest: &str,
    ) -> Result<Self, ObjectStoreError> {
        if !valid_digest(digest)
            || !job_id.starts_with("djob_")
            || !(13..=80).contains(&job_id.len())
            || !job_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ObjectStoreError::InvalidAddress);
        }
        let digest_bytes = decode_digest(digest).ok_or(ObjectStoreError::InvalidAddress)?;
        let suffix = match kind {
            DataJobArtifactKind::ImportUpload => "import-upload.jsonl",
            DataJobArtifactKind::ExportOutput => "export-output.jsonl",
            DataJobArtifactKind::Manifest => "manifest.json",
        };
        let path = format!(
            "projects/{}/environments/{}/data-jobs/{job_id}/{digest}/{suffix}",
            tenant.project_id(),
            tenant.environment_id(),
        );
        Ok(Self {
            tenant,
            path,
            digest: digest_bytes,
        })
    }

    #[must_use]
    pub const fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn require_tenant(&self, tenant: &TenantScope) -> Result<(), ObjectStoreError> {
        if self.tenant == *tenant {
            Ok(())
        } else {
            Err(ObjectStoreError::TenantMismatch)
        }
    }

    pub(crate) fn content_matches(&self, bytes: &[u8]) -> bool {
        Sha256::digest(bytes).as_slice() == self.digest
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataJobArtifactKind {
    ImportUpload,
    ExportOutput,
    Manifest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PutImmutableOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectStoreError {
    InvalidConfiguration,
    InvalidAddress,
    InvalidObject,
    TenantMismatch,
    ImmutableConflict,
    Integrity,
    Unavailable,
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "object storage configuration is invalid",
            Self::InvalidAddress => "object address is invalid",
            Self::InvalidObject => "object content is invalid",
            Self::TenantMismatch => "object tenant does not match the trusted tenant",
            Self::ImmutableConflict => "immutable object already has different content",
            Self::Integrity => "object storage content failed integrity verification",
            Self::Unavailable => "object storage is unavailable",
        })
    }
}

impl Error for ObjectStoreError {}

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put_immutable(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
        bytes: Arc<[u8]>,
    ) -> Result<PutImmutableOutcome, ObjectStoreError>;

    async fn get(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<Option<Arc<[u8]>>, ObjectStoreError>;

    /// Remove the exact tenant-bound immutable object. Missing objects are an
    /// idempotent success so retention cleanup can safely resume after a crash.
    async fn delete(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<(), ObjectStoreError>;
}

/// Deterministic reference store for development and boundary qualification.
#[derive(Clone, Default)]
pub struct MemoryObjectStore {
    objects: Arc<Mutex<BTreeMap<String, Arc<[u8]>>>>,
}

impl MemoryObjectStore {
    /// Every stored path with its bytes; for tests that assert on what the
    /// store holds without going through an address.
    #[must_use]
    pub fn dump(&self) -> BTreeMap<String, Vec<u8>> {
        self.objects
            .lock()
            .expect("object store lock")
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.to_vec()))
            .collect()
    }
}

impl fmt::Debug for MemoryObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryObjectStore")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ObjectStore for MemoryObjectStore {
    async fn put_immutable(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
        bytes: Arc<[u8]>,
    ) -> Result<PutImmutableOutcome, ObjectStoreError> {
        address.require_tenant(tenant)?;
        if bytes.is_empty() || bytes.len() > MAX_OBJECT_BYTES {
            return Err(ObjectStoreError::InvalidObject);
        }
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| ObjectStoreError::Unavailable)?;
        match objects.get(address.path()) {
            None => {
                if !address.content_matches(&bytes) {
                    return Err(ObjectStoreError::InvalidObject);
                }
                objects.insert(address.path().to_owned(), bytes);
                Ok(PutImmutableOutcome::Created)
            }
            Some(existing) if existing.as_ref() == bytes.as_ref() => {
                Ok(PutImmutableOutcome::AlreadyPresent)
            }
            Some(_) => Err(ObjectStoreError::ImmutableConflict),
        }
    }

    async fn get(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<Option<Arc<[u8]>>, ObjectStoreError> {
        address.require_tenant(tenant)?;
        self.objects
            .lock()
            .map_err(|_| ObjectStoreError::Unavailable)
            .map(|objects| objects.get(address.path()).cloned())
    }

    async fn delete(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<(), ObjectStoreError> {
        address.require_tenant(tenant)?;
        self.objects
            .lock()
            .map_err(|_| ObjectStoreError::Unavailable)?
            .remove(address.path());
        Ok(())
    }
}

/// Bucket names are lowercase, start with a letter, and use only letters, digits,
/// and single hyphens: safe in an object key, a URL, and a header without escaping.
#[must_use]
pub fn valid_bucket_name(bucket: &str) -> bool {
    (2..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && bucket
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !bucket.ends_with('-')
        && !bucket.contains("--")
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn decode_digest(value: &str) -> Option<[u8; 32]> {
    if !valid_digest(value) {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, chunk) in value.as_bytes()[7..].chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Some(decoded)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use proptest::prelude::*;
    use sha2::{Digest, Sha256};

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn arbitrary_tenant_pairs_never_cross_object_namespaces(
            project_a in "[a-z0-9_-]{8,24}",
            environment_a in "[a-z0-9_-]{8,24}",
            project_b in "[a-z0-9_-]{8,24}",
            environment_b in "[a-z0-9_-]{8,24}",
            digest_byte in any::<u8>(),
        ) {
            prop_assume!(project_a != project_b || environment_a != environment_b);
            let tenant_a = tenant(&project_a, &environment_a);
            let tenant_b = tenant(&project_b, &environment_b);
            let digest = digest_for(&[digest_byte]);
            let address = ObjectAddress::function_bundle(tenant_a.clone(), &digest).expect("address");
            let expected_prefix = format!(
                "projects/{}/environments/{}/",
                tenant_a.project_id(),
                tenant_a.environment_id(),
            );
            prop_assert!(address.path().starts_with(&expected_prefix));
            let store = MemoryObjectStore::default();
            block_on(async {
                store
                    .put_immutable(&tenant_a, &address, Arc::from([digest_byte]))
                    .await
                    .expect("owner write");
                prop_assert_eq!(
                    store.get(&tenant_b, &address).await,
                    Err(ObjectStoreError::TenantMismatch),
                );
                Ok(())
            })?;
        }

        #[test]
        fn arbitrary_invalid_digest_text_never_builds_an_object_address(value in ".{0,100}") {
            let tenant = tenant("abcdefgh", "abcdefgh");
            let valid = valid_digest(&value);
            prop_assert_eq!(ObjectAddress::function_bundle(tenant, &value).is_ok(), valid);
        }
    }

    #[test]
    fn immutable_retries_succeed_but_changed_content_conflicts() {
        block_on(async {
            let tenant = tenant("abcdefgh", "abcdefgh");
            let digest = digest_for(b"first");
            let address = ObjectAddress::function_bundle(tenant.clone(), &digest).expect("address");
            let store = MemoryObjectStore::default();
            assert_eq!(
                store
                    .put_immutable(&tenant, &address, Arc::from(b"first".as_slice()))
                    .await,
                Ok(PutImmutableOutcome::Created),
            );
            assert_eq!(
                store
                    .put_immutable(&tenant, &address, Arc::from(b"first".as_slice()))
                    .await,
                Ok(PutImmutableOutcome::AlreadyPresent),
            );
            assert_eq!(
                store
                    .put_immutable(&tenant, &address, Arc::from(b"changed".as_slice()))
                    .await,
                Err(ObjectStoreError::ImmutableConflict),
            );
        });
    }

    #[test]
    fn deletion_is_tenant_bound_and_idempotent() {
        block_on(async {
            let owner = tenant("abcdefgh", "abcdefgh");
            let other = tenant("ijklmnop", "ijklmnop");
            let digest = digest_for(b"retained artifact");
            let address = ObjectAddress::data_job_artifact(
                owner.clone(),
                "djob_abcdefgh",
                DataJobArtifactKind::ExportOutput,
                &digest,
            )
            .expect("address");
            let store = MemoryObjectStore::default();
            store
                .put_immutable(&owner, &address, Arc::from(b"retained artifact".as_slice()))
                .await
                .expect("write");
            assert_eq!(
                store.delete(&other, &address).await,
                Err(ObjectStoreError::TenantMismatch)
            );
            store.delete(&owner, &address).await.expect("delete");
            store.delete(&owner, &address).await.expect("repeat delete");
            assert_eq!(store.get(&owner, &address).await.expect("get"), None);
        });
    }

    fn tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(format!("prj_{project}")).expect("project"),
            EnvironmentId::parse(format!("env_{environment}")).expect("environment"),
        )
    }

    fn digest_for(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
}
