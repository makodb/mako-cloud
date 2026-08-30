use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};

use mako_api::{CollectionId, CollectionScope, TenantScope};
use mako_object_store::{ObjectAddress, ObjectStore, ObjectStoreError};
use mako_policy::{
    DiagnosticSeverity, PolicyCompiler, PolicyEffect, PolicyEvaluationContext, PolicyEvaluator,
    PolicyEvaluatorState, PolicyRule, PolicyRuleId, PolicySet, PolicyState, PolicyVersion,
    SafeRequestMetadata, SubjectId, VerifiedEmail, VerifiedIdentity, VerifiedRole,
};
use mako_storage::{Durability, KvAdapter};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    BucketAccess, BucketConfig, BucketRecord, BucketStore, BucketTotals, FileStorageError,
    ObjectCipher, ObjectKeyRoot, ObjectOperation, ObjectRecord, RuleEffect, content_type_allowed,
    object_schema, validate_object_attributes, validate_object_path,
};

const MAX_LIST_LIMIT: usize = 1_000;
const REMOVAL_PAGE: usize = 500;

/// Who is asking, as the HTTP layer verified them.
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectPrincipal {
    /// No credential at all; only a public bucket's reads admit this.
    Anonymous,
    /// A service credential: policies do not apply, and the caller audits the bypass.
    Service { actor_id: String },
    /// An application user's verified session.
    User {
        user_id: String,
        role: String,
        /// The address the session's token carries, if any, as a bucket rule
        /// reads it: a rule that hands an object to an address needs the
        /// same input a document policy has.
        email: Option<VerifiedEmail>,
        trusted_claims: Value,
    },
}

/// Everything about one request the service needs beyond the principal.
#[derive(Clone, Debug)]
pub struct ObjectRequest {
    pub principal: ObjectPrincipal,
    /// Attributes exposed to policies as `request.<name>`: method and request id.
    pub request_metadata: Vec<(String, String)>,
    pub now_unix_seconds: u64,
    /// The plan's ceiling on stored object bytes for the environment, if capped.
    pub storage_ceiling_bytes: Option<u64>,
    /// What the application attaches to the object it is writing, readable by
    /// the bucket's rules as `new.attributes.<name>` and stored with the
    /// object so `old.attributes.<name>` decides later reads. Ignored on
    /// every operation but a write.
    pub attributes: BTreeMap<String, String>,
}

