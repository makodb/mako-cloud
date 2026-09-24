use std::{error::Error, fmt};

use crate::KeyRange;

const FORMAT_VERSION: u8 = 1;
const SYSTEM_NAMESPACE: u8 = 0x10;
const PROJECT_NAMESPACE: u8 = 0x20;
const PROJECT_METADATA: u8 = 0x21;
const ENVIRONMENT_NAMESPACE: u8 = 0x30;
const ENVIRONMENT_METADATA: u8 = 0x31;
const COLLECTION_NAMESPACE: u8 = 0x40;
const DOCUMENT_NAMESPACE: u8 = 0x41;
const INDEX_NAMESPACE: u8 = 0x42;
const CHANGE_NAMESPACE: u8 = 0x43;
const IDEMPOTENCY_NAMESPACE: u8 = 0x44;
const SEQUENCER_NAMESPACE: u8 = 0x45;
const REVISION_NAMESPACE: u8 = 0x46;
const INDEX_METADATA_NAMESPACE: u8 = 0x47;
const POLICY_NAMESPACE: u8 = 0x48;
const AUTHORIZATION_NAMESPACE: u8 = 0x49;
const IDENTITY_NAMESPACE: u8 = 0x4a;
const INTERNAL_RPC_NAMESPACE: u8 = 0x4b;
const BUCKET_NAMESPACE: u8 = 0x4c;
const BUCKET_RECORD: u8 = 0x01;
const BUCKET_OBJECT: u8 = 0x02;
const BUCKET_TOTALS: u8 = 0x03;
const INDEX_ENTRY: u8 = 0x10;
const INDEX_UNIQUE_OWNER: u8 = 0x11;
const INDEX_COMPONENT: u8 = 0x01;
const INDEX_DOCUMENT: u8 = 0x02;
const INDEX_CATALOG: u8 = 0x01;
const INDEX_BUILD_PROGRESS: u8 = 0x02;
const POLICY_VERSION: u8 = 0x01;
const POLICY_ACTIVE: u8 = 0x02;
const AUTHORIZATION_ENVIRONMENT_EPOCH: u8 = 0x01;
const AUTHORIZATION_USER_EPOCH: u8 = 0x02;
const AUTHORIZATION_INVALIDATION_SEQUENCE: u8 = 0x03;
const AUTHORIZATION_INVALIDATION: u8 = 0x04;
const IDENTITY_USER: u8 = 0x01;
const IDENTITY_LINK: u8 = 0x02;
const IDENTITY_CREDENTIAL: u8 = 0x03;
const IDENTITY_SESSION: u8 = 0x04;
const IDENTITY_TOKEN_FAMILY: u8 = 0x05;
const IDENTITY_EMAIL_OWNER: u8 = 0x06;
const IDENTITY_TOKEN_DIGEST_OWNER: u8 = 0x07;
const IDENTITY_USER_CREDENTIAL_OWNER: u8 = 0x08;
const IDENTITY_REVOCATION_SEQUENCE: u8 = 0x09;
const IDENTITY_REVOCATION_EVENT: u8 = 0x0a;
const IDENTITY_USER_SESSION: u8 = 0x0b;
const IDENTITY_PROJECT_CREDENTIAL: u8 = 0x0c;
const IDENTITY_USER_EMAIL: u8 = 0x0d;
const IDENTITY_SIGNING_KEY_RING: u8 = 0x0e;
const IDENTITY_PROVIDER_IDENTITY_OWNER: u8 = 0x0f;
const INTERNAL_RPC_NONCE: u8 = 0x01;
const INTERNAL_RPC_IDEMPOTENCY: u8 = 0x02;
const INTERNAL_RPC_RESPONSE: u8 = 0x03;
const SEGMENT_ESCAPE: u8 = 0x00;
const ESCAPED_ZERO: u8 = 0xff;
const SEGMENT_END: u8 = 0x00;
const MAX_SEGMENT_BYTES: usize = 16 * 1024;

/// A scoped encoder whose output cannot leave its project/environment key range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantKeyspace {
    project: Vec<u8>,
    environment: Vec<u8>,
    prefix: Vec<u8>,
}

impl TenantKeyspace {
    pub fn new(
        project: impl Into<Vec<u8>>,
        environment: impl Into<Vec<u8>>,
    ) -> Result<Self, KeyCodecError> {
        let project = project.into();
        let environment = environment.into();
        validate_identifier("project", &project)?;
        validate_identifier("environment", &environment)?;

        let mut prefix = vec![FORMAT_VERSION, PROJECT_NAMESPACE];
        encode_segment(&mut prefix, &project)?;
        prefix.push(ENVIRONMENT_NAMESPACE);
        encode_segment(&mut prefix, &environment)?;
        Ok(Self {
            project,
            environment,
            prefix,
        })
    }

