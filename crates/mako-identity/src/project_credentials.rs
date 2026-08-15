use std::{collections::BTreeSet, fmt, num::NonZeroUsize};

use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, ConditionalPut, KeyCondition, ScanDirection, ScanRequest,
    WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{CredentialDigest, IdentityRecordError, IdentityStore, IdentityStoreError};

const MAX_CREDENTIAL_OVERLAP_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectCredentialId(String);

impl ProjectCredentialId {
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentityRecordError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(IdentityRecordError::InvalidField {
                field: "project credential id",
                reason: "must contain 1-128 ASCII letters, digits, hyphens, or underscores",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectCredentialKind {
    Public,
    Service,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCredentialOperation {
    Create,
    Read,
    Update,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ServiceCredentialScope {
    collections: BTreeSet<String>,
    operations: BTreeSet<ServiceCredentialOperation>,
}

impl ServiceCredentialScope {
    pub fn new(
        collections: impl IntoIterator<Item = String>,
        operations: impl IntoIterator<Item = ServiceCredentialOperation>,
    ) -> Result<Self, IdentityRecordError> {
        let collections = collections.into_iter().collect::<BTreeSet<_>>();
        let operations = operations.into_iter().collect::<BTreeSet<_>>();
        if collections.is_empty() || collections.len() > 64 {
            return Err(IdentityRecordError::InvalidField {
                field: "service credential collections",
                reason: "must contain 1-64 collection identifiers",
            });
        }
        if collections.iter().any(|collection| {
            collection.is_empty()
                || collection.len() > 128
                || !collection
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) {
            return Err(IdentityRecordError::InvalidField {
                field: "service credential collection",
                reason: "must be a bounded collection identifier",
            });
        }
        if operations.is_empty() {
            return Err(IdentityRecordError::InvalidField {
                field: "service credential operations",
                reason: "must contain at least one operation",
            });
        }
        Ok(Self {
            collections,
            operations,
        })
    }

    #[must_use]
    pub fn permits(&self, collection: &str, operation: ServiceCredentialOperation) -> bool {
        self.collections.contains(collection) && self.operations.contains(&operation)
    }

    #[must_use]
    pub fn collections(&self) -> &BTreeSet<String> {
        &self.collections
    }

    #[must_use]
    pub fn operations(&self) -> &BTreeSet<ServiceCredentialOperation> {
        &self.operations
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectCredentialState {
    Active,
    Overlap,
    Retired,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProjectCredential(String);

impl ProjectCredential {
    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProjectCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProjectCredential([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedProjectCredential {
    pub metadata: ProjectCredentialMetadata,
    pub credential: ProjectCredential,
}

pub struct PreparedProjectCredentialCreate {
    issued: IssuedProjectCredential,
    record: StoredProjectCredential,
    key: Vec<u8>,
}

impl PreparedProjectCredentialCreate {
    #[must_use]
    pub fn issued(&self) -> &IssuedProjectCredential {
        &self.issued
    }
}

impl fmt::Debug for PreparedProjectCredentialCreate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedProjectCredentialCreate")
            .field("credential", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

pub struct PreparedProjectCredentialRotation {
    issued: IssuedProjectCredential,
    current_key: Vec<u8>,
    current_bytes: Vec<u8>,
    current: StoredProjectCredential,
    replacement_key: Vec<u8>,
    replacement: StoredProjectCredential,
}

impl PreparedProjectCredentialRotation {
    #[must_use]
    pub fn issued(&self) -> &IssuedProjectCredential {
        &self.issued
    }
}

impl fmt::Debug for PreparedProjectCredentialRotation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedProjectCredentialRotation")
            .field("credential", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectCredentialMetadata {
    scope: TenantScope,
    id: ProjectCredentialId,
    kind: ProjectCredentialKind,
    service_scope: Option<ServiceCredentialScope>,
    state: ProjectCredentialState,
    created_at_unix_seconds: u64,
    overlap_ends_at_unix_seconds: Option<u64>,
    retired_at_unix_seconds: Option<u64>,
}

impl ProjectCredentialMetadata {
    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn id(&self) -> &ProjectCredentialId {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> ProjectCredentialKind {
        self.kind
    }

    #[must_use]
    pub fn service_scope(&self) -> Option<&ServiceCredentialScope> {
        self.service_scope.as_ref()
    }

    #[must_use]
    pub const fn state(&self) -> ProjectCredentialState {
        self.state
    }

    #[must_use]
    pub const fn overlap_ends_at_unix_seconds(&self) -> Option<u64> {
        self.overlap_ends_at_unix_seconds
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPublicCredential {
    scope: TenantScope,
    credential_id: ProjectCredentialId,
}

impl VerifiedPublicCredential {
    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn credential_id(&self) -> &ProjectCredentialId {
        &self.credential_id
    }

    #[must_use]
    pub const fn can_bypass_document_policies(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedServiceCredential {
    scope: TenantScope,
    credential_id: ProjectCredentialId,
    service_scope: ServiceCredentialScope,
}

impl VerifiedServiceCredential {
    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    #[must_use]
    pub fn credential_id(&self) -> &ProjectCredentialId {
        &self.credential_id
    }

    #[must_use]
    pub fn permits(&self, collection: &str, operation: ServiceCredentialOperation) -> bool {
        self.service_scope.permits(collection, operation)
    }

    #[must_use]
    pub const fn can_bypass_document_policies(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifiedProjectCredential {
    Public(VerifiedPublicCredential),
    Service(VerifiedServiceCredential),
}

impl VerifiedProjectCredential {
    #[must_use]
    pub const fn can_bypass_document_policies(&self) -> bool {
        match self {
            Self::Public(public) => public.can_bypass_document_policies(),
            Self::Service(service) => service.can_bypass_document_policies(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredProjectCredential {
    metadata: ProjectCredentialMetadata,
    digest: CredentialDigest,
}

impl IdentityStore {
    pub async fn create_public_project_key(
        &self,
        id: ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, IdentityStoreError> {
        let prepared = self.prepare_public_project_key(id, now_unix_seconds)?;
        self.commit_prepared_project_credential(prepared, None)
            .await
    }

    pub async fn create_service_credential(
        &self,
        id: ProjectCredentialId,
        scope: ServiceCredentialScope,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, IdentityStoreError> {
        let prepared = self.prepare_service_credential(id, scope, now_unix_seconds)?;
        self.commit_prepared_project_credential(prepared, None)
            .await
    }

    pub fn prepare_public_project_key(
        &self,
        id: ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<PreparedProjectCredentialCreate, IdentityStoreError> {
        self.prepare_project_credential(id, ProjectCredentialKind::Public, None, now_unix_seconds)
    }

    pub fn prepare_service_credential(
        &self,
        id: ProjectCredentialId,
        scope: ServiceCredentialScope,
        now_unix_seconds: u64,
    ) -> Result<PreparedProjectCredentialCreate, IdentityStoreError> {
        self.prepare_project_credential(
            id,
            ProjectCredentialKind::Service,
            Some(scope),
            now_unix_seconds,
        )
    }

    fn prepare_project_credential(
        &self,
        id: ProjectCredentialId,
        kind: ProjectCredentialKind,
        service_scope: Option<ServiceCredentialScope>,
        now_unix_seconds: u64,
    ) -> Result<PreparedProjectCredentialCreate, IdentityStoreError> {
        if (kind == ProjectCredentialKind::Public) != service_scope.is_none() {
            return Err(IdentityStoreError::InvalidProjectCredential);
        }
        let credential = generate_credential(kind, &id);
        let metadata = ProjectCredentialMetadata {
            scope: self.tenant.clone(),
            id: id.clone(),
            kind,
            service_scope,
            state: ProjectCredentialState::Active,
            created_at_unix_seconds: now_unix_seconds,
            overlap_ends_at_unix_seconds: None,
            retired_at_unix_seconds: None,
        };
        let record = StoredProjectCredential {
            metadata: metadata.clone(),
            digest: credential_digest(&credential)?,
        };
        let key = self.keyspace.project_credential_key(id.as_str())?;
        Ok(PreparedProjectCredentialCreate {
            issued: IssuedProjectCredential {
                metadata,
                credential,
            },
            record,
            key,
        })
    }

    pub async fn commit_prepared_project_credential(
        &self,
        prepared: PreparedProjectCredentialCreate,
        supplemental: Option<ConditionalPut>,
    ) -> Result<IssuedProjectCredential, IdentityStoreError> {
        let mut batch = WriteBatch::new();
        batch.put(&prepared.key, serde_json::to_vec(&prepared.record)?);
        let mut conditions = vec![KeyCondition::Missing {
            key: prepared.key.clone(),
        }];
        if let Some(supplemental) = supplemental {
            supplemental.append_to(&mut conditions, &mut batch);
        }
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::RecordAlreadyExists);
        }
        Ok(prepared.issued)
    }

    pub async fn rotate_project_credential(
        &self,
        current_id: &ProjectCredentialId,
        replacement_id: ProjectCredentialId,
        overlap_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, IdentityStoreError> {
        let prepared = self
            .prepare_project_credential_rotation(
                current_id,
                replacement_id,
                overlap_seconds,
                now_unix_seconds,
            )
            .await?;
        self.commit_prepared_project_credential_rotation(prepared, None)
            .await
    }

    pub async fn prepare_project_credential_rotation(
        &self,
        current_id: &ProjectCredentialId,
        replacement_id: ProjectCredentialId,
        overlap_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<PreparedProjectCredentialRotation, IdentityStoreError> {
        if overlap_seconds > MAX_CREDENTIAL_OVERLAP_SECONDS || current_id == &replacement_id {
            return Err(IdentityStoreError::InvalidProjectCredential);
        }
        let current_key = self.keyspace.project_credential_key(current_id.as_str())?;
        let current_bytes = self
            .adapter
            .get(&current_key)
            .await?
            .ok_or(IdentityStoreError::ProjectCredentialNotFound)?;
        let mut current: StoredProjectCredential = serde_json::from_slice(&current_bytes)?;
        validate_stored(&current, &self.tenant, current_id)?;
        if current.metadata.state != ProjectCredentialState::Active {
            return Err(IdentityStoreError::InvalidProjectCredential);
        }
        let replacement_credential = generate_credential(current.metadata.kind, &replacement_id);
        let replacement_metadata = ProjectCredentialMetadata {
            scope: self.tenant.clone(),
            id: replacement_id.clone(),
            kind: current.metadata.kind,
            service_scope: current.metadata.service_scope.clone(),
            state: ProjectCredentialState::Active,
            created_at_unix_seconds: now_unix_seconds,
            overlap_ends_at_unix_seconds: None,
            retired_at_unix_seconds: None,
        };
        let replacement = StoredProjectCredential {
            metadata: replacement_metadata.clone(),
            digest: credential_digest(&replacement_credential)?,
        };
        current.metadata.state = ProjectCredentialState::Overlap;
        current.metadata.overlap_ends_at_unix_seconds = Some(
            now_unix_seconds
                .checked_add(overlap_seconds)
                .ok_or(IdentityStoreError::InvalidProjectCredential)?,
        );
        let replacement_key = self
            .keyspace
            .project_credential_key(replacement_id.as_str())?;
        Ok(PreparedProjectCredentialRotation {
            issued: IssuedProjectCredential {
                metadata: replacement_metadata,
                credential: replacement_credential,
            },
            current_key,
            current_bytes,
            current,
            replacement_key,
            replacement,
        })
    }

    pub async fn commit_prepared_project_credential_rotation(
        &self,
        prepared: PreparedProjectCredentialRotation,
        supplemental: Option<ConditionalPut>,
    ) -> Result<IssuedProjectCredential, IdentityStoreError> {
        let mut batch = WriteBatch::with_capacity(3);
        batch.put(
            &prepared.current_key,
            serde_json::to_vec(&prepared.current)?,
        );
        batch.put(
            &prepared.replacement_key,
            serde_json::to_vec(&prepared.replacement)?,
        );
        let mut conditions = vec![
            KeyCondition::ValueEquals {
                key: prepared.current_key.clone(),
                value: prepared.current_bytes,
            },
            KeyCondition::Missing {
                key: prepared.replacement_key.clone(),
            },
        ];
        if let Some(supplemental) = supplemental {
            supplemental.append_to(&mut conditions, &mut batch);
        }
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::ConcurrentProjectCredentialChange);
        }
        Ok(prepared.issued)
    }

    pub async fn retire_project_credential(
        &self,
        id: &ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<(), IdentityStoreError> {
        let key = self.keyspace.project_credential_key(id.as_str())?;
        let bytes = self
            .adapter
            .get(&key)
            .await?
            .ok_or(IdentityStoreError::ProjectCredentialNotFound)?;
        let mut record: StoredProjectCredential = serde_json::from_slice(&bytes)?;
        validate_stored(&record, &self.tenant, id)?;
        if record.metadata.state == ProjectCredentialState::Retired {
            return Ok(());
        }
        record.metadata.state = ProjectCredentialState::Retired;
        record.metadata.retired_at_unix_seconds = Some(now_unix_seconds);
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        if self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals { key, value: bytes }],
                batch,
                durability: self.durability,
            })
            .await?
            != CompareAndWriteResult::Applied
        {
            return Err(IdentityStoreError::ConcurrentProjectCredentialChange);
        }
        Ok(())
    }

    pub async fn project_credential_metadata(
        &self,
        id: &ProjectCredentialId,
    ) -> Result<Option<ProjectCredentialMetadata>, IdentityStoreError> {
        let key = self.keyspace.project_credential_key(id.as_str())?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: StoredProjectCredential = serde_json::from_slice(&bytes)?;
        validate_stored(&record, &self.tenant, id)?;
        Ok(Some(record.metadata))
    }

    pub async fn list_project_credential_metadata(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<ProjectCredentialMetadata>, IdentityStoreError> {
        if limit.get() > 100 {
            return Err(IdentityStoreError::InvalidProjectCredential);
        }
        let entries = self
            .adapter
            .scan(ScanRequest::new(
                self.keyspace.project_credentials_range()?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let mut metadata = Vec::with_capacity(entries.len());
        for entry in entries {
            let record: StoredProjectCredential = serde_json::from_slice(&entry.value)?;
            validate_stored(&record, &self.tenant, record.metadata.id())?;
            metadata.push(record.metadata);
        }
        Ok(metadata)
    }

    pub async fn verify_project_credential(
        &self,
        presented: &str,
        now_unix_seconds: u64,
    ) -> Result<Option<VerifiedProjectCredential>, IdentityStoreError> {
        let Some((presented_kind, id)) = parse_presented_credential(presented) else {
            return Ok(None);
        };
        let key = self.keyspace.project_credential_key(id.as_str())?;
        let Some(bytes) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: StoredProjectCredential = serde_json::from_slice(&bytes)?;
        validate_stored(&record, &self.tenant, &id)?;
        let is_current = match record.metadata.state {
            ProjectCredentialState::Active => true,
            ProjectCredentialState::Overlap => record
                .metadata
                .overlap_ends_at_unix_seconds
                .is_some_and(|ends| now_unix_seconds < ends),
            ProjectCredentialState::Retired => false,
        };
        if !is_current
            || record.metadata.kind != presented_kind
            || !constant_time_matches(record.digest.as_bytes(), presented.as_bytes())
        {
            return Ok(None);
        }
        Ok(Some(match record.metadata.kind {
            ProjectCredentialKind::Public => {
                VerifiedProjectCredential::Public(VerifiedPublicCredential {
                    scope: self.tenant.clone(),
                    credential_id: id,
                })
            }
            ProjectCredentialKind::Service => {
                VerifiedProjectCredential::Service(VerifiedServiceCredential {
                    scope: self.tenant.clone(),
                    credential_id: id,
                    service_scope: record
                        .metadata
                        .service_scope
                        .ok_or(IdentityStoreError::InvalidProjectCredential)?,
                })
            }
        }))
    }
}

fn validate_stored(
    record: &StoredProjectCredential,
    tenant: &TenantScope,
    id: &ProjectCredentialId,
) -> Result<(), IdentityStoreError> {
    if record.metadata.scope != *tenant
        || record.metadata.id != *id
        || (record.metadata.kind == ProjectCredentialKind::Public)
            != record.metadata.service_scope.is_none()
    {
        return Err(IdentityStoreError::InvalidProjectCredential);
    }
    Ok(())
}

fn generate_credential(kind: ProjectCredentialKind, id: &ProjectCredentialId) -> ProjectCredential {
    let mut random = [0_u8; 32];
    OsRng.fill_bytes(&mut random);
    let prefix = match kind {
        ProjectCredentialKind::Public => "mako_pk",
        ProjectCredentialKind::Service => "mako_sk",
    };
    let mut value = format!("{prefix}.{}.", id.as_str());
    for byte in random {
        use fmt::Write;
        write!(&mut value, "{byte:02x}").expect("writing to a string cannot fail");
    }
    ProjectCredential(value)
}

fn parse_presented_credential(
    presented: &str,
) -> Option<(ProjectCredentialKind, ProjectCredentialId)> {
    let mut parts = presented.split('.');
    let kind = match parts.next()? {
        "mako_pk" => ProjectCredentialKind::Public,
        "mako_sk" => ProjectCredentialKind::Service,
        _ => return None,
    };
    let id = ProjectCredentialId::parse(parts.next()?).ok()?;
    let random = parts.next()?;
    if parts.next().is_some()
        || random.len() != 64
        || !random.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some((kind, id))
}

fn credential_digest(
    credential: &ProjectCredential,
) -> Result<CredentialDigest, IdentityStoreError> {
    Ok(CredentialDigest::new(
        blake3::hash(credential.0.as_bytes()).as_bytes().to_vec(),
    )?)
}

fn constant_time_matches(expected_digest: &[u8], presented: &[u8]) -> bool {
    let actual = blake3::hash(presented);
    expected_digest.len() == actual.as_bytes().len()
        && expected_digest
            .iter()
            .zip(actual.as_bytes())
            .fold(0_u8, |difference, (expected, actual)| {
                difference | (expected ^ actual)
            })
            == 0
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{ConditionalPut, Durability, KvAdapter, MemoryAdapter, WriteBatch};

    use super::*;

    #[test]
    fn credentials_are_one_time_scoped_rotatable_and_public_keys_never_bypass() {
        futures::executor::block_on(async {
            let tenant = tenant("prj_abcdefgh");
            let store = IdentityStore::new(
                Arc::new(MemoryAdapter::new()),
                &tenant,
                &tenant,
                Durability::Memory,
            )
            .expect("store");
            let public_id = ProjectCredentialId::parse("key_public_1").expect("id");
            let public = store
                .create_public_project_key(public_id.clone(), 10)
                .await
                .expect("public");
            let raw_public = public.credential.expose_once().to_owned();
            assert!(!format!("{:?}", public.credential).contains(&raw_public));
            assert!(
                serde_json::to_string(
                    &store
                        .project_credential_metadata(&public_id)
                        .await
                        .expect("metadata")
                        .expect("record")
                )
                .expect("json")
                .find(&raw_public)
                .is_none()
            );
            let verified_public = store
                .verify_project_credential(&raw_public, 11)
                .await
                .expect("verify")
                .expect("valid");
            assert!(!verified_public.can_bypass_document_policies());

            let service_id = ProjectCredentialId::parse("key_service_1").expect("id");
            let service = store
                .create_service_credential(service_id.clone(), service_scope(), 10)
                .await
                .expect("service");
            let raw_service = service.credential.expose_once().to_owned();
            let replacement_id = ProjectCredentialId::parse("key_service_2").expect("id");
            let replacement = store
                .rotate_project_credential(&service_id, replacement_id.clone(), 5, 20)
                .await
                .expect("rotate");
            assert!(
                store
                    .verify_project_credential(&raw_service, 24)
                    .await
                    .expect("overlap")
                    .is_some()
            );
            assert!(
                store
                    .verify_project_credential(&raw_service, 25)
                    .await
                    .expect("expired overlap")
                    .is_none()
            );
            let verified = store
                .verify_project_credential(replacement.credential.expose_once(), 25)
                .await
                .expect("replacement")
                .expect("valid");
            let VerifiedProjectCredential::Service(verified) = verified else {
                panic!("must be service");
            };
            assert!(verified.can_bypass_document_policies());
            assert!(verified.permits("todos", ServiceCredentialOperation::Read));
            assert!(!verified.permits("private", ServiceCredentialOperation::Read));

            store
                .retire_project_credential(&replacement_id, 30)
                .await
                .expect("retire");
            assert!(
                store
                    .verify_project_credential(replacement.credential.expose_once(), 31)
                    .await
                    .expect("retired")
                    .is_none()
            );
        });
    }

    #[test]
    fn credential_ids_and_values_cannot_cross_project_boundaries() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let first_tenant = tenant("prj_abcdefgh");
            let second_tenant = tenant("prj_ijklmnop");
            let first = IdentityStore::new(
                adapter.clone(),
                &first_tenant,
                &first_tenant,
                Durability::Memory,
            )
            .expect("first");
            let second =
                IdentityStore::new(adapter, &second_tenant, &second_tenant, Durability::Memory)
                    .expect("second");
            let issued = first
                .create_public_project_key(ProjectCredentialId::parse("key_public").expect("id"), 1)
                .await
                .expect("issued");
            assert!(
                second
                    .verify_project_credential(issued.credential.expose_once(), 2)
                    .await
                    .expect("verify")
                    .is_none()
            );
        });
    }

    #[test]
    fn credential_and_encrypted_retry_record_share_one_atomic_commit() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant("prj_abcdefgh");
            let store = IdentityStore::new(adapter.clone(), &tenant, &tenant, Durability::Memory)
                .expect("store");
            let credential_id = ProjectCredentialId::parse("key_atomic").expect("id");
            let prepared = store
                .prepare_public_project_key(credential_id.clone(), 10)
                .expect("prepare");
            let journal_key = b"tenant-bound-encrypted-journal".to_vec();
            store
                .commit_prepared_project_credential(
                    prepared,
                    Some(ConditionalPut::new(
                        journal_key.clone(),
                        b"ciphertext".to_vec(),
                    )),
                )
                .await
                .expect("atomic commit");
            assert!(
                store
                    .project_credential_metadata(&credential_id)
                    .await
                    .expect("metadata")
                    .is_some()
            );
            assert_eq!(
                adapter.get(&journal_key).await.expect("journal"),
                Some(b"ciphertext".to_vec())
            );

            let occupied_journal = b"occupied-journal".to_vec();
            let mut batch = WriteBatch::new();
            batch.put(&occupied_journal, b"existing");
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("seed conflict");
            let rejected_id = ProjectCredentialId::parse("key_rejected").expect("id");
            let rejected = store
                .prepare_public_project_key(rejected_id.clone(), 11)
                .expect("prepare rejected");
            assert!(
                store
                    .commit_prepared_project_credential(
                        rejected,
                        Some(ConditionalPut::new(
                            occupied_journal,
                            b"new-ciphertext".to_vec(),
                        )),
                    )
                    .await
                    .is_err()
            );
            assert!(
                store
                    .project_credential_metadata(&rejected_id)
                    .await
                    .expect("metadata")
                    .is_none()
            );
        });
    }

    fn service_scope() -> ServiceCredentialScope {
        ServiceCredentialScope::new(
            ["todos".to_owned()],
            [
                ServiceCredentialOperation::Create,
                ServiceCredentialOperation::Read,
            ],
        )
        .expect("scope")
    }

    fn tenant(project: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