/// A condition a write holds against the object currently at the path,
/// decided atomically with the write: the record it is checked against is
/// the one the commit is conditioned on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectPrecondition {
    /// `If-None-Match: *` -- only when nothing is stored at the path.
    NotStored,
    /// `If-Match: *` -- only when something is stored at the path.
    Stored,
    /// `If-Match: "<digest>"` -- only when the stored object's plaintext
    /// digest (the `ETag` a read returns) is exactly this.
    Digest(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredObject {
    pub record: ObjectRecord,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectPage {
    pub items: Vec<ObjectRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RemovedBucket {
    pub object_count: u64,
    pub total_bytes: u64,
}

/// Buckets and objects for one tenant. Built per request: it holds no state
/// beyond the store handles and the tenant's derived key.
#[derive(Clone)]
pub struct FileStorageService {
    tenant: TenantScope,
    store: BucketStore,
    objects: Arc<dyn ObjectStore>,
    cipher: ObjectCipher,
}

impl std::fmt::Debug for FileStorageService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileStorageService")
            .field("tenant", &self.tenant)
            .finish_non_exhaustive()
    }
}

impl FileStorageService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        objects: Arc<dyn ObjectStore>,
        key_root: &ObjectKeyRoot,
        tenant: TenantScope,
        durability: Durability,
    ) -> Result<Self, FileStorageError> {
        Ok(Self {
            store: BucketStore::new(adapter, &tenant, durability)?,
            objects,
            cipher: ObjectCipher::for_tenant(key_root, &tenant),
            tenant,
        })
    }

    #[must_use]
    pub fn store(&self) -> &BucketStore {
        &self.store
    }

    // ---- buckets ----------------------------------------------------------

    /// Creates or reconfigures a bucket. Rules are compiled against the object
    /// document first; a bucket whose rules do not compile is never installed.
    pub async fn install_bucket(
        &self,
        config: BucketConfig,
        now_unix_seconds: u64,
    ) -> Result<BucketRecord, FileStorageError> {
        config.validate()?;
        self.compile_rules(&config, 1)?;
        let previous = self.store.get_bucket(&config.id).await?;
        let next = match &previous {
            Some(previous) => BucketRecord {
                config,
                version: previous.version + 1,
                created_at_unix_seconds: previous.created_at_unix_seconds,
                updated_at_unix_seconds: now_unix_seconds.max(previous.updated_at_unix_seconds),
            },
            None => BucketRecord {
                config,
                version: 1,
                created_at_unix_seconds: now_unix_seconds,
                updated_at_unix_seconds: now_unix_seconds,
            },
        };
        self.store.put_bucket(previous.as_ref(), &next).await?;
        Ok(next)
    }

    pub async fn get_bucket(&self, bucket_id: &str) -> Result<BucketRecord, FileStorageError> {
        self.store
            .get_bucket(bucket_id)
            .await?
            .ok_or(FileStorageError::BucketNotFound)
    }

    pub async fn list_buckets(&self) -> Result<Vec<BucketRecord>, FileStorageError> {
        self.store
            .list_buckets(NonZeroUsize::new(MAX_LIST_LIMIT).expect("limit"))
            .await
    }

    pub async fn inspect_bucket(
        &self,
        bucket_id: &str,
    ) -> Result<(BucketRecord, BucketTotals), FileStorageError> {
        let bucket = self.get_bucket(bucket_id).await?;
        let totals = self.store.totals(bucket_id).await?;
        Ok((bucket, totals))
    }

    /// The environment's stored object bytes and count, for the sampled level.
    pub async fn environment_totals(&self) -> Result<BucketTotals, FileStorageError> {
        self.store.environment_totals().await
    }

    /// Removes a bucket. One that still holds objects is refused unless the
    /// caller confirmed their loss, in which case every object goes first.
    pub async fn remove_bucket(
        &self,
        bucket_id: &str,
        delete_objects: bool,
    ) -> Result<RemovedBucket, FileStorageError> {
        let bucket = self.get_bucket(bucket_id).await?;
        let mut removed = RemovedBucket::default();
        loop {
            let objects = self
                .store
                .list_objects(
                    bucket_id,
                    None,
                    None,
                    NonZeroUsize::new(REMOVAL_PAGE).expect("page"),
                )
                .await?;
            if objects.is_empty() {
                break;
            }
            if !delete_objects {
                return Err(FileStorageError::BucketNotEmpty);
            }
            for record in objects {
                self.remove_record_and_bytes(&record).await?;
                removed.object_count += 1;
                removed.total_bytes = removed.total_bytes.saturating_add(record.size_bytes);
            }
        }
        let current = self.get_bucket(bucket_id).await?;
        if current != bucket {
            return Err(FileStorageError::Conflict);
        }
        self.store.delete_bucket(&current).await?;
        Ok(removed)
    }

    // ---- objects ----------------------------------------------------------

    /// Stores an object under a bucket. A new path is a `create`, an existing
    /// one an `update`; both are evaluated with the object document the write
    /// would produce, so a rule can constrain owner, size, type, and path.
    pub async fn put_object(
        &self,
        bucket_id: &str,
        path: &str,
        content_type: &str,
        bytes: &[u8],
        request: &ObjectRequest,
    ) -> Result<ObjectRecord, FileStorageError> {
        self.put_object_if(bucket_id, path, content_type, bytes, request, &[])
            .await
    }

    /// [`Self::put_object`] that writes only if every precondition holds
    /// against the object currently at the path. Preconditions are checked
    /// after the bucket's rules, so a caller the rules refuse learns nothing
    /// about what is stored, and atomically with the commit: the record they
    /// were checked against is the one the write is conditioned on, so a
    /// concurrent change is a [`FileStorageError::Conflict`], never a write
    /// past a failed check.
    pub async fn put_object_if(
        &self,
        bucket_id: &str,
        path: &str,
        content_type: &str,
        bytes: &[u8],
        request: &ObjectRequest,
        preconditions: &[ObjectPrecondition],
    ) -> Result<ObjectRecord, FileStorageError> {
        validate_object_path(path)?;
        let bucket = self.get_bucket(bucket_id).await?;
        let size = u64::try_from(bytes.len()).map_err(|_| FileStorageError::ObjectTooLarge {
            maximum_bytes: bucket.config.max_object_bytes,
        })?;
        if size == 0 || size > bucket.config.max_object_bytes {
            return Err(FileStorageError::ObjectTooLarge {
                maximum_bytes: bucket.config.max_object_bytes,
            });
        }
        if !content_type_allowed(&bucket.config.allowed_content_types, content_type) {
            return Err(FileStorageError::ContentTypeNotAllowed);
        }
        let previous = self.store.get_object(bucket_id, path).await?;
        let observed = self.store.observe_totals(bucket_id).await?;
        if let Some(ceiling) = request.storage_ceiling_bytes {
            let environment = self.store.environment_totals().await?;
            let after = environment
                .total_bytes
                .saturating_sub(previous.as_ref().map_or(0, |record| record.size_bytes))
                .saturating_add(size);
            if after > ceiling {
                return Err(FileStorageError::StorageCapExceeded {
                    ceiling_bytes: ceiling,
                });
            }
        }
        let owner_id = match &request.principal {
            ObjectPrincipal::User { user_id, .. } => Some(user_id.clone()),
            ObjectPrincipal::Service { .. } | ObjectPrincipal::Anonymous => {
                previous.as_ref().and_then(|record| record.owner_id.clone())
            }
        };
        validate_object_attributes(&request.attributes)?;
        let plaintext_digest = digest_of(bytes);
        let sealed = self.cipher.seal(&aad(bucket_id, path), bytes)?;
        let stored_digest = digest_of(&sealed);
        let next = ObjectRecord {
            bucket_id: bucket_id.to_owned(),
            path: path.to_owned(),
            content_type: normalize_content_type(content_type),
            size_bytes: size,
            owner_id,
            digest: plaintext_digest,
            stored_digest: stored_digest.clone(),
            created_at_unix_seconds: previous
                .as_ref()
                .map_or(request.now_unix_seconds, |record| {
                    record.created_at_unix_seconds
                }),
            updated_at_unix_seconds: request.now_unix_seconds,
            attributes: request.attributes.clone(),
        };
        let operation = if previous.is_some() {
            ObjectOperation::Update
        } else {
            ObjectOperation::Create
        };
        self.authorize(
            &bucket,
            operation,
            request,
            previous.as_ref().map(ObjectRecord::policy_document),
            Some(next.policy_document()),
        )?;
        for precondition in preconditions {
            let holds = match precondition {
                ObjectPrecondition::NotStored => previous.is_none(),
                ObjectPrecondition::Stored => previous.is_some(),
                ObjectPrecondition::Digest(digest) => previous
                    .as_ref()
                    .is_some_and(|record| &record.digest == digest),
            };
            if !holds {
                return Err(FileStorageError::PreconditionFailed);
            }
        }
        // Bytes first, then the record: an orphaned blob is a leak, a record
        // pointing at nothing is a lie.
        let address =
            ObjectAddress::application_object(self.tenant.clone(), bucket_id, &stored_digest)
                .map_err(|_| FileStorageError::ObjectStore)?;
        self.objects
            .put_immutable(&self.tenant, &address, Arc::from(sealed))
            .await
            .map_err(map_store_error)?;
        let totals = BucketTotals {
            object_count: observed.totals.object_count + u64::from(previous.is_none()),
            total_bytes: observed
                .totals
                .total_bytes
                .saturating_sub(previous.as_ref().map_or(0, |record| record.size_bytes))
                .saturating_add(size),
        };
        self.store
            .commit_object(previous.as_ref(), &next, &observed, totals)
            .await?;
        if let Some(previous) = previous
            && previous.stored_digest != next.stored_digest
        {
            // The old bytes are unreachable now; losing this delete leaks
            // only storage, never data, so it is not allowed to fail the write.
            let _ = self.delete_bytes(bucket_id, &previous.stored_digest).await;
        }
        Ok(next)
    }

    /// Reads an object. A public bucket serves reads to anyone; otherwise the
    /// bucket's rules decide, and a refusal sends no bytes.
    pub async fn get_object(
        &self,
        bucket_id: &str,
        path: &str,
        request: &ObjectRequest,
    ) -> Result<StoredObject, FileStorageError> {
        validate_object_path(path)?;
        let bucket = self.get_bucket(bucket_id).await?;
        let record = self
            .store
            .get_object(bucket_id, path)
            .await?
            .ok_or(FileStorageError::ObjectNotFound)?;
        self.authorize(
            &bucket,
            ObjectOperation::Read,
            request,
            Some(record.policy_document()),
            None,
        )?;
        let bytes = self.read_bytes(&record).await?;
        Ok(StoredObject { record, bytes })
    }

    pub async fn delete_object(
        &self,
        bucket_id: &str,
        path: &str,
        request: &ObjectRequest,
    ) -> Result<ObjectRecord, FileStorageError> {
        validate_object_path(path)?;
        let bucket = self.get_bucket(bucket_id).await?;
        let record = self
            .store
            .get_object(bucket_id, path)
            .await?
            .ok_or(FileStorageError::ObjectNotFound)?;
        self.authorize(
            &bucket,
            ObjectOperation::Delete,
            request,
            Some(record.policy_document()),
            None,
        )?;
        self.remove_record_and_bytes(&record).await?;
        Ok(record)
    }

    /// Lists objects the principal may read, in path order. Each candidate is
    /// evaluated as a `read`, so a listing never names what a read would refuse.
    pub async fn list_objects(
        &self,
        bucket_id: &str,
        prefix: Option<&str>,
        limit: usize,
        cursor: Option<&str>,
        request: &ObjectRequest,
    ) -> Result<ObjectPage, FileStorageError> {
        let bucket = self.get_bucket(bucket_id).await?;
        if let Some(prefix) = prefix
            && (prefix.len() > crate::MAX_OBJECT_PATH_BYTES || prefix.contains(".."))
        {
            return Err(FileStorageError::InvalidPath("prefix is invalid"));
        }
        if let Some(cursor) = cursor {
            validate_object_path(cursor)?;
        }
        let limit = NonZeroUsize::new(limit.clamp(1, MAX_LIST_LIMIT)).expect("limit");
        let candidates = self
            .store
            .list_objects(bucket_id, prefix, cursor, limit)
            .await?;
        let more = candidates.len() == limit.get();
        let items: Vec<ObjectRecord> = candidates
            .into_iter()
            .filter(|record| {
                self.authorize(
                    &bucket,
                    ObjectOperation::Read,
                    request,
                    Some(record.policy_document()),
                    None,
                )
                .is_ok()
            })
            .collect();
        let next_cursor = if more {
            // The cursor is the last candidate seen, readable or not, so a page
            // of refusals still advances.
            self.store
                .list_objects(bucket_id, prefix, cursor, limit)
                .await?
                .last()
                .map(|record| record.path.clone())
        } else {
            None
        };
        Ok(ObjectPage { items, next_cursor })
    }

    // ---- internals --------------------------------------------------------

    fn authorize(
        &self,
        bucket: &BucketRecord,
        operation: ObjectOperation,
        request: &ObjectRequest,
        old_document: Option<Value>,
        new_document: Option<Value>,
    ) -> Result<(), FileStorageError> {
        let identity = match &request.principal {
            ObjectPrincipal::Service { .. } => return Ok(()),
            ObjectPrincipal::Anonymous => {
                if bucket.config.access == BucketAccess::Public
                    && operation == ObjectOperation::Read
                {
                    return Ok(());
                }
                return Err(FileStorageError::Denied("credential_required".to_owned()));
            }
            ObjectPrincipal::User {
                user_id,
                role,
                email,
                trusted_claims,
            } => {
                if bucket.config.access == BucketAccess::Public
                    && operation == ObjectOperation::Read
                {
                    return Ok(());
                }
                let subject = SubjectId::parse(user_id.as_str())
                    .map_err(|_| FileStorageError::Denied("identity_invalid".to_owned()))?;
                let role = VerifiedRole::parse(role.as_str())
                    .map_err(|_| FileStorageError::Denied("identity_invalid".to_owned()))?;
                VerifiedIdentity::user(subject, role, email.clone(), trusted_claims.clone())
                    .map_err(|_| FileStorageError::Denied("identity_invalid".to_owned()))?
            }
        };
        let compiled = self.compile_rules(&bucket.config, bucket.version)?;
        let metadata = SafeRequestMetadata::new(request.request_metadata.iter().cloned())
            .map_err(|_| FileStorageError::Denied("request_invalid".to_owned()))?;
        let context = PolicyEvaluationContext::new(
            self.scope(&bucket.config.id)?,
            operation.as_policy_operation(),
            identity,
            old_document,
            new_document,
            metadata,
        )
        .map_err(|_| FileStorageError::Denied("context_invalid".to_owned()))?;
        let decision = PolicyEvaluator.evaluate_guarded(
            Some(&compiled),
            &context,
            PolicyEvaluatorState::Ready,
        );
        if decision.is_allowed() {
            Ok(())
        } else {
            Err(FileStorageError::Denied(decision.stable_code().to_owned()))
        }
    }

    fn scope(&self, bucket_id: &str) -> Result<CollectionScope, FileStorageError> {
        let id = CollectionId::parse(bucket_id)
            .map_err(|_| FileStorageError::InvalidBucket("bucket id"))?;
        Ok(CollectionScope::new(self.tenant.clone(), id))
    }

    fn compile_rules(
        &self,
        config: &BucketConfig,
        version: u64,
    ) -> Result<mako_policy::CompiledPolicySet, FileStorageError> {
        let rules = config
            .rules
            .iter()
            .map(|rule| {
                PolicyRule::new(
                    PolicyRuleId::parse(rule.id.as_str())
                        .map_err(|error| FileStorageError::InvalidPolicy(error.to_string()))?,
                    match rule.effect {
                        RuleEffect::Allow => PolicyEffect::Allow,
                        RuleEffect::Deny => PolicyEffect::Deny,
                    },
                    rule.operations
                        .iter()
                        .map(|operation| operation.as_policy_operation()),
                    rule.expression.as_str(),
                )
                .map_err(|error| FileStorageError::InvalidPolicy(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let policy = PolicySet::new(
            self.scope(&config.id)?,
            PolicyVersion::new(version.max(1))
                .map_err(|error| FileStorageError::InvalidPolicy(error.to_string()))?,
            PolicyState::Active,
            rules,
            Vec::new(),
        )
        .map_err(|error| FileStorageError::InvalidPolicy(error.to_string()))?;
        let compilation = PolicyCompiler::default()
            .compile(&policy, &object_schema())
            .map_err(|error| FileStorageError::InvalidPolicy(error.to_string()))?;
        let errors: Vec<String> = compilation
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
            .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.message()))
            .collect();
        if !errors.is_empty() {
            return Err(FileStorageError::InvalidPolicy(errors.join("; ")));
        }
        compilation
            .into_compiled()
            .ok_or_else(|| FileStorageError::InvalidPolicy("rules did not compile".to_owned()))
    }

    async fn read_bytes(&self, record: &ObjectRecord) -> Result<Vec<u8>, FileStorageError> {
        let address = ObjectAddress::application_object(
            self.tenant.clone(),
            &record.bucket_id,
            &record.stored_digest,
        )
        .map_err(|_| FileStorageError::Corrupt)?;
        let sealed = self
            .objects
            .get(&self.tenant, &address)
            .await
            .map_err(map_store_error)?
            .ok_or(FileStorageError::Corrupt)?;
        let bytes = self
            .cipher
            .open(&aad(&record.bucket_id, &record.path), &sealed)?;
        if digest_of(&bytes) != record.digest || bytes.len() as u64 != record.size_bytes {
            return Err(FileStorageError::Corrupt);
        }
        Ok(bytes)
    }

    async fn remove_record_and_bytes(&self, record: &ObjectRecord) -> Result<(), FileStorageError> {
        let observed = self.store.observe_totals(&record.bucket_id).await?;
        let totals = BucketTotals {
            object_count: observed.totals.object_count.saturating_sub(1),
            total_bytes: observed
                .totals
                .total_bytes
                .saturating_sub(record.size_bytes),
        };
        self.store.remove_object(record, &observed, totals).await?;
        let _ = self
            .delete_bytes(&record.bucket_id, &record.stored_digest)
            .await;
        Ok(())
    }

    async fn delete_bytes(
        &self,
        bucket_id: &str,
        stored_digest: &str,
    ) -> Result<(), FileStorageError> {
        let address =
            ObjectAddress::application_object(self.tenant.clone(), bucket_id, stored_digest)
                .map_err(|_| FileStorageError::Corrupt)?;
        self.objects
            .delete(&self.tenant, &address)
            .await
            .map_err(map_store_error)
    }
}

fn aad(bucket_id: &str, path: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(bucket_id.len() + path.len() + 1);
    aad.extend_from_slice(bucket_id.as_bytes());
    aad.push(0);
    aad.extend_from_slice(path.as_bytes());
    aad
}

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn normalize_content_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn map_store_error(error: ObjectStoreError) -> FileStorageError {
    match error {
        ObjectStoreError::Integrity | ObjectStoreError::InvalidObject => FileStorageError::Corrupt,
        ObjectStoreError::ImmutableConflict => FileStorageError::Conflict,
        _ => FileStorageError::ObjectStore,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_object_store::MemoryObjectStore;
    use mako_storage::{Durability, KvAdapter, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::BucketRule;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_filestore").expect("project"),
            EnvironmentId::parse("env_filestore").expect("environment"),
        )
    }

    fn service_on(
        adapter: Arc<dyn KvAdapter>,
        objects: Arc<MemoryObjectStore>,
        tenant: TenantScope,
    ) -> FileStorageService {
        FileStorageService::new(
            adapter,
            objects,
            &ObjectKeyRoot::from_bytes([7u8; 32]),
            tenant,
            Durability::Memory,
        )
        .expect("service")
    }

    fn user(id: &str) -> ObjectRequest {
        ObjectRequest {
            principal: ObjectPrincipal::User {
                user_id: id.to_owned(),
                role: "user".to_owned(),
                email: None,
                trusted_claims: json!({}),
            },
            request_metadata: vec![("method".to_owned(), "PUT".to_owned())],
            now_unix_seconds: 1_000,
            storage_ceiling_bytes: None,
            attributes: BTreeMap::new(),
        }
    }

    /// A caller who is also attaching something the bucket's rules read.
    fn user_attaching(id: &str, claims: Value, attributes: &[(&str, &str)]) -> ObjectRequest {
        let mut request = user(id);
        request.principal = ObjectPrincipal::User {
            user_id: id.to_owned(),
            role: "user".to_owned(),
            email: None,
            trusted_claims: claims,
        };
        request.attributes = attributes
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        request
    }

    fn owner_bucket(id: &str, access: BucketAccess) -> BucketConfig {
        BucketConfig {
            id: id.to_owned(),
            access,
            max_object_bytes: 1024,
            allowed_content_types: vec!["image/*".to_owned(), "text/plain".to_owned()],
            rules: vec![
                BucketRule {
                    id: "owner-creates".to_owned(),
                    effect: RuleEffect::Allow,
                    operations: vec![ObjectOperation::Create],
                    expression: "new.owner_id == identity.user_id".to_owned(),
                },
                BucketRule {
                    id: "owner-changes".to_owned(),
                    effect: RuleEffect::Allow,
                    operations: vec![ObjectOperation::Update],
                    expression:
                        "old.owner_id == identity.user_id && new.owner_id == identity.user_id"
                            .to_owned(),
                },
                BucketRule {
                    id: "owner-reads-and-deletes".to_owned(),
                    effect: RuleEffect::Allow,
                    operations: vec![ObjectOperation::Read, ObjectOperation::Delete],
                    expression: "old.owner_id == identity.user_id".to_owned(),
                },
            ],
        }
    }

    /// A receipt belongs to a household, not to whoever uploaded it. Before
    /// attributes, the object document named the uploader and nothing else,
    /// so a bucket rule could reach exactly one person and shared files were
    /// unbuildable. The attribute names the household; the rule verifies that
    /// name against the caller's trusted claims, so attaching a household
    /// somebody is not in grants nothing.
    #[test]
    fn an_object_attribute_lets_a_rule_reach_a_household_and_no_further() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter.clone(), objects.clone(), tenant());
            let mut config = owner_bucket("receipts", BucketAccess::Policy);
            config.rules = vec![
                BucketRule {
                    id: "member-writes".to_owned(),
                    effect: RuleEffect::Allow,
                    operations: vec![ObjectOperation::Create, ObjectOperation::Update],
                    expression: "claims.households[new.attributes.household_id] != null".to_owned(),
                },
                BucketRule {
                    id: "member-reads".to_owned(),
                    effect: RuleEffect::Allow,
                    operations: vec![ObjectOperation::Read, ObjectOperation::Delete],
                    expression: "claims.households[old.attributes.household_id] != null".to_owned(),
                },
            ];
            service.install_bucket(config, 500).await.expect("bucket");

            let in_household = json!({"households": {"hh_one": "owner"}});
            let uploader = user_attaching(
                "user_alice",
                in_household.clone(),
                &[("household_id", "hh_one")],
            );
            let record = service
                .put_object(
                    "receipts",
                    "households/hh_one/transactions/txn_1/receipt.png",
                    "image/png",
                    b"PNG BYTES",
                    &uploader,
                )
                .await
                .expect("upload");
            assert_eq!(
                record.attributes.get("household_id").map(String::as_str),
                Some("hh_one")
            );

            // Another member of the same household reads it, though they did
            // not upload it -- which is the whole point.
            let other_member = user_attaching("user_bob", in_household, &[]);
            let read = service
                .get_object(
                    "receipts",
                    "households/hh_one/transactions/txn_1/receipt.png",
                    &other_member,
                )
                .await
                .expect("member reads");
            assert_eq!(read.bytes, b"PNG BYTES");

            // Somebody in a different household does not.
            let outsider =
                user_attaching("user_eve", json!({"households": {"hh_two": "owner"}}), &[]);
            assert!(matches!(
                service
                    .get_object(
                        "receipts",
                        "households/hh_one/transactions/txn_1/receipt.png",
                        &outsider,
                    )
                    .await,
                Err(FileStorageError::Denied(_))
            ));

            // And naming a household they are not in buys the outsider
            // nothing: the rule reads the claim, not the attribute.
            let forging = user_attaching(
                "user_eve",
                json!({"households": {"hh_two": "owner"}}),
                &[("household_id", "hh_one")],
            );
            assert!(matches!(
                service
                    .put_object(
                        "receipts",
                        "households/hh_one/forged.png",
                        "image/png",
                        b"X",
                        &forging
                    )
                    .await,
                Err(FileStorageError::Denied(_))
            ));

            // Attributes are bounded; an unbounded bag would be a policy input
            // an application could grow without limit.
            let mut too_many = user_attaching(
                "user_alice",
                json!({"households": {"hh_one": "owner"}}),
                &[],
            );
            for index in 0..9 {
                too_many
                    .attributes
                    .insert(format!("name_{index}"), "value".to_owned());
            }
            assert!(matches!(
                service
                    .put_object(
                        "receipts",
                        "households/hh_one/many.png",
                        "image/png",
                        b"X",
                        &too_many
                    )
                    .await,
                Err(FileStorageError::InvalidAttributes(_))
            ));
        });
    }

    #[test]
    fn objects_are_stored_encrypted_served_to_their_owner_and_refused_to_others() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter.clone(), objects.clone(), tenant());
            let bucket = service
                .install_bucket(owner_bucket("avatars", BucketAccess::Policy), 500)
                .await
                .expect("bucket");
            assert_eq!(bucket.version, 1);

            let alice = user("user_alice");
            let record = service
                .put_object(
                    "avatars",
                    "users/alice/me.png",
                    "image/png",
                    b"PNG BYTES",
                    &alice,
                )
                .await
                .expect("upload");
            assert_eq!(record.owner_id.as_deref(), Some("user_alice"));
            assert_eq!(record.size_bytes, 9);
            assert_eq!(record.content_type, "image/png");
            assert!(record.digest.starts_with("sha256:"));
            assert_ne!(
                record.digest, record.stored_digest,
                "the store holds ciphertext"
            );

            // The store never sees the plaintext.
            let held = objects.dump();
            assert!(
                held.values()
                    .all(|bytes| !bytes.windows(9).any(|window| window == b"PNG BYTES"))
            );

            let served = service
                .get_object("avatars", "users/alice/me.png", &alice)
                .await
                .expect("owner reads");
            assert_eq!(served.bytes, b"PNG BYTES");

            let bob = user("user_bob");
            assert!(matches!(
                service
                    .get_object("avatars", "users/alice/me.png", &bob)
                    .await,
                Err(FileStorageError::Denied(_))
            ));
            assert!(matches!(
                service
                    .delete_object("avatars", "users/alice/me.png", &bob)
                    .await,
                Err(FileStorageError::Denied(_))
            ));
            let anonymous = ObjectRequest {
                principal: ObjectPrincipal::Anonymous,
                ..alice.clone()
            };
            assert!(matches!(
                service.get_object("avatars", "users/alice/me.png", &anonymous).await,
                Err(FileStorageError::Denied(code)) if code == "credential_required"
            ));

            // Totals follow every write.
            let (_, totals) = service.inspect_bucket("avatars").await.expect("inspect");
            assert_eq!(
                totals,
                BucketTotals {
                    object_count: 1,
                    total_bytes: 9
                }
            );
            let replaced = service
                .put_object(
                    "avatars",
                    "users/alice/me.png",
                    "image/png",
                    b"PNG BYTES v2",
                    &alice,
                )
                .await
                .expect("overwrite");
            assert_eq!(
                replaced.created_at_unix_seconds,
                record.created_at_unix_seconds
            );
            let (_, totals) = service.inspect_bucket("avatars").await.expect("inspect");
            assert_eq!(
                totals,
                BucketTotals {
                    object_count: 1,
                    total_bytes: 12
                }
            );
            assert_eq!(
                objects.dump().len(),
                1,
                "the replaced bytes are gone from the store"
            );
            assert_eq!(
                service
                    .environment_totals()
                    .await
                    .expect("environment")
                    .total_bytes,
                12
            );

            service
                .delete_object("avatars", "users/alice/me.png", &alice)
                .await
                .expect("owner deletes");
            assert!(objects.dump().is_empty());
            assert_eq!(
                service.environment_totals().await.expect("environment"),
                BucketTotals::default()
            );
        });
    }

    #[test]
    fn limits_paths_and_caps_fail_closed() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter, objects, tenant());
            service
                .install_bucket(owner_bucket("uploads", BucketAccess::Policy), 500)
                .await
                .expect("bucket");
            let alice = user("user_alice");
            for path in [
                "../escape",
                "a/../b",
                "/abs",
                "a//b",
                "a/",
                "with\u{0}nul",
                "",
            ] {
                assert!(
                    matches!(
                        service
                            .put_object("uploads", path, "text/plain", b"x", &alice)
                            .await,
                        Err(FileStorageError::InvalidPath(_))
                    ),
                    "{path:?} must be refused"
                );
            }
            assert!(matches!(
                service
                    .put_object("uploads", "big.txt", "text/plain", &[0u8; 1025], &alice)
                    .await,
                Err(FileStorageError::ObjectTooLarge {
                    maximum_bytes: 1024
                })
            ));
            assert!(matches!(
                service
                    .put_object(
                        "uploads",
                        "app.bin",
                        "application/octet-stream",
                        b"x",
                        &alice
                    )
                    .await,
                Err(FileStorageError::ContentTypeNotAllowed)
            ));
            let capped = ObjectRequest {
                storage_ceiling_bytes: Some(10),
                ..alice.clone()
            };
            service
                .put_object("uploads", "a.txt", "text/plain", b"12345678", &capped)
                .await
                .expect("under the ceiling");
            assert!(matches!(
                service
                    .put_object("uploads", "b.txt", "text/plain", b"123", &capped)
                    .await,
                Err(FileStorageError::StorageCapExceeded { ceiling_bytes: 10 })
            ));
            service
                .put_object("uploads", "a.txt", "text/plain", b"1234567890", &capped)
                .await
                .expect("replacing counts the freed bytes");
            assert!(matches!(
                service.remove_bucket("uploads", false).await,
                Err(FileStorageError::BucketNotEmpty)
            ));
            let removed = service
                .remove_bucket("uploads", true)
                .await
                .expect("forced removal");
            assert_eq!(removed.object_count, 1);
            assert!(matches!(
                service.get_bucket("uploads").await,
                Err(FileStorageError::BucketNotFound)
            ));
        });
    }

    #[test]
    fn public_buckets_serve_reads_to_anyone_but_never_writes() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter, objects, tenant());
            service
                .install_bucket(owner_bucket("public-assets", BucketAccess::Public), 500)
                .await
                .expect("bucket");
            let alice = user("user_alice");
            service
                .put_object("public-assets", "logo.png", "image/png", b"LOGO", &alice)
                .await
                .expect("owner uploads");
            let anonymous = ObjectRequest {
                principal: ObjectPrincipal::Anonymous,
                ..alice.clone()
            };
            let served = service
                .get_object("public-assets", "logo.png", &anonymous)
                .await
                .expect("anyone reads a public bucket");
            assert_eq!(served.bytes, b"LOGO");
            assert!(matches!(
                service
                    .put_object("public-assets", "logo.png", "image/png", b"X", &anonymous)
                    .await,
                Err(FileStorageError::Denied(_))
            ));
            assert!(matches!(
                service
                    .delete_object("public-assets", "logo.png", &anonymous)
                    .await,
                Err(FileStorageError::Denied(_))
            ));
            let bob = user("user_bob");
            assert!(matches!(
                service
                    .put_object("public-assets", "logo.png", "image/png", b"X", &bob)
                    .await,
                Err(FileStorageError::Denied(_))
            ));
            let page = service
                .list_objects("public-assets", None, 10, None, &anonymous)
                .await
                .expect("public listing");
            assert_eq!(page.items.len(), 1);
        });
    }

    #[test]
    fn listings_show_only_what_a_read_would_allow_and_page_by_path() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter, objects, tenant());
            service
                .install_bucket(owner_bucket("docs", BucketAccess::Policy), 500)
                .await
                .expect("bucket");
            let alice = user("user_alice");
            let bob = user("user_bob");
            for path in ["alice/1.txt", "alice/2.txt", "alice/3.txt"] {
                service
                    .put_object("docs", path, "text/plain", b"a", &alice)
                    .await
                    .expect("alice");
            }
            service
                .put_object("docs", "bob/1.txt", "text/plain", b"b", &bob)
                .await
                .expect("bob");
            let page = service
                .list_objects("docs", None, 2, None, &alice)
                .await
                .expect("page 1");
            assert_eq!(
                page.items
                    .iter()
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>(),
                ["alice/1.txt", "alice/2.txt"]
            );
            let cursor = page.next_cursor.expect("more");
            let page = service
                .list_objects("docs", None, 2, Some(&cursor), &alice)
                .await
                .expect("page 2");
            assert_eq!(
                page.items
                    .iter()
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>(),
                ["alice/3.txt"],
                "bob's file is filtered out"
            );
            let bobs = service
                .list_objects("docs", Some("bob/"), 10, None, &bob)
                .await
                .expect("prefix");
            assert_eq!(bobs.items.len(), 1);
            assert!(bobs.next_cursor.is_none());
            let service_view = ObjectRequest {
                principal: ObjectPrincipal::Service {
                    actor_id: "svc_backup".to_owned(),
                },
                ..alice.clone()
            };
            let all = service
                .list_objects("docs", None, 10, None, &service_view)
                .await
                .expect("service sees all");
            assert_eq!(all.items.len(), 4);
        });
    }

    #[test]
    fn rules_that_do_not_compile_never_install_and_keys_are_per_tenant() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter.clone(), objects.clone(), tenant());
            let mut broken = owner_bucket("broken", BucketAccess::Policy);
            broken.rules[0].expression = "new.no_such_field == 1".to_owned();
            assert!(matches!(
                service.install_bucket(broken, 500).await,
                Err(FileStorageError::InvalidPolicy(_))
            ));
            assert!(matches!(
                service.get_bucket("broken").await,
                Err(FileStorageError::BucketNotFound)
            ));

            let mut bad_name = owner_bucket("Bad", BucketAccess::Policy);
            bad_name.id = "Bad".to_owned();
            assert!(matches!(
                service.install_bucket(bad_name, 500).await,
                Err(FileStorageError::InvalidBucket(_))
            ));

            // The same bytes sealed for one tenant do not open for another.
            service
                .install_bucket(owner_bucket("shared", BucketAccess::Public), 500)
                .await
                .expect("bucket");
            let alice = user("user_alice");
            let record = service
                .put_object(
                    "shared",
                    "secret.txt",
                    "text/plain",
                    b"for one tenant",
                    &alice,
                )
                .await
                .expect("upload");
            let other_tenant = TenantScope::new(
                ProjectId::parse("prj_filestore").expect("project"),
                EnvironmentId::parse("env_elsewhere").expect("environment"),
            );
            let other = service_on(adapter, objects.clone(), other_tenant.clone());
            let foreign_cipher =
                ObjectCipher::for_tenant(&ObjectKeyRoot::from_bytes([7u8; 32]), &other_tenant);
            let sealed = objects.dump().into_values().next().expect("stored bytes");
            assert!(
                foreign_cipher
                    .open(&aad("shared", "secret.txt"), &sealed)
                    .is_err()
            );
            assert!(
                other.get_bucket("shared").await.is_err(),
                "buckets are per tenant too"
            );
            assert_eq!(record.size_bytes, 14);
        });
    }

    /// `If-None-Match: *` makes an upload create-only; `If-Match` pins it
    /// to the version the caller read. Both are decided after the rules and
    /// with the record the commit is conditioned on.
    #[test]
    fn put_preconditions_hold_against_the_stored_object_after_the_rules() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let objects = Arc::new(MemoryObjectStore::default());
            let service = service_on(adapter, objects, tenant());
            service
                .install_bucket(owner_bucket("receipts", BucketAccess::Policy), 500)
                .await
                .expect("bucket");
            let alice = user("user_alice");
            let path = "households/h1/receipt.txt";
            assert_eq!(
                service
                    .put_object_if(
                        "receipts",
                        path,
                        "text/plain",
                        b"first",
                        &alice,
                        &[ObjectPrecondition::Digest("sha256:nothing".to_owned())],
                    )
                    .await
                    .expect_err("nothing to match yet"),
                FileStorageError::PreconditionFailed
            );
            assert_eq!(
                service
                    .put_object_if(
                        "receipts",
                        path,
                        "text/plain",
                        b"first",
                        &alice,
                        &[ObjectPrecondition::Stored],
                    )
                    .await
                    .expect_err("nothing stored yet"),
                FileStorageError::PreconditionFailed
            );
            let first = service
                .put_object_if(
                    "receipts",
                    path,
                    "text/plain",
                    b"first",
                    &alice,
                    &[ObjectPrecondition::NotStored],
                )
                .await
                .expect("create-only upload of a new path");
            // A second create-only upload loses, and leaves the object alone.
            assert_eq!(
                service
                    .put_object_if(
                        "receipts",
                        path,
                        "text/plain",
                        b"second",
                        &alice,
                        &[ObjectPrecondition::NotStored],
                    )
                    .await
                    .expect_err("the path is taken"),
                FileStorageError::PreconditionFailed
            );
            assert_eq!(
                service
                    .get_object("receipts", path, &alice)
                    .await
                    .expect("read")
                    .bytes,
                b"first"
            );
            // The wrong digest loses; the digest a read returned wins.
            assert_eq!(
                service
                    .put_object_if(
                        "receipts",
                        path,
                        "text/plain",
                        b"second",
                        &alice,
                        &[ObjectPrecondition::Digest("sha256:stale".to_owned())],
                    )
                    .await
                    .expect_err("stale digest"),
                FileStorageError::PreconditionFailed
            );
            let second = service
                .put_object_if(
                    "receipts",
                    path,
                    "text/plain",
                    b"second",
                    &alice,
                    &[
                        ObjectPrecondition::Stored,
                        ObjectPrecondition::Digest(first.digest.clone()),
                    ],
                )
                .await
                .expect("matching digest");
            assert_ne!(second.digest, first.digest);
            assert_eq!(
                service
                    .get_object("receipts", path, &alice)
                    .await
                    .expect("read")
                    .bytes,
                b"second"
            );
            // The rules answer first: a stranger's create-only probe of a
            // taken path is refused as a policy denial, not a precondition.
            assert!(matches!(
                service
                    .put_object_if(
                        "receipts",
                        path,
                        "text/plain",
                        b"probe",
                        &user("user_mallory"),
                        &[ObjectPrecondition::NotStored],
                    )
                    .await,
                Err(FileStorageError::Denied(_))
            ));
        });
    }
}
