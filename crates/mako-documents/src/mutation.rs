use std::{error::Error, fmt};

use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, SequencerKeyKind, StorageError,
    WriteBatch,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CanonicalDocument, CommitPosition, DocumentCodecError, DocumentEngineScopeError, DocumentId,
    DocumentMutationAuthorizer, IndexDefinition, IndexError, IndexName, IndexVersion,
    MutationAuthorizationContext, MutationAuthorizationOperation, RevisionToken,
    ScopedCollectionEngine, ValidatedDocumentBody,
    document::validate_token,
    index::IndexCatalog,
    sequencer::{STATUS_MARKER, SequencerError, decode_position},
};

const CHANGE_RECORD_ENCODING_VERSION: u8 = 2;
const IDEMPOTENCY_RECORD_ENCODING_VERSION: u8 = 1;
pub(crate) const DOCUMENT_MUTATION_DOMAIN: &[u8] = b"document-mutation";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MutationId(String);

impl MutationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, MutationError> {
        let value = value.into();
        validate_token("mutation_id", &value).map_err(MutationError::Document)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn revision_for(&self, document_id: &DocumentId) -> RevisionToken {
        let mut hasher = blake3::Hasher::new();
        hash_part(&mut hasher, b"mako-document-revision-v1");
        hash_part(&mut hasher, self.as_str().as_bytes());
        hash_part(&mut hasher, document_id.as_str().as_bytes());
        RevisionToken::parse(format!("r1_{}", hasher.finalize().to_hex()))
            .expect("a BLAKE3 revision token is always valid")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedRevision {
    Missing,
    Exact(RevisionToken),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IndexDelta {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

impl IndexDelta {
    #[must_use]
    pub fn put(key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Self {
        Self::Put {
            key: key.into(),
            value: value.into(),
        }
    }

    #[must_use]
    pub fn delete(key: impl Into<Vec<u8>>) -> Self {
        Self::Delete { key: key.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationRequest {
    pub mutation_id: MutationId,
    pub expected_revision: ExpectedRevision,
    pub revision: RevisionToken,
    pub commit_position: CommitPosition,
    pub deleted: bool,
    pub document: ValidatedDocumentBody,
    pub durability: Durability,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationInput {
    pub mutation_id: MutationId,
    pub commit_position: CommitPosition,
    pub document: ValidatedDocumentBody,
    pub durability: Durability,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeRecord {
    encoding_version: u8,
    document_id: String,
    previous_revision: Option<String>,
    previous_commit_position: Option<u64>,
    revision: String,
    commit_position: u64,
    #[serde(rename = "_deleted")]
    deleted: bool,
}

impl ChangeRecord {
    #[must_use]
    pub fn document_id(&self) -> &str {
        &self.document_id
    }

    #[must_use]
    pub fn previous_revision(&self) -> Option<&str> {
        self.previous_revision.as_deref()
    }

    #[must_use]
    pub const fn previous_commit_position(&self) -> Option<u64> {
        self.previous_commit_position
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    #[must_use]
    pub const fn commit_position(&self) -> u64 {
        self.commit_position
    }

    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MutationError> {
        let record: Self = serde_json::from_slice(bytes).map_err(MutationError::Json)?;
        if record.encoding_version != CHANGE_RECORD_ENCODING_VERSION {
            return Err(MutationError::UnsupportedChangeEncoding(
                record.encoding_version,
            ));
        }
        Ok(record)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationReceipt {
    pub mutation_id: MutationId,
    pub document_id: DocumentId,
    pub revision: RevisionToken,
    pub commit_position: CommitPosition,
    pub deleted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationCommitOutcome {
    Applied(MutationReceipt),
    Replayed(MutationReceipt),
    RevisionConflict {
        current_revision: Option<RevisionToken>,
    },
}

impl ScopedCollectionEngine {
    pub async fn create_document(
        &self,
        input: MutationInput,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation(input.into_request(ExpectedRevision::Missing, false))
            .await
    }

    pub async fn create_document_authorized(
        &self,
        input: MutationInput,
        authorizer: &dyn DocumentMutationAuthorizer,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation_authorized(
            input.into_request(ExpectedRevision::Missing, false),
            authorizer,
        )
        .await
    }

    pub async fn update_document(
        &self,
        assumed_revision: RevisionToken,
        input: MutationInput,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation(input.into_request(ExpectedRevision::Exact(assumed_revision), false))
            .await
    }

    pub async fn update_document_authorized(
        &self,
        assumed_revision: RevisionToken,
        input: MutationInput,
        authorizer: &dyn DocumentMutationAuthorizer,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation_authorized(
            input.into_request(ExpectedRevision::Exact(assumed_revision), false),
            authorizer,
        )
        .await
    }

    pub async fn delete_document(
        &self,
        assumed_revision: RevisionToken,
        input: MutationInput,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation(input.into_request(ExpectedRevision::Exact(assumed_revision), true))
            .await
    }

    pub async fn delete_document_authorized(
        &self,
        assumed_revision: RevisionToken,
        input: MutationInput,
        authorizer: &dyn DocumentMutationAuthorizer,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation_authorized(
            input.into_request(ExpectedRevision::Exact(assumed_revision), true),
            authorizer,
        )
        .await
    }

    pub async fn commit_mutation(
        &self,
        request: MutationRequest,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation_inner(request, None).await
    }

    pub async fn commit_mutation_authorized(
        &self,
        request: MutationRequest,
        authorizer: &dyn DocumentMutationAuthorizer,
    ) -> Result<MutationCommitOutcome, MutationError> {
        self.commit_mutation_inner(request, Some(authorizer)).await
    }

    async fn commit_mutation_inner(
        &self,
        request: MutationRequest,
        authorizer: Option<&dyn DocumentMutationAuthorizer>,
    ) -> Result<MutationCommitOutcome, MutationError> {
        if self.adapter.capabilities().strongest_durability < request.durability {
            return Err(MutationError::UnsupportedDurability(request.durability));
        }
        let idempotency_key = self
            .keyspace
            .idempotency_key(DOCUMENT_MUTATION_DOMAIN, request.mutation_id.as_str())?;
        let fingerprint = mutation_fingerprint(&request)?;
        if let Some(encoded) = self.adapter.get(&idempotency_key).await? {
            return replay_outcome(&request.mutation_id, &fingerprint, &encoded);
        }
        self.require_allocated_position(request.commit_position.get())
            .await?;
        let document_key = self.document_key(request.document.primary_key())?;
        let current_bytes = self.adapter.get(&document_key).await?;
        let current = current_bytes
            .as_deref()
            .map(CanonicalDocument::decode)
            .transpose()?;
        if !revision_matches(&request.expected_revision, current.as_ref()) {
            return Ok(MutationCommitOutcome::RevisionConflict {
                current_revision: current.map(|document| document.revision().clone()),
            });
        }

        let previous_revision = current
            .as_ref()
            .map(|document| document.revision().as_str().to_owned());
        let previous_commit_position = current
            .as_ref()
            .map(|document| document.commit_position().get());
        let document = CanonicalDocument::new(
            request.document.primary_key().clone(),
            request.document.schema_version(),
            request.revision.clone(),
            request.commit_position,
            request.deleted,
            Value::Object(request.document.body().clone()),
        )?;
        if let Some(authorizer) = authorizer {
            let operation = if request.deleted {
                MutationAuthorizationOperation::Delete
            } else if current.is_some() {
                MutationAuthorizationOperation::Update
            } else {
                MutationAuthorizationOperation::Create
            };
            let decision = authorizer.authorize(MutationAuthorizationContext::new(
                self,
                operation,
                current.as_ref(),
                &document,
            ));
            if !decision.is_allowed() {
                return Err(MutationError::AuthorizationDenied {
                    code: decision.stable_code().to_owned(),
                });
            }
        }
        let (catalog_key, stored_catalog, catalog) = self.load_index_catalog().await?;
        let index_plan =
            plan_index_mutation(&self.keyspace, &catalog, current.as_ref(), &document)?;
        let encoded_document = document.encode()?;

        let change = ChangeRecord {
            encoding_version: CHANGE_RECORD_ENCODING_VERSION,
            document_id: document.primary_key().as_str().to_owned(),
            previous_revision,
            previous_commit_position,
            revision: document.revision().as_str().to_owned(),
            commit_position: document.commit_position().get(),
            deleted: document.is_deleted(),
        };
        let encoded_change = serde_json::to_vec(&change).map_err(MutationError::Json)?;
        let revision_key = self.keyspace.revision_key(
            self.scope().collection_id().as_str(),
            request.commit_position.get(),
            document.primary_key().as_str(),
            document.revision().as_str(),
        )?;
        let change_key = self.keyspace.change_key(
            self.scope().collection_id().as_str(),
            request.commit_position.get(),
            document.primary_key().as_str(),
        )?;
        let committed_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Committed, request.commit_position.get());
        let aborted_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Aborted, request.commit_position.get());
        let receipt = MutationReceipt {
            mutation_id: request.mutation_id,
            document_id: document.primary_key().clone(),
            revision: document.revision().clone(),
            commit_position: document.commit_position(),
            deleted: document.is_deleted(),
        };
        let encoded_receipt = encode_receipt(&receipt, &fingerprint)?;

        let current_condition = match current_bytes {
            Some(value) => KeyCondition::ValueEquals {
                key: document_key.clone(),
                value,
            },
            None => KeyCondition::Missing {
                key: document_key.clone(),
            },
        };
        let mut batch = WriteBatch::with_capacity(5 + index_plan.deltas.len());
        batch.put(&document_key, &encoded_document);
        batch.put(&revision_key, &encoded_document);
        for delta in index_plan.deltas {
            match delta {
                IndexDelta::Put { key, value } => {
                    batch.put(key, value);
                }
                IndexDelta::Delete { key } => {
                    batch.delete(key);
                }
            }
        }
        batch.put(&change_key, encoded_change);
        batch.put(&idempotency_key, encoded_receipt);
        batch.put(&committed_key, STATUS_MARKER);

        let mut conditions = vec![
            current_condition,
            KeyCondition::Missing { key: revision_key },
            KeyCondition::Missing { key: change_key },
            KeyCondition::Missing {
                key: idempotency_key.clone(),
            },
            KeyCondition::Missing { key: committed_key },
            KeyCondition::Missing { key: aborted_key },
        ];
        let catalog_condition_index = conditions.len();
        conditions.push(match stored_catalog {
            Some(value) => KeyCondition::ValueEquals {
                key: catalog_key,
                value,
            },
            None => KeyCondition::Missing { key: catalog_key },
        });
        let first_index_condition = conditions.len();
        conditions.extend(
            index_plan
                .conditions
                .iter()
                .map(|condition| condition.condition.clone()),
        );

        let result = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: request.durability,
            })
            .await?;

        match result {
            CompareAndWriteResult::Applied => Ok(MutationCommitOutcome::Applied(receipt)),
            CompareAndWriteResult::Conflict {
                failed_condition,
                actual_value,
            } => {
                if let Some(encoded) = self.adapter.get(&idempotency_key).await? {
                    return replay_outcome(&receipt.mutation_id, &fingerprint, &encoded);
                }
                if failed_condition != 0 {
                    if failed_condition == catalog_condition_index {
                        return Err(MutationError::IndexCatalogChanged);
                    }
                    if let Some(condition) = failed_condition
                        .checked_sub(first_index_condition)
                        .and_then(|index| index_plan.conditions.get(index))
                        && condition.unique_claim
                    {
                        return Err(MutationError::UniqueConstraintViolation {
                            index: condition.index.clone(),
                            version: condition.version,
                        });
                    }
                    return Err(MutationError::AtomicConflict { failed_condition });
                }
                let current_revision = actual_value
                    .as_deref()
                    .map(CanonicalDocument::decode)
                    .transpose()?
                    .map(|document| document.revision().clone());
                Ok(MutationCommitOutcome::RevisionConflict { current_revision })
            }
        }
    }

    async fn require_allocated_position(&self, position: u64) -> Result<(), MutationError> {
        let lease_key = self.keyspace.sequencer_key(SequencerKeyKind::Lease);
        let next = self
            .adapter
            .get(&lease_key)
            .await?
            .as_deref()
            .map(decode_position)
            .transpose()?
            .unwrap_or(1);
        if position == 0 || position >= next {
            Err(MutationError::PositionNotAllocated(position))
        } else {
            Ok(())
        }
    }
}

impl MutationInput {
    fn into_request(self, expected_revision: ExpectedRevision, deleted: bool) -> MutationRequest {
        let revision = self.mutation_id.revision_for(self.document.primary_key());
        MutationRequest {
            mutation_id: self.mutation_id,
            expected_revision,
            revision,
            commit_position: self.commit_position,
            deleted,
            document: self.document,
            durability: self.durability,
        }
    }
}

fn revision_matches(expected: &ExpectedRevision, current: Option<&CanonicalDocument>) -> bool {
    match (expected, current) {
        (ExpectedRevision::Missing, None) => true,
        (ExpectedRevision::Exact(expected), Some(current)) => expected == current.revision(),
        _ => false,
    }
}

#[derive(Serialize)]
struct MutationReceiptWire<'a> {
    encoding_version: u8,
    mutation_id: &'a str,
    document_id: &'a str,
    revision: &'a str,
    commit_position: u64,
    deleted: bool,
    fingerprint: &'a str,
}

fn encode_receipt(receipt: &MutationReceipt, fingerprint: &str) -> Result<Vec<u8>, MutationError> {
    serde_json::to_vec(&MutationReceiptWire {
        encoding_version: IDEMPOTENCY_RECORD_ENCODING_VERSION,
        mutation_id: receipt.mutation_id.as_str(),
        document_id: receipt.document_id.as_str(),
        revision: receipt.revision.as_str(),
        commit_position: receipt.commit_position.get(),
        deleted: receipt.deleted,
        fingerprint,
    })
    .map_err(MutationError::Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedMutationReceiptWire {
    encoding_version: u8,
    mutation_id: String,
    document_id: String,
    revision: String,
    commit_position: u64,
    deleted: bool,
    fingerprint: String,
}

fn decode_receipt(bytes: &[u8]) -> Result<(MutationReceipt, String), MutationError> {
    let wire: OwnedMutationReceiptWire =
        serde_json::from_slice(bytes).map_err(MutationError::Json)?;
    if wire.encoding_version != IDEMPOTENCY_RECORD_ENCODING_VERSION {
        return Err(MutationError::UnsupportedIdempotencyEncoding(
            wire.encoding_version,
        ));
    }
    Ok((
        MutationReceipt {
            mutation_id: MutationId::parse(wire.mutation_id)?,
            document_id: DocumentId::parse(wire.document_id)?,
            revision: RevisionToken::parse(wire.revision)?,
            commit_position: CommitPosition::new(wire.commit_position)?,
            deleted: wire.deleted,
        },
        wire.fingerprint,
    ))
}

pub(crate) fn receipt_commit_position(bytes: &[u8]) -> Result<u64, MutationError> {
    decode_receipt(bytes).map(|(receipt, _)| receipt.commit_position.get())
}

fn replay_outcome(
    mutation_id: &MutationId,
    fingerprint: &str,
    encoded: &[u8],
) -> Result<MutationCommitOutcome, MutationError> {
    let (receipt, stored_fingerprint) = decode_receipt(encoded)?;
    if receipt.mutation_id != *mutation_id || stored_fingerprint != fingerprint {
        return Err(MutationError::IdempotencyMismatch);
    }
    Ok(MutationCommitOutcome::Replayed(receipt))
}

fn mutation_fingerprint(request: &MutationRequest) -> Result<String, MutationError> {
    let mut hasher = blake3::Hasher::new();
    hash_part(&mut hasher, b"mako-document-mutation-v1");
    hash_part(&mut hasher, request.mutation_id.as_str().as_bytes());
    match &request.expected_revision {
        ExpectedRevision::Missing => hash_part(&mut hasher, b"missing"),
        ExpectedRevision::Exact(revision) => {
            hash_part(&mut hasher, b"exact");
            hash_part(&mut hasher, revision.as_str().as_bytes());
        }
    }
    hash_part(&mut hasher, request.revision.as_str().as_bytes());
    hash_part(
        &mut hasher,
        request.document.primary_key().as_str().as_bytes(),
    );
    hash_part(
        &mut hasher,
        &request.document.schema_version().get().to_be_bytes(),
    );
    // Commit positions are server allocation details, not client mutation
    // identity. Excluding them lets a retry replay its persisted receipt even
    // when the retry reserves a different position, which the caller aborts.
    hash_part(&mut hasher, &[u8::from(request.deleted)]);
    hash_part(
        &mut hasher,
        &serde_json::to_vec(request.document.body()).map_err(MutationError::Json)?,
    );
    Ok(hasher.finalize().to_hex().to_string())
}

#[derive(Debug, Default)]
struct MutationIndexPlan {
    deltas: Vec<IndexDelta>,
    conditions: Vec<MutationIndexCondition>,
}

#[derive(Debug)]
struct MutationIndexCondition {
    condition: KeyCondition,
    index: IndexName,
    version: IndexVersion,
    unique_claim: bool,
}

fn plan_index_mutation(
    keyspace: &mako_storage::TenantKeyspace,
    catalog: &IndexCatalog,
    current: Option<&CanonicalDocument>,
    next: &CanonicalDocument,
) -> Result<MutationIndexPlan, MutationError> {
    let mut plan = MutationIndexPlan::default();
    for definition in catalog
        .definitions()
        .iter()
        .filter(|definition| definition.is_maintained_by_writes())
    {
        let old_entry = current
            .filter(|document| !document.is_deleted())
            .map(|document| definition.entry_key(keyspace, document))
            .transpose()?;
        let new_entry = (!next.is_deleted())
            .then(|| definition.entry_key(keyspace, next))
            .transpose()?;

        if old_entry != new_entry {
            if let Some(key) = old_entry {
                plan.deltas.push(IndexDelta::delete(key));
            }
            if let Some(key) = new_entry {
                plan.deltas.push(IndexDelta::put(key, []));
            }
        }
        if definition.enforces_uniqueness() {
            plan_unique_ownership(&mut plan, keyspace, definition, current, next)?;
        }
    }
    Ok(plan)
}

fn plan_unique_ownership(
    plan: &mut MutationIndexPlan,
    keyspace: &mako_storage::TenantKeyspace,
    definition: &IndexDefinition,
    current: Option<&CanonicalDocument>,
    next: &CanonicalDocument,
) -> Result<(), MutationError> {
    let document_id = next.primary_key().as_str().as_bytes().to_vec();
    let old_owner = current
        .filter(|document| !document.is_deleted())
        .map(|document| definition.owner_key(keyspace, document.body()))
        .transpose()?;
    let new_owner = (!next.is_deleted())
        .then(|| definition.owner_key(keyspace, next.body()))
        .transpose()?;

    if old_owner == new_owner {
        if let Some(key) = old_owner {
            plan.conditions.push(index_condition(
                definition,
                KeyCondition::ValueEquals {
                    key,
                    value: document_id,
                },
                false,
            ));
        }
        return Ok(());
    }
    if let Some(key) = old_owner {
        plan.conditions.push(index_condition(
            definition,
            KeyCondition::ValueEquals {
                key: key.clone(),
                value: document_id.clone(),
            },
            false,
        ));
        plan.deltas.push(IndexDelta::delete(key));
    }
    if let Some(key) = new_owner {
        plan.conditions.push(index_condition(
            definition,
            KeyCondition::Missing { key: key.clone() },
            true,
        ));
        plan.deltas.push(IndexDelta::put(key, document_id));
    }
    Ok(())
}

fn index_condition(
    definition: &IndexDefinition,
    condition: KeyCondition,
    unique_claim: bool,
) -> MutationIndexCondition {
    MutationIndexCondition {
        condition,
        index: definition.name().clone(),
        version: definition.version(),
        unique_claim,
    }
}

fn hash_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[derive(Debug)]
pub enum MutationError {
    Document(DocumentCodecError),
    Scope(DocumentEngineScopeError),
    Sequencer(SequencerError),
    Index(IndexError),
    Storage(StorageError),
    KeyCodec(mako_storage::KeyCodecError),
    Json(serde_json::Error),
    UnsupportedDurability(Durability),
    PositionNotAllocated(u64),
    IndexCatalogChanged,
    UniqueConstraintViolation {
        index: IndexName,
        version: IndexVersion,
    },
    AtomicConflict {
        failed_condition: usize,
    },
    IdempotencyMismatch,
    AuthorizationDenied {
        code: String,
    },
    UnsupportedChangeEncoding(u8),
    UnsupportedIdempotencyEncoding(u8),
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(error) => error.fmt(formatter),
            Self::Scope(error) => error.fmt(formatter),
            Self::Sequencer(error) => error.fmt(formatter),
            Self::Index(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid mutation encoding: {error}"),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::PositionNotAllocated(position) => {
                write!(formatter, "sequence position {position} was not allocated")
            }
            Self::IndexCatalogChanged => {
                formatter.write_str("index catalog changed while the mutation was committing")
            }
            Self::UniqueConstraintViolation { index, version } => write!(
                formatter,
                "unique index {} version {} already owns the indexed value",
                index.as_str(),
                version.get()
            ),
            Self::AtomicConflict { failed_condition } => write!(
                formatter,
                "atomic mutation condition {failed_condition} changed concurrently"
            ),
            Self::IdempotencyMismatch => {
                formatter.write_str("mutation ID was already used for different mutation content")
            }
            Self::AuthorizationDenied { code } => {
                write!(formatter, "document mutation denied by policy ({code})")
            }
            Self::UnsupportedChangeEncoding(version) => {
                write!(
                    formatter,
                    "unsupported change-record encoding version {version}"
                )
            }
            Self::UnsupportedIdempotencyEncoding(version) => write!(
                formatter,
                "unsupported idempotency-record encoding version {version}"
            ),
        }
    }
}

impl Error for MutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Document(error) => Some(error),
            Self::Scope(error) => Some(error),
            Self::Sequencer(error) => Some(error),
            Self::Index(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DocumentCodecError> for MutationError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

impl From<DocumentEngineScopeError> for MutationError {
    fn from(error: DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<SequencerError> for MutationError {
    fn from(error: SequencerError) -> Self {
        Self::Sequencer(error)
    }
}

impl From<IndexError> for MutationError {
    fn from(error: IndexError) -> Self {
        Self::Index(error)
    }
}

impl From<StorageError> for MutationError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<mako_storage::KeyCodecError> for MutationError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{FailurePoint, MemoryAdapter, StorageError, StorageErrorKind};
    use serde_json::json;

    use super::*;
    use crate::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, DocumentEngine,
        DocumentValidator, IndexDefinition, IndexField, IndexKind, IndexName, IndexValue,
        IndexVersion, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
    };

    fn setup() -> (
        Arc<MemoryAdapter>,
        ScopedCollectionEngine,
        crate::EnvironmentSequencer,
        DocumentValidator,
    ) {
        let adapter = Arc::new(MemoryAdapter::new());
        let engine = DocumentEngine::new(adapter.clone());
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        let collection = CollectionScope::new(
            tenant.clone(),
            CollectionId::parse("todos").expect("collection"),
        );
        let scoped = engine
            .scope_collection(&tenant, collection)
            .expect("scoped collection");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Memory)
            .expect("sequencer");
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "title": {"type": "string"}
                },
                "required": ["id", "title"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        let validator = DocumentValidator::compile(&metadata).expect("validator");
        (adapter, scoped, sequencer, validator)
    }

    fn request(
        validator: &DocumentValidator,
        position: u64,
        expected_revision: ExpectedRevision,
        revision: &str,
    ) -> MutationRequest {
        MutationRequest {
            mutation_id: MutationId::parse(format!("mutation-{position}")).expect("mutation id"),
            expected_revision,
            revision: RevisionToken::parse(revision).expect("revision"),
            commit_position: CommitPosition::new(position).expect("position"),
            deleted: false,
            document: validator
                .validate_create(json!({"id": "todo-1", "title": revision}))
                .expect("validated document"),
            durability: Durability::Memory,
        }
    }

    #[test]
    fn commits_document_change_receipt_and_position_atomically() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(1).expect("non-zero"))
                .await
                .expect("lease");
            let outcome = scoped
                .commit_mutation(request(&validator, 1, ExpectedRevision::Missing, "rev-1"))
                .await
                .expect("commit");
            assert!(matches!(outcome, MutationCommitOutcome::Applied(_)));

            let state = adapter.dump().expect("state");
            let document_key = scoped
                .document_key(&DocumentId::parse("todo-1").expect("id"))
                .expect("document key");
            let document =
                CanonicalDocument::decode(state.get(&document_key).expect("stored document"))
                    .expect("decode document");
            assert_eq!(document.revision().as_str(), "rev-1");

            let change_key = scoped
                .keyspace
                .change_key("todos", 1, "todo-1")
                .expect("change key");
            let change = ChangeRecord::decode(state.get(&change_key).expect("change record"))
                .expect("decode change");
            assert_eq!(change.revision(), "rev-1");
            assert_eq!(
                sequencer.disposition(1).await.expect("disposition"),
                Some(crate::SequenceDisposition::Committed)
            );
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 1);
        });
    }

    #[test]
    fn injected_commit_failure_leaves_every_mutation_key_unchanged() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(1).expect("non-zero"))
                .await
                .expect("lease");
            let before = adapter.dump().expect("before");
            adapter
                .fail_next(
                    FailurePoint::BeforeCommit,
                    StorageError::new(
                        StorageErrorKind::Io,
                        "mutation_test",
                        true,
                        "fail before atomic commit",
                    ),
                )
                .expect("inject failure");

            let result = scoped
                .commit_mutation(request(&validator, 1, ExpectedRevision::Missing, "rev-1"))
                .await;

            assert!(matches!(result, Err(MutationError::Storage(_))));
            assert_eq!(adapter.dump().expect("after"), before);
        });
    }

