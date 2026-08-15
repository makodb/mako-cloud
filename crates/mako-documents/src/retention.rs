use std::{collections::BTreeSet, error::Error, fmt};

use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCodecError, KeyCondition, KeyRange,
    KeyValue, KvAdapter, KvSnapshot, ScanDirection, ScanRequest, SequencerKeyKind, StorageError,
    WriteBatch,
};

use crate::{
    CanonicalDocument, DocumentCodecError, MutationError, ScopedCollectionEngine, SequencerError,
    mutation::{DOCUMENT_MUTATION_DOMAIN, receipt_commit_position},
    sequencer::decode_high_water,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointStatus {
    Valid,
    Expired { minimum_position: u64 },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactionReport {
    pub watermark: u64,
    pub dry_run: bool,
    pub revisions_eligible: usize,
    pub changes_eligible: usize,
    pub idempotency_records_eligible: usize,
    pub tombstones_eligible: usize,
    pub revisions_removed: usize,
    pub changes_removed: usize,
    pub idempotency_records_removed: usize,
    pub tombstones_removed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionMode {
    DryRun,
    Apply,
}

impl ScopedCollectionEngine {
    pub async fn retention_watermark(&self) -> Result<u64, RetentionError> {
        let key = self
            .keyspace
            .sequencer_key(SequencerKeyKind::RetentionWatermark);
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(Into::into)
    }

    pub async fn checkpoint_status(
        &self,
        checkpoint_position: u64,
    ) -> Result<CheckpointStatus, RetentionError> {
        let minimum_position = self.retention_watermark().await?;
        if checkpoint_position < minimum_position {
            Ok(CheckpointStatus::Expired { minimum_position })
        } else {
            Ok(CheckpointStatus::Valid)
        }
    }

    /// Expires old checkpoints before deleting any data, so interruption can
    /// retain extra history but can never make a nominally valid checkpoint skip it.
    pub async fn compact_through(
        &self,
        position: u64,
        durability: Durability,
    ) -> Result<CompactionReport, RetentionError> {
        self.compact_through_with_mode(position, durability, CompactionMode::Apply)
            .await
    }

    pub async fn preview_compaction_through(
        &self,
        position: u64,
        durability: Durability,
    ) -> Result<CompactionReport, RetentionError> {
        self.compact_through_with_mode(position, durability, CompactionMode::DryRun)
            .await
    }

    pub async fn compact_through_with_mode(
        &self,
        position: u64,
        durability: Durability,
        mode: CompactionMode,
    ) -> Result<CompactionReport, RetentionError> {
        if self.adapter.capabilities().strongest_durability < durability {
            return Err(RetentionError::UnsupportedDurability(durability));
        }
        let watermark = match mode {
            CompactionMode::DryRun => self.prospective_watermark(position).await?,
            CompactionMode::Apply => self.raise_retention_barrier(position, durability).await?,
        };
        let snapshot = self.adapter.snapshot().await?;
        let candidates = self
            .compaction_candidates(snapshot.as_ref(), watermark)
            .await?;
        let eligible = candidates.report(watermark, mode == CompactionMode::DryRun);
        if mode == CompactionMode::DryRun {
            return Ok(eligible);
        }

        delete_keys(self.adapter.as_ref(), &candidates.revisions, durability).await?;
        delete_keys(self.adapter.as_ref(), &candidates.changes, durability).await?;
        delete_keys(
            self.adapter.as_ref(),
            &candidates.idempotency_records,
            durability,
        )
        .await?;
        let tombstones_removed =
            delete_conditional_entries(self.adapter.as_ref(), &candidates.tombstones, durability)
                .await?;

        Ok(CompactionReport {
            revisions_removed: candidates.revisions.len(),
            changes_removed: candidates.changes.len(),
            idempotency_records_removed: candidates.idempotency_records.len(),
            tombstones_removed,
            ..eligible
        })
    }

    async fn compaction_candidates(
        &self,
        snapshot: &dyn KvSnapshot,
        watermark: u64,
    ) -> Result<CompactionCandidates, RetentionError> {
        let collection = self.scope().collection_id().as_str();
        let scan_limit = self.adapter.capabilities().maximum_scan_items;

        let change_entries = scan_all(
            snapshot,
            self.keyspace.changes_range(collection)?,
            scan_limit,
        )
        .await?;
        let mut change_keys = Vec::new();
        let mut retained_predecessors = BTreeSet::new();
        for entry in &change_entries {
            let position = self
                .keyspace
                .decode_change_key(collection, &entry.key)?
                .position;
            if position <= watermark {
                change_keys.push(entry.key.clone());
                continue;
            }
            let change = crate::ChangeRecord::decode(&entry.value)?;
            match (
                change.previous_commit_position(),
                change.previous_revision(),
            ) {
                (Some(previous_position), Some(previous_revision))
                    if previous_position <= watermark =>
                {
                    retained_predecessors.insert(self.keyspace.revision_key(
                        collection,
                        previous_position,
                        change.document_id(),
                        previous_revision,
                    )?);
                }
                (Some(_), Some(_)) | (None, None) => {}
                _ => return Err(RetentionError::InvalidChangeHistory),
            }
        }

        let revision_entries = scan_all(
            snapshot,
            self.keyspace.revisions_range(collection)?,
            scan_limit,
        )
        .await?;
        let mut revision_keys = Vec::new();
        for entry in revision_entries {
            if self
                .keyspace
                .decode_revision_key(collection, &entry.key)?
                .position
                > watermark
            {
                break;
            }
            if !retained_predecessors.contains(&entry.key) {
                revision_keys.push(entry.key);
            }
        }

        let idempotency_entries = scan_all(
            snapshot,
            self.keyspace.idempotency_range(DOCUMENT_MUTATION_DOMAIN)?,
            scan_limit,
        )
        .await?;
        let mut idempotency_keys = Vec::new();
        for entry in idempotency_entries {
            if receipt_commit_position(&entry.value)? <= watermark {
                idempotency_keys.push(entry.key);
            }
        }

        let document_entries = scan_all(
            snapshot,
            self.keyspace.documents_range(collection)?,
            scan_limit,
        )
        .await?;
        let mut tombstone_keys = Vec::new();
        for entry in document_entries {
            let document = CanonicalDocument::decode(&entry.value)?;
            if document.is_deleted() && document.commit_position().get() <= watermark {
                tombstone_keys.push(entry);
            }
        }

        Ok(CompactionCandidates {
            revisions: revision_keys,
            changes: change_keys,
            idempotency_records: idempotency_keys,
            tombstones: tombstone_keys,
        })
    }

    async fn prospective_watermark(&self, requested: u64) -> Result<u64, RetentionError> {
        let current = self.retention_watermark().await?;
        let high_water_key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        let high_water = self
            .adapter
            .get(&high_water_key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()?
            .unwrap_or_default();
        if requested > high_water {
            return Err(RetentionError::BeyondCommittedHighWater {
                requested,
                high_water,
            });
        }
        Ok(current.max(requested))
    }

    async fn raise_retention_barrier(
        &self,
        requested: u64,
        durability: Durability,
    ) -> Result<u64, RetentionError> {
        let watermark_key = self
            .keyspace
            .sequencer_key(SequencerKeyKind::RetentionWatermark);
        let high_water_key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        loop {
            let high_water = self
                .adapter
                .get(&high_water_key)
                .await?
                .as_deref()
                .map(decode_high_water)
                .transpose()?
                .unwrap_or_default();
            if requested > high_water {
                return Err(RetentionError::BeyondCommittedHighWater {
                    requested,
                    high_water,
                });
            }

            let stored = self.adapter.get(&watermark_key).await?;
            let current = stored
                .as_deref()
                .map(decode_high_water)
                .transpose()?
                .unwrap_or_default();
            if requested <= current {
                return Ok(current);
            }
            let condition = match &stored {
                Some(value) => KeyCondition::ValueEquals {
                    key: watermark_key.clone(),
                    value: value.clone(),
                },
                None => KeyCondition::Missing {
                    key: watermark_key.clone(),
                },
            };
            let mut batch = WriteBatch::new();
            batch.put(&watermark_key, requested.to_be_bytes());
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(requested);
            }
        }
    }
}

struct CompactionCandidates {
    revisions: Vec<Vec<u8>>,
    changes: Vec<Vec<u8>>,
    idempotency_records: Vec<Vec<u8>>,
    tombstones: Vec<KeyValue>,
}

impl CompactionCandidates {
    fn report(&self, watermark: u64, dry_run: bool) -> CompactionReport {
        CompactionReport {
            watermark,
            dry_run,
            revisions_eligible: self.revisions.len(),
            changes_eligible: self.changes.len(),
            idempotency_records_eligible: self.idempotency_records.len(),
            tombstones_eligible: self.tombstones.len(),
            revisions_removed: 0,
            changes_removed: 0,
            idempotency_records_removed: 0,
            tombstones_removed: 0,
        }
    }
}

async fn scan_all(
    snapshot: &dyn KvSnapshot,
    mut range: KeyRange,
    limit: std::num::NonZeroUsize,
) -> Result<Vec<KeyValue>, RetentionError> {
    let mut output = Vec::new();
    loop {
        let page = snapshot
            .scan(ScanRequest::new(
                range.clone(),
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let full_page = page.len() == limit.get();
        let Some(last_key) = page.last().map(|entry| entry.key.clone()) else {
            break;
        };
        output.extend(page);
        if !full_page {
            break;
        }
        let mut next = last_key;
        next.push(0);
        if next >= range.end_exclusive {
            break;
        }
        range.start_inclusive = next;
    }
    Ok(output)
}

async fn delete_conditional_entries(
    adapter: &dyn KvAdapter,
    entries: &[KeyValue],
    durability: Durability,
) -> Result<usize, RetentionError> {
    let mut removed = 0;
    for entry in entries {
        let mut batch = WriteBatch::new();
        batch.delete(&entry.key);
        if adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                }],
                batch,
                durability,
            })
            .await?
            == CompareAndWriteResult::Applied
        {
            removed += 1;
        }
    }
    Ok(removed)
}

async fn delete_keys(
    adapter: &dyn KvAdapter,
    keys: &[Vec<u8>],
    durability: Durability,
) -> Result<(), RetentionError> {
    let maximum = adapter.capabilities().maximum_batch_operations.get();
    for chunk in keys.chunks(maximum) {
        let mut batch = WriteBatch::with_capacity(chunk.len());
        for key in chunk {
            batch.delete(key);
        }
        adapter.write(batch, durability).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum RetentionError {
    Storage(StorageError),
    KeyCodec(KeyCodecError),
    Document(DocumentCodecError),
    Mutation(MutationError),
    Sequencer(SequencerError),
    UnsupportedDurability(Durability),
    BeyondCommittedHighWater { requested: u64, high_water: u64 },
    InvalidChangeHistory,
}

impl fmt::Display for RetentionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
            Self::Sequencer(error) => error.fmt(formatter),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::BeyondCommittedHighWater {
                requested,
                high_water,
            } => write!(
                formatter,
                "retention position {requested} exceeds committed high water {high_water}"
            ),
            Self::InvalidChangeHistory => {
                formatter.write_str("change record has inconsistent previous revision metadata")
            }
        }
    }
}