    /// Every key this tenant owns, as one half-open range.
    ///
    /// Tenant separation is enforced by the encoded prefix, so this is what
    /// makes "how much does this tenant store" answerable without reading
    /// another tenant's keys.
    pub fn tenant_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.prefix)
    }

    #[must_use]
    pub fn project(&self) -> &[u8] {
        &self.project
    }

    #[must_use]
    pub fn environment(&self) -> &[u8] {
        &self.environment
    }

    #[must_use]
    pub fn environment_prefix(&self) -> &[u8] {
        &self.prefix
    }

    pub fn system_range() -> Result<KeyRange, KeyCodecError> {
        prefix_range(&[FORMAT_VERSION, SYSTEM_NAMESPACE])
    }

    pub fn system_key(
        domain: impl AsRef<[u8]>,
        item: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = vec![FORMAT_VERSION, SYSTEM_NAMESPACE];
        encode_required_segment(&mut key, "system domain", domain.as_ref())?;
        encode_required_segment(&mut key, "system item", item.as_ref())?;
        Ok(key)
    }

    pub fn system_domain_range(domain: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = vec![FORMAT_VERSION, SYSTEM_NAMESPACE];
        encode_required_segment(&mut prefix, "system domain", domain.as_ref())?;
        prefix_range(&prefix)
    }

    /// All system keys whose domain starts with `domain_prefix`. Unlike
    /// `system_domain_range`, this does not terminate the domain segment.
    /// Include a separator in the prefix when selecting a domain subtree.
    pub fn system_domain_prefix_range(
        domain_prefix: impl AsRef<[u8]>,
    ) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = vec![FORMAT_VERSION, SYSTEM_NAMESPACE];
        encode_required_segment(&mut prefix, "system domain prefix", domain_prefix.as_ref())?;
        prefix.truncate(prefix.len() - 2); // Remove the encoded segment terminator.
        prefix_range(&prefix)
    }

    pub fn project_range(project: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = vec![FORMAT_VERSION, PROJECT_NAMESPACE];
        encode_required_segment(&mut prefix, "project", project.as_ref())?;
        prefix_range(&prefix)
    }

    pub fn project_metadata_key(project: impl AsRef<[u8]>) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = vec![FORMAT_VERSION, PROJECT_NAMESPACE];
        encode_required_segment(&mut key, "project", project.as_ref())?;
        key.push(PROJECT_METADATA);
        Ok(key)
    }

    pub fn environment_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.prefix)
    }

    #[must_use]
    pub fn environment_metadata_key(&self) -> Vec<u8> {
        self.namespaced(ENVIRONMENT_METADATA)
    }

    /// Every bucket record in the environment.
    pub fn buckets_range(&self) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.namespaced(BUCKET_NAMESPACE);
        prefix.push(BUCKET_RECORD);
        prefix_range(&prefix)
    }

    pub fn bucket_key(&self, bucket: impl AsRef<[u8]>) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(BUCKET_NAMESPACE);
        key.push(BUCKET_RECORD);
        encode_required_segment(&mut key, "bucket", bucket.as_ref())?;
        Ok(key)
    }

    /// The running object count and byte total of one bucket.
    pub fn bucket_totals_key(&self, bucket: impl AsRef<[u8]>) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(BUCKET_NAMESPACE);
        key.push(BUCKET_TOTALS);
        encode_required_segment(&mut key, "bucket", bucket.as_ref())?;
        Ok(key)
    }

    /// Every bucket's totals record, for the environment's sampled level.
    pub fn bucket_totals_range(&self) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.namespaced(BUCKET_NAMESPACE);
        prefix.push(BUCKET_TOTALS);
        prefix_range(&prefix)
    }

    fn object_prefix(&self, bucket: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(BUCKET_NAMESPACE);
        prefix.push(BUCKET_OBJECT);
        encode_required_segment(&mut prefix, "bucket", bucket)?;
        Ok(prefix)
    }

    /// Every object metadata record of one bucket, in path order.
    pub fn objects_range(&self, bucket: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.object_prefix(bucket.as_ref())?)
    }

    pub fn object_key(
        &self,
        bucket: impl AsRef<[u8]>,
        path: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.object_prefix(bucket.as_ref())?;
        encode_required_segment(&mut key, "object", path.as_ref())?;
        Ok(key)
    }

    pub fn decode_object_key(
        &self,
        bucket: impl AsRef<[u8]>,
        key: &[u8],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let prefix = self.object_prefix(bucket.as_ref())?;
        let mut offset = require_prefix(key, &prefix)?;
        let path = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(path)
    }

    pub fn collections_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.namespaced(COLLECTION_NAMESPACE))
    }

    pub fn collection_key(&self, collection: impl AsRef<[u8]>) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(COLLECTION_NAMESPACE);
        encode_required_segment(&mut key, "collection", collection.as_ref())?;
        Ok(key)
    }

    pub fn documents_range(&self, collection: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.document_prefix(collection.as_ref())?)
    }

    pub fn document_key(
        &self,
        collection: impl AsRef<[u8]>,
        document: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.document_prefix(collection.as_ref())?;
        encode_required_segment(&mut key, "document", document.as_ref())?;
        Ok(key)
    }

    pub fn decode_document_key(
        &self,
        collection: impl AsRef<[u8]>,
        key: &[u8],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let prefix = self.document_prefix(collection.as_ref())?;
        let mut offset = require_prefix(key, &prefix)?;
        let document = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(document)
    }

    pub fn indexes_range(&self, collection: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        let prefix = self.index_prefix(collection.as_ref(), None)?;
        prefix_range(&prefix)
    }

    pub fn index_catalog_key(
        &self,
        collection: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.index_metadata_prefix(collection.as_ref())?;
        key.push(INDEX_CATALOG);
        Ok(key)
    }

    pub fn index_build_progress_key(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        version: u64,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.index_metadata_prefix(collection.as_ref())?;
        key.push(INDEX_BUILD_PROGRESS);
        encode_required_segment(&mut key, "index", index.as_ref())?;
        key.extend_from_slice(&version.to_be_bytes());
        Ok(key)
    }

    pub fn policy_versions_range(
        &self,
        collection: impl AsRef<[u8]>,
    ) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.policy_prefix(collection.as_ref())?;
        prefix.push(POLICY_VERSION);
        prefix_range(&prefix)
    }

    pub fn policy_version_key(
        &self,
        collection: impl AsRef<[u8]>,
        version: u64,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.policy_prefix(collection.as_ref())?;
        key.push(POLICY_VERSION);
        key.extend_from_slice(&version.to_be_bytes());
        Ok(key)
    }

    pub fn active_policy_key(
        &self,
        collection: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.policy_prefix(collection.as_ref())?;
        key.push(POLICY_ACTIVE);
        Ok(key)
    }

    #[must_use]
    pub fn authorization_environment_epoch_key(&self) -> Vec<u8> {
        let mut key = self.namespaced(AUTHORIZATION_NAMESPACE);
        key.push(AUTHORIZATION_ENVIRONMENT_EPOCH);
        key
    }

    pub fn authorization_user_epoch_key(
        &self,
        subject: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(AUTHORIZATION_NAMESPACE);
        key.push(AUTHORIZATION_USER_EPOCH);
        encode_required_segment(&mut key, "authorization subject", subject.as_ref())?;
        Ok(key)
    }

    pub fn explorer_developer_epoch_key(
        &self,
        developer_identity_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(AUTHORIZATION_NAMESPACE);
        key.push(0x20);
        encode_required_segment(
            &mut key,
            "explorer developer identity",
            developer_identity_id.as_ref(),
        )?;
        Ok(key)
    }

    pub fn explorer_developer_epochs_range(&self) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.namespaced(AUTHORIZATION_NAMESPACE);
        prefix.push(0x20);
        prefix_range(&prefix)
    }

    pub fn explorer_grant_key(&self, nonce: impl AsRef<[u8]>) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(AUTHORIZATION_NAMESPACE);
        key.push(0x21);
        encode_required_segment(&mut key, "explorer grant nonce", nonce.as_ref())?;
        Ok(key)
    }

    #[must_use]
    pub fn authorization_invalidation_sequence_key(&self) -> Vec<u8> {
        let mut key = self.namespaced(AUTHORIZATION_NAMESPACE);
        key.push(AUTHORIZATION_INVALIDATION_SEQUENCE);
        key
    }

    pub fn authorization_invalidations_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.authorization_invalidation_prefix())
    }

    pub fn authorization_invalidations_after(
        &self,
        sequence: u64,
    ) -> Result<Option<KeyRange>, KeyCodecError> {
        let Some(next) = sequence.checked_add(1) else {
            return Ok(None);
        };
        let prefix = self.authorization_invalidation_prefix();
        let mut start = prefix.clone();
        start.extend_from_slice(&next.to_be_bytes());
        let end = prefix_successor(&prefix).ok_or(KeyCodecError::PrefixHasNoSuccessor)?;
        Ok(Some(
            KeyRange::new(start, end).map_err(|_| KeyCodecError::MalformedKey)?,
        ))
    }

    #[must_use]
    pub fn authorization_invalidation_key(&self, sequence: u64) -> Vec<u8> {
        let mut key = self.authorization_invalidation_prefix();
        key.extend_from_slice(&sequence.to_be_bytes());
        key
    }

    pub fn decode_authorization_invalidation_key(&self, key: &[u8]) -> Result<u64, KeyCodecError> {
        let prefix = self.authorization_invalidation_prefix();
        let offset = require_prefix(key, &prefix)?;
        let encoded: [u8; 8] = key
            .get(offset..)
            .ok_or(KeyCodecError::MalformedKey)?
            .try_into()
            .map_err(|_| KeyCodecError::MalformedKey)?;
        Ok(u64::from_be_bytes(encoded))
    }

    pub fn application_user_key(
        &self,
        user_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_USER, "user id", user_id.as_ref())
    }

    pub fn application_users_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.identity_record_prefix(IDENTITY_USER))
    }

    pub fn decode_application_user_key(&self, key: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        self.decode_identity_record_key(IDENTITY_USER, key)
    }

    pub fn application_identity_key(
        &self,
        identity_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_LINK, "identity id", identity_id.as_ref())
    }

    pub fn application_credential_key(
        &self,
        credential_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_CREDENTIAL, "credential id", credential_id.as_ref())
    }

    pub fn application_session_key(
        &self,
        session_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_SESSION, "session id", session_id.as_ref())
    }

    pub fn application_token_family_key(
        &self,
        family_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_TOKEN_FAMILY, "token family id", family_id.as_ref())
    }

    pub fn project_credential_key(
        &self,
        credential_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(
            IDENTITY_PROJECT_CREDENTIAL,
            "project credential id",
            credential_id.as_ref(),
        )
    }

    pub fn project_credentials_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.identity_record_prefix(IDENTITY_PROJECT_CREDENTIAL))
    }

    /// One tenant-bound application signing-key ring. The encrypted private
    /// records remain inside the data-plane-owned identity namespace.
    #[must_use]
    pub fn project_signing_key_ring_key(&self) -> Vec<u8> {
        self.identity_record_prefix(IDENTITY_SIGNING_KEY_RING)
    }

    pub fn internal_rpc_nonce_key(
        &self,
        caller: impl AsRef<[u8]>,
        nonce: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.internal_rpc_key(INTERNAL_RPC_NONCE, caller.as_ref(), nonce.as_ref())
    }

    pub fn internal_rpc_idempotency_key(
        &self,
        caller: impl AsRef<[u8]>,
        idempotency_key: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.internal_rpc_key(
            INTERNAL_RPC_IDEMPOTENCY,
            caller.as_ref(),
            idempotency_key.as_ref(),
        )
    }

    pub fn internal_rpc_response_key(
        &self,
        caller: impl AsRef<[u8]>,
        idempotency_key: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.internal_rpc_key(
            INTERNAL_RPC_RESPONSE,
            caller.as_ref(),
            idempotency_key.as_ref(),
        )
    }

    pub fn normalized_email_owner_key(
        &self,
        normalized_email: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(
            IDENTITY_EMAIL_OWNER,
            "normalized email",
            normalized_email.as_ref(),
        )
    }

    pub fn normalized_email_owners_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.identity_record_prefix(IDENTITY_EMAIL_OWNER))
    }

    pub fn decode_normalized_email_owner_key(&self, key: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        self.decode_identity_record_key(IDENTITY_EMAIL_OWNER, key)
    }

    pub fn application_user_email_key(
        &self,
        user_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(IDENTITY_USER_EMAIL, "user id", user_id.as_ref())
    }

    pub fn identity_token_digest_owner_key(
        &self,
        digest: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        self.identity_record_key(
            IDENTITY_TOKEN_DIGEST_OWNER,
            "identity token digest",
            digest.as_ref(),
        )
    }

    pub fn application_user_credential_owner_key(
        &self,
        user_id: impl AsRef<[u8]>,
        kind: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(IDENTITY_NAMESPACE);
        key.push(IDENTITY_USER_CREDENTIAL_OWNER);
        encode_required_segment(&mut key, "user id", user_id.as_ref())?;
        encode_required_segment(&mut key, "credential kind", kind.as_ref())?;
        Ok(key)
    }

    /// Owner index from one external sign-in identity (provider name plus the
    /// provider's subject) to the application user it is linked to. Both
    /// segments are required, so a subject can never be confused across
    /// providers and an empty subject can never claim a whole provider.
    pub fn provider_identity_owner_key(
        &self,
        provider: impl AsRef<[u8]>,
        subject: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.identity_record_prefix(IDENTITY_PROVIDER_IDENTITY_OWNER);
        encode_required_segment(&mut key, "provider name", provider.as_ref())?;
        encode_required_segment(&mut key, "provider subject", subject.as_ref())?;
        Ok(key)
    }

    pub fn provider_identities_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.identity_record_prefix(IDENTITY_PROVIDER_IDENTITY_OWNER))
    }

    #[must_use]
    pub fn identity_revocation_sequence_key(&self) -> Vec<u8> {
        let mut key = self.namespaced(IDENTITY_NAMESPACE);
        key.push(IDENTITY_REVOCATION_SEQUENCE);
        key
    }

    pub fn identity_revocations_after(
        &self,
        sequence: u64,
    ) -> Result<Option<KeyRange>, KeyCodecError> {
        let Some(next) = sequence.checked_add(1) else {
            return Ok(None);
        };
        let prefix = self.identity_revocation_prefix();
        let mut start = prefix.clone();
        start.extend_from_slice(&next.to_be_bytes());
        let end = prefix_successor(&prefix).ok_or(KeyCodecError::PrefixHasNoSuccessor)?;
        Ok(Some(
            KeyRange::new(start, end).map_err(|_| KeyCodecError::MalformedKey)?,
        ))
    }

    #[must_use]
    pub fn identity_revocation_key(&self, sequence: u64) -> Vec<u8> {
        let mut key = self.identity_revocation_prefix();
        key.extend_from_slice(&sequence.to_be_bytes());
        key
    }

    pub fn decode_identity_revocation_key(&self, key: &[u8]) -> Result<u64, KeyCodecError> {
        let prefix = self.identity_revocation_prefix();
        let offset = require_prefix(key, &prefix)?;
        let encoded: [u8; 8] = key
            .get(offset..)
            .ok_or(KeyCodecError::MalformedKey)?
            .try_into()
            .map_err(|_| KeyCodecError::MalformedKey)?;
        Ok(u64::from_be_bytes(encoded))
    }

    pub fn application_user_session_key(
        &self,
        user_id: impl AsRef<[u8]>,
        session_id: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.application_user_session_prefix(user_id.as_ref())?;
        encode_required_segment(&mut key, "session id", session_id.as_ref())?;
        Ok(key)
    }

    pub fn application_user_sessions_range(
        &self,
        user_id: impl AsRef<[u8]>,
    ) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.application_user_session_prefix(user_id.as_ref())?)
    }

    pub fn index_range(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
    ) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_ENTRY);
        prefix_range(&prefix)
    }

    pub fn unique_index_owners_range(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
    ) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_UNIQUE_OWNER);
        prefix_range(&prefix)
    }

    pub fn index_value_range(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        components: &[impl AsRef<[u8]>],
    ) -> Result<KeyRange, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_ENTRY);
        encode_index_components(&mut prefix, components)?;
        prefix.push(INDEX_DOCUMENT);
        prefix_range(&prefix)
    }

    pub fn index_entry_key(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        components: &[impl AsRef<[u8]>],
        document: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        if components.is_empty() {
            return Err(KeyCodecError::InvalidIdentifier {
                field: "index components",
                reason: "at least one component is required",
            });
        }
        let mut key = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        key.push(INDEX_ENTRY);
        encode_index_components(&mut key, components)?;
        key.push(INDEX_DOCUMENT);
        encode_required_segment(&mut key, "document", document.as_ref())?;
        Ok(key)
    }

    pub fn unique_index_owner_key(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        components: &[impl AsRef<[u8]>],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        key.push(INDEX_UNIQUE_OWNER);
        encode_index_components(&mut key, components)?;
        Ok(key)
    }

    pub fn index_components_prefix(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        components: &[impl AsRef<[u8]>],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_ENTRY);
        encode_index_components(&mut prefix, components)?;
        Ok(prefix)
    }

    /// Every entry of one index, whatever its components.
    ///
    /// An index *entry* always has at least one component, so
    /// `index_components_prefix` refuses an empty list. A caller that wants
    /// the whole index -- a range over its leading field, with no equality
    /// prefix to stand on -- needs the prefix the entries share and nothing
    /// more, which is what this is.
    pub fn index_entries_prefix(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_ENTRY);
        Ok(prefix)
    }

    pub fn prefixed_range(prefix: &[u8]) -> Result<KeyRange, KeyCodecError> {
        prefix_range(prefix)
    }

    pub fn decode_index_entry_key(
        &self,
        collection: impl AsRef<[u8]>,
        index: impl AsRef<[u8]>,
        key: &[u8],
    ) -> Result<DecodedIndexKey, KeyCodecError> {
        let mut prefix = self.index_prefix(collection.as_ref(), Some(index.as_ref()))?;
        prefix.push(INDEX_ENTRY);
        let mut offset = require_prefix(key, &prefix)?;
        let mut components = Vec::new();
        loop {
            let marker = *key.get(offset).ok_or(KeyCodecError::MalformedKey)?;
            offset += 1;
            match marker {
                INDEX_COMPONENT => components.push(decode_segment(key, &mut offset)?),
                INDEX_DOCUMENT if !components.is_empty() => break,
                _ => return Err(KeyCodecError::MalformedKey),
            }
        }
        let document = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(DecodedIndexKey {
            components,
            document,
        })
    }

    pub fn changes_range(&self, collection: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.change_prefix(collection.as_ref())?)
    }

    pub fn changes_after_through(
        &self,
        collection: impl AsRef<[u8]>,
        after: u64,
        through: u64,
    ) -> Result<KeyRange, KeyCodecError> {
        if after >= through {
            return Err(KeyCodecError::InvalidIdentifier {
                field: "change position range",
                reason: "after must be less than through",
            });
        }
        let prefix = self.change_prefix(collection.as_ref())?;
        let mut start = prefix.clone();
        start.extend_from_slice(&after.saturating_add(1).to_be_bytes());
        let end = match through.checked_add(1) {
            Some(end_position) => {
                let mut end = prefix;
                end.extend_from_slice(&end_position.to_be_bytes());
                end
            }
            None => prefix_successor(&prefix).ok_or(KeyCodecError::PrefixHasNoSuccessor)?,
        };
        KeyRange::new(start, end).map_err(|_| KeyCodecError::MalformedKey)
    }

    pub fn revisions_range(&self, collection: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.revision_prefix(collection.as_ref())?)
    }

    pub fn revision_key(
        &self,
        collection: impl AsRef<[u8]>,
        position: u64,
        document: impl AsRef<[u8]>,
        revision: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.revision_prefix(collection.as_ref())?;
        key.extend_from_slice(&position.to_be_bytes());
        encode_required_segment(&mut key, "document", document.as_ref())?;
        encode_required_segment(&mut key, "revision", revision.as_ref())?;
        Ok(key)
    }

    pub fn decode_revision_key(
        &self,
        collection: impl AsRef<[u8]>,
        key: &[u8],
    ) -> Result<DecodedRevisionKey, KeyCodecError> {
        let prefix = self.revision_prefix(collection.as_ref())?;
        let mut offset = require_prefix(key, &prefix)?;
        let position_bytes: [u8; 8] = key
            .get(offset..offset + 8)
            .ok_or(KeyCodecError::MalformedKey)?
            .try_into()
            .map_err(|_| KeyCodecError::MalformedKey)?;
        offset += 8;
        let document = decode_segment(key, &mut offset)?;
        let revision = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(DecodedRevisionKey {
            position: u64::from_be_bytes(position_bytes),
            document,
            revision,
        })
    }

    pub fn change_key(
        &self,
        collection: impl AsRef<[u8]>,
        position: u64,
        document: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.change_prefix(collection.as_ref())?;
        key.extend_from_slice(&position.to_be_bytes());
        encode_required_segment(&mut key, "document", document.as_ref())?;
        Ok(key)
    }

    pub fn decode_change_key(
        &self,
        collection: impl AsRef<[u8]>,
        key: &[u8],
    ) -> Result<DecodedChangeKey, KeyCodecError> {
        let prefix = self.change_prefix(collection.as_ref())?;
        let mut offset = require_prefix(key, &prefix)?;
        let position_bytes: [u8; 8] = key
            .get(offset..offset + 8)
            .ok_or(KeyCodecError::MalformedKey)?
            .try_into()
            .map_err(|_| KeyCodecError::MalformedKey)?;
        offset += 8;
        let document = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(DecodedChangeKey {
            position: u64::from_be_bytes(position_bytes),
            document,
        })
    }

    pub fn idempotency_range(&self, domain: impl AsRef<[u8]>) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.idempotency_prefix(domain.as_ref())?)
    }

    pub fn idempotency_key(
        &self,
        domain: impl AsRef<[u8]>,
        token: impl AsRef<[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.idempotency_prefix(domain.as_ref())?;
        encode_required_segment(&mut key, "idempotency token", token.as_ref())?;
        Ok(key)
    }

    pub fn sequencer_range(&self) -> Result<KeyRange, KeyCodecError> {
        prefix_range(&self.namespaced(SEQUENCER_NAMESPACE))
    }

    #[must_use]
    pub fn sequencer_key(&self, kind: SequencerKeyKind) -> Vec<u8> {
        let mut key = self.namespaced(SEQUENCER_NAMESPACE);
        key.push(kind as u8);
        key
    }

    #[must_use]
    pub fn sequencer_position_key(&self, kind: SequencerKeyKind, position: u64) -> Vec<u8> {
        let mut key = self.sequencer_key(kind);
        key.extend_from_slice(&position.to_be_bytes());
        key
    }

    fn namespaced(&self, namespace: u8) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.push(namespace);
        key
    }

    fn document_prefix(&self, collection: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(DOCUMENT_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        Ok(prefix)
    }

    fn index_prefix(
        &self,
        collection: &[u8],
        index: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(INDEX_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        if let Some(index) = index {
            encode_required_segment(&mut prefix, "index", index)?;
        }
        Ok(prefix)
    }

    fn index_metadata_prefix(&self, collection: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(INDEX_METADATA_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        Ok(prefix)
    }

    fn policy_prefix(&self, collection: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(POLICY_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        Ok(prefix)
    }

    fn authorization_invalidation_prefix(&self) -> Vec<u8> {
        let mut prefix = self.namespaced(AUTHORIZATION_NAMESPACE);
        prefix.push(AUTHORIZATION_INVALIDATION);
        prefix
    }

    fn identity_record_key(
        &self,
        record_type: u8,
        field: &'static str,
        identifier: &[u8],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.identity_record_prefix(record_type);
        encode_required_segment(&mut key, field, identifier)?;
        Ok(key)
    }

    fn identity_record_prefix(&self, record_type: u8) -> Vec<u8> {
        let mut prefix = self.namespaced(IDENTITY_NAMESPACE);
        prefix.push(record_type);
        prefix
    }

    fn internal_rpc_key(
        &self,
        record_type: u8,
        caller: &[u8],
        identifier: &[u8],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let mut key = self.namespaced(INTERNAL_RPC_NAMESPACE);
        key.push(record_type);
        encode_required_segment(&mut key, "internal RPC caller", caller)?;
        encode_required_segment(&mut key, "internal RPC identifier", identifier)?;
        Ok(key)
    }

    fn decode_identity_record_key(
        &self,
        record_type: u8,
        key: &[u8],
    ) -> Result<Vec<u8>, KeyCodecError> {
        let prefix = self.identity_record_prefix(record_type);
        let mut offset = require_prefix(key, &prefix)?;
        let identifier = decode_segment(key, &mut offset)?;
        require_end(key, offset)?;
        Ok(identifier)
    }

    fn identity_revocation_prefix(&self) -> Vec<u8> {
        let mut prefix = self.namespaced(IDENTITY_NAMESPACE);
        prefix.push(IDENTITY_REVOCATION_EVENT);
        prefix
    }

    fn application_user_session_prefix(&self, user_id: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(IDENTITY_NAMESPACE);
        prefix.push(IDENTITY_USER_SESSION);
        encode_required_segment(&mut prefix, "user id", user_id)?;
        Ok(prefix)
    }

    fn change_prefix(&self, collection: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(CHANGE_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        Ok(prefix)
    }

    fn revision_prefix(&self, collection: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(REVISION_NAMESPACE);
        encode_required_segment(&mut prefix, "collection", collection)?;
        Ok(prefix)
    }

    fn idempotency_prefix(&self, domain: &[u8]) -> Result<Vec<u8>, KeyCodecError> {
        let mut prefix = self.namespaced(IDEMPOTENCY_NAMESPACE);
        encode_required_segment(&mut prefix, "idempotency domain", domain)?;
        Ok(prefix)
    }
}

pub(crate) type TenantIdentity = (Vec<u8>, Vec<u8>);

pub(crate) fn decode_tenant_identity(key: &[u8]) -> Result<Option<TenantIdentity>, KeyCodecError> {
    if key.get(..2) != Some(&[FORMAT_VERSION, PROJECT_NAMESPACE]) {
        return Ok(None);
    }
    let mut offset = 2;
    let project = decode_segment(key, &mut offset)?;
    if key.get(offset) != Some(&ENVIRONMENT_NAMESPACE) {
        return Ok(None);
    }
    offset += 1;
    let environment = decode_segment(key, &mut offset)?;
    Ok(Some((project, environment)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SequencerKeyKind {
    HighWater = 0x01,
    Lease = 0x02,
    Committed = 0x03,
    Aborted = 0x04,
    RetentionWatermark = 0x05,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedChangeKey {
    pub position: u64,
    pub document: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedIndexKey {
    pub components: Vec<Vec<u8>>,
    pub document: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedRevisionKey {
    pub position: u64,
    pub document: Vec<u8>,
    pub revision: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyCodecError {
    InvalidIdentifier {
        field: &'static str,
        reason: &'static str,
    },
    MalformedKey,
    PrefixHasNoSuccessor,
}

impl fmt::Display for KeyCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { field, reason } => {
                write!(formatter, "invalid {field}: {reason}")
            }
            Self::MalformedKey => formatter.write_str("malformed encoded storage key"),
            Self::PrefixHasNoSuccessor => {
                formatter.write_str("encoded prefix has no lexicographic successor")
            }
        }
    }
}

impl Error for KeyCodecError {}

fn validate_identifier(field: &'static str, value: &[u8]) -> Result<(), KeyCodecError> {
    if value.is_empty() {
        return Err(KeyCodecError::InvalidIdentifier {
            field,
            reason: "cannot be empty",
        });
    }
    if value.len() > MAX_SEGMENT_BYTES {
        return Err(KeyCodecError::InvalidIdentifier {
            field,
            reason: "exceeds 16 KiB",
        });
    }
    Ok(())
}

fn encode_required_segment(
    output: &mut Vec<u8>,
    field: &'static str,
    value: &[u8],
) -> Result<(), KeyCodecError> {
    validate_identifier(field, value)?;
    encode_segment(output, value)
}

fn encode_segment(output: &mut Vec<u8>, value: &[u8]) -> Result<(), KeyCodecError> {
    if value.len() > MAX_SEGMENT_BYTES {
        return Err(KeyCodecError::InvalidIdentifier {
            field: "key segment",
            reason: "exceeds 16 KiB",
        });
    }
    output.reserve(value.len() + 2);
    for byte in value {
        if *byte == SEGMENT_ESCAPE {
            output.extend_from_slice(&[SEGMENT_ESCAPE, ESCAPED_ZERO]);
        } else {
            output.push(*byte);
        }
    }
    output.extend_from_slice(&[SEGMENT_ESCAPE, SEGMENT_END]);
    Ok(())
}

fn decode_segment(key: &[u8], offset: &mut usize) -> Result<Vec<u8>, KeyCodecError> {
    let mut decoded = Vec::new();
    loop {
        let byte = *key.get(*offset).ok_or(KeyCodecError::MalformedKey)?;
        *offset += 1;
        if byte != SEGMENT_ESCAPE {
            decoded.push(byte);
            continue;
        }
        match key.get(*offset).copied() {
            Some(ESCAPED_ZERO) => {
                decoded.push(SEGMENT_ESCAPE);
                *offset += 1;
            }
            Some(SEGMENT_END) => {
                *offset += 1;
                return Ok(decoded);
            }
            _ => return Err(KeyCodecError::MalformedKey),
        }
    }
}

fn encode_index_components<T: AsRef<[u8]>>(
    key: &mut Vec<u8>,
    components: &[T],
) -> Result<(), KeyCodecError> {
    if components.is_empty() {
        return Err(KeyCodecError::InvalidIdentifier {
            field: "index components",
            reason: "at least one component is required",
        });
    }
    for component in components {
        key.push(INDEX_COMPONENT);
        encode_segment(key, component.as_ref())?;
    }
    Ok(())
}

fn prefix_range(prefix: &[u8]) -> Result<KeyRange, KeyCodecError> {
    let end = prefix_successor(prefix).ok_or(KeyCodecError::PrefixHasNoSuccessor)?;
    KeyRange::new(prefix.to_vec(), end).map_err(|_| KeyCodecError::PrefixHasNoSuccessor)
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut successor = prefix.to_vec();
    for index in (0..successor.len()).rev() {
        if successor[index] != u8::MAX {
            successor[index] += 1;
            successor.truncate(index + 1);
            return Some(successor);
        }
    }
    None
}

fn require_prefix(key: &[u8], prefix: &[u8]) -> Result<usize, KeyCodecError> {
    key.starts_with(prefix)
        .then_some(prefix.len())
        .ok_or(KeyCodecError::MalformedKey)
}

fn require_end(key: &[u8], offset: usize) -> Result<(), KeyCodecError> {
    if offset == key.len() {
        Ok(())
    } else {
        Err(KeyCodecError::MalformedKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_domain_prefix_ranges_include_descendants_without_widening_exact_ranges() {
        for prefix in [b"control/jobs/".as_slice(), b"control/jo\0bs/".as_slice()] {
            let range = TenantKeyspace::system_domain_prefix_range(prefix).expect("prefix range");
            for suffix in [b"".as_slice(), b"project/env", b"project/env/index/state"] {
                let domain = [prefix, suffix].concat();
                let key = TenantKeyspace::system_key(&domain, "job").expect("key");
                assert!(range.contains(&key));
                let exact = TenantKeyspace::system_domain_range(prefix).expect("exact range");
                assert_eq!(exact.contains(&key), suffix.is_empty());
            }
            for domain in [
                prefix[..prefix.len() - 1].to_vec(),
                [
                    prefix[..prefix.len() - 1].to_vec(),
                    b"-other/project/env".to_vec(),
                ]
                .concat(),
                b"unrelated".to_vec(),
            ] {
                assert!(!range.contains(&TenantKeyspace::system_key(domain, "job").expect("key")));
            }
        }
        assert!(TenantKeyspace::system_domain_prefix_range(b"").is_err());
        assert!(
            TenantKeyspace::system_domain_prefix_range(vec![b'x'; MAX_SEGMENT_BYTES + 1]).is_err()
        );
    }

    #[test]
    fn bucket_and_object_keys_stay_inside_their_bucket_and_decode_back() {
        let keyspace = TenantKeyspace::new("prj_a", "env_a").expect("keyspace");
        let bucket = keyspace.bucket_key("avatars").expect("bucket key");
        let totals = keyspace.bucket_totals_key("avatars").expect("totals key");
        let object = keyspace
            .object_key("avatars", "users/42/me.png")
            .expect("object key");
        assert_ne!(bucket, totals);
        assert_ne!(bucket, object);
        let buckets = keyspace.buckets_range().expect("buckets range");
        assert!(buckets.contains(&bucket));
        assert!(
            !buckets.contains(&object),
            "objects live outside the bucket records"
        );
        let objects = keyspace.objects_range("avatars").expect("objects range");
        assert!(objects.contains(&object));
        assert!(!objects.contains(&bucket));
        let other = keyspace
            .object_key("avatars-2", "users/42/me.png")
            .expect("other bucket");
        assert!(
            !objects.contains(&other),
            "a bucket's range never reaches a sibling"
        );
        assert_eq!(
            keyspace
                .decode_object_key("avatars", &object)
                .expect("decode"),
            b"users/42/me.png".to_vec()
        );
        assert!(keyspace.decode_object_key("avatars-2", &object).is_err());
        let mut sorted = [
            keyspace.object_key("avatars", "b").expect("b"),
            keyspace.object_key("avatars", "a/z").expect("a/z"),
            keyspace.object_key("avatars", "a").expect("a"),
        ];
        sorted.sort();
        let paths: Vec<_> = sorted
            .iter()
            .map(|key| keyspace.decode_object_key("avatars", key).expect("decode"))
            .collect();
        assert_eq!(
            paths,
            vec![b"a".to_vec(), b"a/z".to_vec(), b"b".to_vec()],
            "object keys sort by path"
        );
    }

    #[test]
    fn arbitrary_separator_bytes_round_trip_without_collision() {
        let tenant =
            TenantKeyspace::new(b"project\0/".to_vec(), b"env\xff".to_vec()).expect("valid tenant");
        let document = b"doc\0/\xff".to_vec();
        let key = tenant
            .document_key(b"collection\0/", &document)
            .expect("encode document key");

        assert_eq!(
            tenant
                .decode_document_key(b"collection\0/", &key)
                .expect("decode document key"),
            document
        );
    }

    #[test]
    fn namespace_and_tenant_ranges_are_disjoint() {
        let first =
            TenantKeyspace::new(b"project-a".to_vec(), b"env".to_vec()).expect("valid tenant");
        let second =
            TenantKeyspace::new(b"project-a\0".to_vec(), b"env".to_vec()).expect("valid tenant");
        let first_key = first.document_key(b"items", b"one").expect("document key");
        let second_key = second.document_key(b"items", b"one").expect("document key");

        assert!(
            first
                .environment_range()
                .expect("range")
                .contains(&first_key)
        );
        assert!(
            !first
                .environment_range()
                .expect("range")
                .contains(&second_key)
        );
        assert!(
            !TenantKeyspace::system_range()
                .expect("system range")
                .contains(&first_key)
        );
    }

    #[test]
    fn change_positions_sort_numerically_then_by_document() {
        let tenant =
            TenantKeyspace::new(b"project".to_vec(), b"env".to_vec()).expect("valid tenant");
        let early = tenant.change_key(b"items", 9, b"z").expect("change key");
        let late_a = tenant.change_key(b"items", 10, b"a").expect("change key");
        let late_b = tenant.change_key(b"items", 10, b"b").expect("change key");

        assert!(early < late_a);
        assert!(late_a < late_b);
        assert_eq!(
            tenant.decode_change_key(b"items", &late_b).expect("decode"),
            DecodedChangeKey {
                position: 10,
                document: b"b".to_vec()
            }
        );
    }

    #[test]
    fn revision_keys_are_ordered_by_position_and_round_trip() {
        let tenant =
            TenantKeyspace::new(b"project".to_vec(), b"env".to_vec()).expect("valid tenant");
        let early = tenant
            .revision_key(b"items", 9, b"doc", b"rev-a")
            .expect("revision key");
        let late = tenant
            .revision_key(b"items", 10, b"doc\0", b"rev-b")
            .expect("revision key");

        assert!(early < late);
        assert!(
            tenant
                .revisions_range(b"items")
                .expect("range")
                .contains(&late)
        );
        assert_eq!(
            tenant.decode_revision_key(b"items", &late).expect("decode"),
            DecodedRevisionKey {
                position: 10,
                document: b"doc\0".to_vec(),
                revision: b"rev-b".to_vec(),
            }
        );
    }

    #[test]
    fn index_tuples_round_trip_and_order() {
        let tenant =
            TenantKeyspace::new(b"project".to_vec(), b"env".to_vec()).expect("valid tenant");
        let components = [b"last\0name".as_slice(), b"first".as_slice()];
        let key = tenant
            .index_entry_key(b"people", b"by-name", &components, b"doc-1")
            .expect("index key");
        let decoded = tenant
            .decode_index_entry_key(b"people", b"by-name", &key)
            .expect("decode index key");

        assert_eq!(decoded.components, components.map(<[u8]>::to_vec));
        assert_eq!(decoded.document, b"doc-1");
        assert!(
            tenant
                .index_value_range(b"people", b"by-name", &components)
                .expect("value range")
                .contains(&key)
        );

        let owner = tenant
            .unique_index_owner_key(b"people", b"by-name", &components)
            .expect("owner key");
        assert!(
            tenant
                .unique_index_owners_range(b"people", b"by-name")
                .expect("owner range")
                .contains(&owner)
        );
        assert!(
            !tenant
                .index_range(b"people", b"by-name")
                .expect("entry range")
                .contains(&owner)
        );
    }

    #[test]
    fn authorization_epochs_and_invalidations_are_scoped_and_ordered() {
        let tenant =
            TenantKeyspace::new(b"project".to_vec(), b"env".to_vec()).expect("valid tenant");
        let other =
            TenantKeyspace::new(b"project".to_vec(), b"other".to_vec()).expect("valid tenant");
        let first = tenant.authorization_invalidation_key(9);
        let second = tenant.authorization_invalidation_key(10);

        assert!(first < second);
        assert_eq!(
            tenant
                .decode_authorization_invalidation_key(&second)
                .expect("decode invalidation"),
            10
        );
        assert!(
            tenant
                .authorization_invalidations_after(9)
                .expect("range")
                .expect("non-empty")
                .contains(&second)
        );
        assert!(
            !tenant
                .environment_range()
                .expect("tenant range")
                .contains(&other.authorization_environment_epoch_key())
        );
        assert_ne!(
            tenant
                .authorization_user_epoch_key(b"user-a")
                .expect("user epoch"),
            tenant
                .authorization_user_epoch_key(b"user-b")
                .expect("user epoch")
        );
    }

    #[test]
    fn identity_records_and_email_owners_are_environment_scoped() {
        let first = TenantKeyspace::new(b"project".to_vec(), b"env-a".to_vec()).expect("tenant");
        let second = TenantKeyspace::new(b"project".to_vec(), b"env-b".to_vec()).expect("tenant");
        let email = first
            .normalized_email_owner_key(b"person@example.com")
            .expect("email key");

        assert!(first.environment_range().expect("range").contains(&email));
        assert_ne!(
            email,
            second
                .normalized_email_owner_key(b"person@example.com")
                .expect("email key")
        );
        assert_ne!(
            first.application_user_key(b"record-a").expect("user"),
            first
                .application_identity_key(b"record-a")
                .expect("identity")
        );

        let provider = first
            .provider_identity_owner_key(b"google", b"subject-a")
            .expect("provider key");
        assert!(
            first
                .environment_range()
                .expect("range")
                .contains(&provider)
        );
        assert!(
            first
                .provider_identities_range()
                .expect("provider range")
                .contains(&provider)
        );
        assert!(
            !first
                .normalized_email_owners_range()
                .expect("email range")
                .contains(&provider)
        );
        assert_ne!(
            provider,
            second
                .provider_identity_owner_key(b"google", b"subject-a")
                .expect("other environment")
        );
        assert_ne!(
            provider,
            first
                .provider_identity_owner_key(b"github", b"subject-a")
                .expect("other provider")
        );
        assert_ne!(
            provider,
            first
                .provider_identity_owner_key(b"google", b"subject-b")
                .expect("other subject")
        );
        assert_ne!(
            first
                .provider_identity_owner_key(b"goog", b"le-subject")
                .expect("shifted boundary"),
            first
                .provider_identity_owner_key(b"google", b"subject")
                .expect("provider key")
        );
        assert!(first.provider_identity_owner_key(b"google", b"").is_err());
        assert!(
            first
                .provider_identity_owner_key(b"", b"subject-a")
                .is_err()
        );
    }

    #[test]
    fn identity_revocations_and_user_sessions_are_scoped_and_ordered() {
        let tenant = TenantKeyspace::new(b"project".to_vec(), b"env".to_vec()).expect("tenant");
        let first = tenant.identity_revocation_key(9);
        let second = tenant.identity_revocation_key(10);
        let session = tenant
            .application_user_session_key(b"user-a", b"session-a")
            .expect("session index");

        assert!(first < second);
        assert_eq!(
            tenant
                .decode_identity_revocation_key(&second)
                .expect("decode"),
            10
        );
        assert!(
            tenant
                .identity_revocations_after(9)
                .expect("range")
                .expect("non-empty")
                .contains(&second)
        );
        assert!(
            tenant
                .application_user_sessions_range(b"user-a")
                .expect("user sessions")
                .contains(&session)
        );
        assert!(
            !tenant
                .application_user_sessions_range(b"user-b")
                .expect("other user sessions")
                .contains(&session)
        );
    }

    #[test]
    fn internal_response_journals_are_tenant_caller_and_domain_separated() {
        let first = TenantKeyspace::new(b"project".to_vec(), b"env-a".to_vec()).expect("tenant");
        let second = TenantKeyspace::new(b"project".to_vec(), b"env-b".to_vec()).expect("tenant");
        let response = first
            .internal_rpc_response_key(b"control-plane", b"idem-a")
            .expect("response");

        assert!(
            first
                .environment_range()
                .expect("range")
                .contains(&response)
        );
        assert_ne!(
            response,
            first
                .internal_rpc_idempotency_key(b"control-plane", b"idem-a")
                .expect("idempotency")
        );
        assert_ne!(
            response,
            first
                .internal_rpc_response_key(b"edge-gateway", b"idem-a")
                .expect("caller")
        );
        assert_ne!(
            response,
            second
                .internal_rpc_response_key(b"control-plane", b"idem-a")
                .expect("other tenant")
        );
    }
}