    #[test]
    fn revision_conflicts_do_not_write() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(2).expect("non-zero"))
                .await
                .expect("lease");
            scoped
                .commit_mutation(request(&validator, 1, ExpectedRevision::Missing, "rev-1"))
                .await
                .expect("create");
            let before = adapter.dump().expect("before");

            let conflict = scoped
                .commit_mutation(request(
                    &validator,
                    2,
                    ExpectedRevision::Exact(RevisionToken::parse("stale").expect("stale revision")),
                    "rev-2",
                ))
                .await
                .expect("conflict result");
            assert!(matches!(
                conflict,
                MutationCommitOutcome::RevisionConflict { .. }
            ));
            assert_eq!(adapter.dump().expect("after conflict"), before);
        });
    }

    #[test]
    fn conditional_create_update_and_delete_return_current_revision_conflicts() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");

            let input = |position: u64, title: &str| MutationInput {
                mutation_id: MutationId::parse(format!("mutation-{position}"))
                    .expect("mutation id"),
                commit_position: CommitPosition::new(position).expect("position"),
                document: validator
                    .validate_create(json!({"id": "todo-1", "title": title}))
                    .expect("validated document"),
                durability: Durability::Memory,
            };

            let MutationCommitOutcome::Applied(created) = scoped
                .create_document(input(1, "created"))
                .await
                .expect("create")
            else {
                panic!("create must apply");
            };
            let revision_1 = created.revision;
            assert!(matches!(
                scoped
                    .create_document(input(2, "duplicate"))
                    .await
                    .expect("create conflict"),
                MutationCommitOutcome::RevisionConflict {
                    current_revision: Some(ref revision)
                } if revision == &revision_1
            ));
            assert!(matches!(
                scoped
                    .update_document(
                        RevisionToken::parse("stale").expect("revision"),
                        input(2, "stale"),
                    )
                    .await
                    .expect("update conflict"),
                MutationCommitOutcome::RevisionConflict {
                    current_revision: Some(ref revision)
                } if revision == &revision_1
            ));
            let MutationCommitOutcome::Applied(updated) = scoped
                .update_document(revision_1, input(2, "updated"))
                .await
                .expect("update")
            else {
                panic!("update must apply");
            };
            let revision_2 = updated.revision;
            assert!(matches!(
                scoped
                    .delete_document(
                        RevisionToken::parse("stale").expect("revision"),
                        input(3, "stale delete"),
                    )
                    .await
                    .expect("delete conflict"),
                MutationCommitOutcome::RevisionConflict {
                    current_revision: Some(ref revision)
                } if revision == &revision_2
            ));
            let MutationCommitOutcome::Applied(deleted) = scoped
                .delete_document(revision_2, input(3, "deleted"))
                .await
                .expect("delete")
            else {
                panic!("delete must apply");
            };

            let document_key = scoped
                .document_key(&DocumentId::parse("todo-1").expect("id"))
                .expect("document key");
            let state = adapter.dump().expect("state");
            let document =
                CanonicalDocument::decode(state.get(&document_key).expect("stored document"))
                    .expect("decode");
            assert!(document.is_deleted());
            assert_eq!(document.revision(), &deleted.revision);
            assert!(
                String::from_utf8(state.get(&document_key).expect("stored tombstone").clone())
                    .expect("utf8")
                    .contains(r#""_deleted":true"#)
            );
            for position in 1..=3 {
                let change_key = scoped
                    .keyspace
                    .change_key("todos", position, "todo-1")
                    .expect("change key");
                assert!(
                    state.contains_key(&change_key),
                    "revision history was removed"
                );
            }
        });
    }

    #[test]
    fn stable_mutation_ids_replay_receipts_without_duplicate_state() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(1).expect("non-zero"))
                .await
                .expect("lease");
            let input = MutationInput {
                mutation_id: MutationId::parse("stable-client-mutation").expect("mutation id"),
                commit_position: CommitPosition::new(1).expect("position"),
                document: validator
                    .validate_create(json!({"id": "todo-1", "title": "original"}))
                    .expect("validated document"),
                durability: Durability::Memory,
            };

            let (first, second) = futures::join!(
                scoped.create_document(input.clone()),
                scoped.create_document(input.clone())
            );
            let first = first.expect("first outcome");
            let second = second.expect("second outcome");
            let applied = [first.clone(), second.clone()]
                .into_iter()
                .find_map(|outcome| match outcome {
                    MutationCommitOutcome::Applied(receipt) => Some(receipt),
                    _ => None,
                })
                .expect("one request applies");
            let replayed = [first, second]
                .into_iter()
                .find_map(|outcome| match outcome {
                    MutationCommitOutcome::Replayed(receipt) => Some(receipt),
                    _ => None,
                })
                .expect("one request replays");
            assert_eq!(replayed, applied);

            let after_commit = adapter.dump().expect("committed state");
            assert_eq!(
                scoped
                    .create_document(input.clone())
                    .await
                    .expect("sequential replay"),
                MutationCommitOutcome::Replayed(applied)
            );
            assert_eq!(adapter.dump().expect("replayed state"), after_commit);

            let mut changed = input;
            changed.document = validator
                .validate_create(json!({"id": "todo-1", "title": "different"}))
                .expect("validated document");
            assert!(matches!(
                scoped.create_document(changed).await,
                Err(MutationError::IdempotencyMismatch)
            ));
            assert_eq!(adapter.dump().expect("mismatch state"), after_commit);
        });
    }

    #[test]
    fn document_mutations_derive_index_additions_removals_and_unique_claims() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            let name = IndexName::parse("by-title").expect("name");
            let version = IndexVersion::new(1).expect("version");
            scoped
                .create_index(
                    IndexDefinition::new_building(
                        CollectionId::parse("todos").expect("collection"),
                        name.clone(),
                        version,
                        IndexKind::NonUnique,
                        [IndexField::ascending("title").expect("field")],
                    )
                    .expect("index"),
                    Durability::Memory,
                )
                .await
                .expect("create index");
            let mut lease = sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");
            let input = |position: u64, title: &str| MutationInput {
                mutation_id: MutationId::parse(format!("indexed-{position}")).expect("mutation"),
                commit_position: CommitPosition::new(position).expect("position"),
                document: validator
                    .validate_create(json!({"id": "todo-1", "title": title}))
                    .expect("document"),
                durability: Durability::Memory,
            };
            let MutationCommitOutcome::Applied(created) = scoped
                .create_document(input(lease.issue().expect("position"), "first"))
                .await
                .expect("create")
            else {
                panic!("create must apply");
            };

            let physical = "by-title@1";
            let first_component =
                [IndexValue::String("first".to_owned()).encode(crate::IndexDirection::Ascending)];
            let second_component = [
                IndexValue::String("second".to_owned()).encode(crate::IndexDirection::Ascending)
            ];
            let first_key = scoped
                .keyspace
                .index_entry_key("todos", physical, &first_component, "todo-1")
                .expect("first key");
            assert!(adapter.dump().expect("state").contains_key(&first_key));

            let MutationCommitOutcome::Applied(updated) = scoped
                .update_document(
                    created.revision,
                    input(lease.issue().expect("position"), "second"),
                )
                .await
                .expect("update")
            else {
                panic!("update must apply");
            };
            let second_key = scoped
                .keyspace
                .index_entry_key("todos", physical, &second_component, "todo-1")
                .expect("second key");
            let state = adapter.dump().expect("state");
            assert!(!state.contains_key(&first_key));
            assert!(state.contains_key(&second_key));

            scoped
                .delete_document(
                    updated.revision,
                    input(lease.issue().expect("position"), "second"),
                )
                .await
                .expect("delete");
            assert!(!adapter.dump().expect("state").contains_key(&second_key));
        });
    }

    #[test]
    fn concurrent_unique_claims_have_exactly_one_winner() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            let name = IndexName::parse("unique-title").expect("name");
            let version = IndexVersion::new(1).expect("version");
            scoped
                .create_index(
                    IndexDefinition::new_building(
                        CollectionId::parse("todos").expect("collection"),
                        name.clone(),
                        version,
                        IndexKind::Unique,
                        [IndexField::ascending("title").expect("field")],
                    )
                    .expect("index"),
                    Durability::Memory,
                )
                .await
                .expect("create index");
            scoped
                .update_index_catalog(&name, version, Durability::Memory, |definition| {
                    definition.activate()
                })
                .await
                .expect("activate");
            sequencer
                .lease(NonZeroU64::new(2).expect("non-zero"))
                .await
                .expect("lease");

            let request = |id: &str, position: u64| MutationRequest {
                mutation_id: MutationId::parse(format!("unique-{id}")).expect("mutation"),
                expected_revision: ExpectedRevision::Missing,
                revision: RevisionToken::parse(format!("revision-{id}")).expect("revision"),
                commit_position: CommitPosition::new(position).expect("position"),
                deleted: false,
                document: validator
                    .validate_create(json!({"id": id, "title": "claimed"}))
                    .expect("document"),
                durability: Durability::Memory,
            };
            let (first, second) = futures::join!(
                scoped.commit_mutation(request("todo-1", 1)),
                scoped.commit_mutation(request("todo-2", 2))
            );
            let outcomes = [first, second];
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| matches!(outcome, Ok(MutationCommitOutcome::Applied(_))))
                    .count(),
                1
            );
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| matches!(
                        outcome,
                        Err(MutationError::UniqueConstraintViolation { .. })
                    ))
                    .count(),
                1
            );

            let range = scoped
                .keyspace
                .index_range("todos", "unique-title@1")
                .expect("range");
            assert_eq!(
                adapter
                    .dump()
                    .expect("state")
                    .keys()
                    .filter(|key| range.contains(key))
                    .count(),
                1
            );
        });
    }
}