impl Error for RetentionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::Mutation(error) => Some(error),
            Self::Sequencer(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StorageError> for RetentionError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<KeyCodecError> for RetentionError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<DocumentCodecError> for RetentionError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

impl From<MutationError> for RetentionError {
    fn from(error: MutationError) -> Self {
        Self::Mutation(error)
    }
}

impl From<SequencerError> for RetentionError {
    fn from(error: SequencerError) -> Self {
        Self::Sequencer(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::{NonZeroU64, NonZeroUsize},
        sync::Arc,
    };

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{MemoryAdapter, ScanDirection};
    use serde_json::json;

    use super::*;
    use crate::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentId, DocumentValidator, MutationCommitOutcome, MutationId,
        MutationInput, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
    };

    #[test]
    fn expires_checkpoints_before_compacting_history_and_tombstones() {
        block_on(async {
            let (adapter, scoped, sequencer, validator) = setup();
            sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");

            let input = |position: u64, title: &str| MutationInput {
                mutation_id: MutationId::parse(format!("retention-{position}"))
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
            let MutationCommitOutcome::Applied(updated) = scoped
                .update_document(created.revision, input(2, "updated"))
                .await
                .expect("update")
            else {
                panic!("update must apply");
            };
            let MutationCommitOutcome::Applied(_) = scoped
                .delete_document(updated.revision, input(3, "deleted"))
                .await
                .expect("delete")
            else {
                panic!("delete must apply");
            };
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 3);

            assert_eq!(
                scoped.checkpoint_status(1).await.expect("checkpoint"),
                CheckpointStatus::Valid
            );
            let preview = scoped
                .preview_compaction_through(2, Durability::Memory)
                .await
                .expect("preview through 2");
            assert!(preview.dry_run);
            assert_eq!(preview.watermark, 2);
            assert_eq!(preview.revisions_eligible, 1);
            assert_eq!(preview.changes_eligible, 2);
            assert_eq!(preview.idempotency_records_eligible, 2);
            assert_eq!(preview.tombstones_eligible, 0);
            assert_eq!(preview.revisions_removed, 0);
            assert_eq!(preview.changes_removed, 0);
            assert_eq!(scoped.retention_watermark().await.expect("watermark"), 0);
            assert_eq!(
                scoped.checkpoint_status(1).await.expect("still valid"),
                CheckpointStatus::Valid
            );
            let first = scoped
                .compact_through(2, Durability::Memory)
                .await
                .expect("compact through 2");
            assert!(!first.dry_run);
            assert_eq!(first.revisions_eligible, 1);
            assert_eq!(first.changes_eligible, 2);
            assert_eq!(first.revisions_removed, 1);
            assert_eq!(first.changes_removed, 2);
            assert_eq!(first.idempotency_records_removed, 2);
            assert_eq!(first.tombstones_removed, 0);
            assert_eq!(
                scoped.checkpoint_status(1).await.expect("expired"),
                CheckpointStatus::Expired {
                    minimum_position: 2
                }
            );
            assert_eq!(
                scoped.checkpoint_status(2).await.expect("valid"),
                CheckpointStatus::Valid
            );
            assert!(
                scoped
                    .get_document(&DocumentId::parse("todo-1").expect("id"))
                    .await
                    .expect("read")
                    .expect("tombstone")
                    .is_deleted()
            );

            let second = scoped
                .compact_through(3, Durability::Memory)
                .await
                .expect("compact through 3");
            assert_eq!(second.revisions_removed, 2);
            assert_eq!(second.changes_removed, 1);
            assert_eq!(second.idempotency_records_removed, 1);
            assert_eq!(second.tombstones_removed, 1);
            assert!(
                scoped
                    .get_document(&DocumentId::parse("todo-1").expect("id"))
                    .await
                    .expect("read")
                    .is_none()
            );
            assert!(matches!(
                scoped.compact_through(4, Durability::Memory).await,
                Err(RetentionError::BeyondCommittedHighWater {
                    requested: 4,
                    high_water: 3
                })
            ));
            assert_eq!(scoped.retention_watermark().await.expect("watermark"), 3);

            let revision_range = scoped.keyspace.revisions_range("todos").expect("range");
            let remaining = adapter
                .scan(ScanRequest::new(
                    revision_range,
                    ScanDirection::Forward,
                    NonZeroUsize::new(100).expect("non-zero"),
                ))
                .await
                .expect("scan");
            assert!(remaining.is_empty());
        });
    }

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
        let scoped = engine
            .scope_collection(
                &tenant,
                CollectionScope::new(
                    tenant.clone(),
                    CollectionId::parse("todos").expect("collection"),
                ),
            )
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
}
