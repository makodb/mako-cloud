use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize};

use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, ScanDirection, ScanRequest,
    SequencerKeyKind, StorageError, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    CanonicalDocument, DocumentCodecError, DocumentEngineScopeError, DocumentId, IndexDefinition,
    IndexError, IndexName, IndexState, IndexVersion, MutationError, ScopedCollectionEngine,
    index::IndexCatalog,
    mutation::ChangeRecord,
    sequencer::{SequencerError, decode_high_water},
};

const INDEX_BUILD_PROGRESS_ENCODING_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexBuildProgress {
    name: IndexName,
    version: IndexVersion,
    captured_position: u64,
    last_document_id: Option<DocumentId>,
    backfill_complete: bool,
    caught_up_position: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexActivationOutcome {
    Active(IndexDefinition),
    Failed(IndexDefinition),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexRemovalReport {
    entries_removed: usize,
    ownership_records_removed: usize,
    progress_removed: bool,
}

impl IndexRemovalReport {
    #[must_use]
    pub const fn entries_removed(&self) -> usize {
        self.entries_removed
    }

    #[must_use]
    pub const fn ownership_records_removed(&self) -> usize {
        self.ownership_records_removed
    }

    #[must_use]
    pub const fn progress_removed(&self) -> bool {
        self.progress_removed
    }
}

impl IndexBuildProgress {
    #[must_use]
    pub fn name(&self) -> &IndexName {
        &self.name
    }

    #[must_use]
    pub const fn version(&self) -> IndexVersion {
        self.version
    }

    #[must_use]
    pub const fn captured_position(&self) -> u64 {
        self.captured_position
    }

    #[must_use]
    pub fn last_document_id(&self) -> Option<&DocumentId> {
        self.last_document_id.as_ref()
    }

    #[must_use]
    pub const fn backfill_complete(&self) -> bool {
        self.backfill_complete
    }

    #[must_use]
    pub const fn caught_up_position(&self) -> u64 {
        self.caught_up_position
    }

    fn encode(&self) -> Result<Vec<u8>, IndexBuildError> {
        serde_json::to_vec(&IndexBuildProgressWire {
            encoding_version: INDEX_BUILD_PROGRESS_ENCODING_VERSION,
            name: self.name.clone(),
            version: self.version.get(),
            captured_position: self.captured_position,
            last_document_id: self
                .last_document_id
                .as_ref()
                .map(|document_id| document_id.as_str().to_owned()),
            backfill_complete: self.backfill_complete,
            caught_up_position: self.caught_up_position,
        })
        .map_err(IndexBuildError::Json)
    }

    fn decode(bytes: &[u8]) -> Result<Self, IndexBuildError> {
        let wire: IndexBuildProgressWire =
            serde_json::from_slice(bytes).map_err(IndexBuildError::Json)?;
        if wire.encoding_version != INDEX_BUILD_PROGRESS_ENCODING_VERSION {
            return Err(IndexBuildError::UnsupportedProgressEncoding(
                wire.encoding_version,
            ));
        }
        if wire.caught_up_position > wire.captured_position && !wire.backfill_complete {
            return Err(IndexBuildError::InvalidProgress(
                "catch-up cannot advance before backfill completes",
            ));
        }
        Ok(Self {
            name: wire.name,
            version: IndexVersion::new(wire.version)?,
            captured_position: wire.captured_position,
            last_document_id: wire.last_document_id.map(DocumentId::parse).transpose()?,
            backfill_complete: wire.backfill_complete,
            caught_up_position: wire.caught_up_position,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IndexBuildProgressWire {
    encoding_version: u8,
    name: IndexName,
    version: u64,
    captured_position: u64,
    last_document_id: Option<String>,
    backfill_complete: bool,
    caught_up_position: u64,
}

impl ScopedCollectionEngine {
    pub async fn index_build_progress(
        &self,
        name: &IndexName,
        version: IndexVersion,
    ) -> Result<Option<IndexBuildProgress>, IndexBuildError> {
        let key = self.progress_key(name, version)?;
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(IndexBuildProgress::decode)
            .transpose()
    }

    /// Writes at most one bounded page. Repeated calls resume after the last
    /// document whose entries and progress marker committed atomically.
    pub async fn backfill_index(
        &self,
        name: &IndexName,
        version: IndexVersion,
        page_size: NonZeroUsize,
        durability: Durability,
    ) -> Result<IndexBuildProgress, IndexBuildError> {
        self.ensure_build_capacity(page_size, durability)?;
        let mut progress = self
            .ensure_build_progress(name, version, durability)
            .await?;
        if progress.backfill_complete {
            return Ok(progress);
        }

        let (catalog_key, catalog_bytes, catalog) = self.load_index_catalog().await?;
        let definition = require_building_definition(&catalog, name, version)?.clone();
        let snapshot = self.adapter.snapshot().await?;
        let mut range = self
            .keyspace
            .documents_range(self.scope().collection_id().as_str())?;
        if let Some(last_document_id) = progress.last_document_id.as_ref() {
            let mut next = self.keyspace.document_key(
                self.scope().collection_id().as_str(),
                last_document_id.as_str(),
            )?;
            next.push(0);
            if next >= range.end_exclusive {
                progress.backfill_complete = true;
                return self
                    .store_progress_only(&catalog_key, catalog_bytes, progress, durability)
                    .await;
            }
            range.start_inclusive = next;
        }
        let entries = snapshot
            .scan(ScanRequest::new(range, ScanDirection::Forward, page_size))
            .await?;

        let progress_key = self.progress_key(name, version)?;
        let stored_progress = self.adapter.get(&progress_key).await?;
        let observed_progress = stored_progress
            .as_deref()
            .map(IndexBuildProgress::decode)
            .transpose()?
            .ok_or(IndexBuildError::ProgressChanged)?;
        if observed_progress != progress {
            return Err(IndexBuildError::ProgressChanged);
        }

        let mut batch = WriteBatch::with_capacity(entries.len() + 1);
        for entry in &entries {
            let document = CanonicalDocument::decode(&entry.value)?;
            if !document.is_deleted() {
                batch.put(definition.entry_key(&self.keyspace, &document)?, []);
            }
        }
        if let Some(last) = entries.last() {
            let decoded = self
                .keyspace
                .decode_document_key(self.scope().collection_id().as_str(), &last.key)?;
            let document_id = String::from_utf8(decoded)
                .map_err(|_| IndexBuildError::InvalidProgress("document key is not UTF-8"))?;
            progress.last_document_id = Some(DocumentId::parse(document_id)?);
        }
        if entries.len() < page_size.get() {
            progress.backfill_complete = true;
        }
        batch.put(&progress_key, progress.encode()?);

        let mut conditions = vec![catalog_condition(&catalog_key, catalog_bytes)];
        conditions.push(KeyCondition::ValueEquals {
            key: progress_key,
            value: stored_progress.expect("observed progress is present"),
        });
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(progress),
            CompareAndWriteResult::Conflict { .. } => Err(IndexBuildError::ProgressChanged),
        }
    }

    /// Replays at most one bounded change-log page through a captured committed
    /// high water. Building indexes are also maintained by live writes, making
    /// replay idempotent while repairing any snapshot/write overlap.
    pub async fn catch_up_index(
        &self,
        name: &IndexName,
        version: IndexVersion,
        page_size: NonZeroUsize,
        durability: Durability,
    ) -> Result<IndexBuildProgress, IndexBuildError> {
        self.ensure_catch_up_capacity(page_size, durability)?;
        let progress_key = self.progress_key(name, version)?;
        let stored_progress = self
            .adapter
            .get(&progress_key)
            .await?
            .ok_or(IndexBuildError::ProgressNotInitialized)?;
        let mut progress = IndexBuildProgress::decode(&stored_progress)?;
        if !progress.backfill_complete {
            return Err(IndexBuildError::BackfillIncomplete);
        }
        let target = self.committed_high_water_for_build().await?;
        if progress.caught_up_position >= target {
            return Ok(progress);
        }

        let (catalog_key, catalog_bytes, catalog) = self.load_index_catalog().await?;
        let definition = require_building_definition(&catalog, name, version)?.clone();
        let snapshot = self.adapter.snapshot().await?;
        let range = self.keyspace.changes_after_through(
            self.scope().collection_id().as_str(),
            progress.caught_up_position,
            target,
        )?;
        let changes = snapshot
            .scan(ScanRequest::new(range, ScanDirection::Forward, page_size))
            .await?;
        let mut batch = WriteBatch::with_capacity((changes.len() * 2) + 1);
        for entry in &changes {
            let change = ChangeRecord::decode(&entry.value)?;
            let old = match (
                change.previous_commit_position(),
                change.previous_revision(),
            ) {
                (Some(position), Some(revision)) => {
                    let key = self.keyspace.revision_key(
                        self.scope().collection_id().as_str(),
                        position,
                        change.document_id(),
                        revision,
                    )?;
                    Some(
                        snapshot
                            .get(&key)
                            .await?
                            .ok_or(IndexBuildError::RequiredHistoryMissing(position))
                            .and_then(|bytes| {
                                CanonicalDocument::decode(&bytes).map_err(Into::into)
                            })?,
                    )
                }
                (None, None) => None,
                _ => return Err(IndexBuildError::InvalidChangeHistory),
            };
            let new_key = self.keyspace.revision_key(
                self.scope().collection_id().as_str(),
                change.commit_position(),
                change.document_id(),
                change.revision(),
            )?;
            let new = snapshot
                .get(&new_key)
                .await?
                .ok_or(IndexBuildError::RequiredHistoryMissing(
                    change.commit_position(),
                ))
                .and_then(|bytes| CanonicalDocument::decode(&bytes).map_err(Into::into))?;
            append_replayed_index_delta(
                &mut batch,
                &self.keyspace,
                &definition,
                old.as_ref(),
                &new,
            )?;
        }
        progress.caught_up_position = if changes.len() < page_size.get() {
            target
        } else {
            let last = changes.last().expect("a full page is non-empty");
            self.keyspace
                .decode_change_key(self.scope().collection_id().as_str(), &last.key)?
                .position
        };
        batch.put(&progress_key, progress.encode()?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    catalog_condition(&catalog_key, catalog_bytes),
                    KeyCondition::ValueEquals {
                        key: progress_key,
                        value: stored_progress,
                    },
                ],
                batch,
                durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(progress),
            CompareAndWriteResult::Conflict { .. } => Err(IndexBuildError::ProgressChanged),
        }
    }

    /// Establishes an activation fence and switches the catalog state only
    /// after durable backfill and change-log catch-up are proven complete.
    pub async fn activate_index(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
    ) -> Result<IndexActivationOutcome, IndexBuildError> {
        let progress = self
            .index_build_progress(name, version)
            .await?
            .ok_or(IndexBuildError::ProgressNotInitialized)?;
        if !progress.backfill_complete {
            return Err(IndexBuildError::BackfillIncomplete);
        }
        let target = self.committed_high_water_for_build().await?;
        if progress.caught_up_position < target {
            return Err(IndexBuildError::CatchUpIncomplete {
                caught_up: progress.caught_up_position,
                target,
            });
        }

        let definition = self
            .index_definition(name, version)
            .await?
            .ok_or(IndexError::DefinitionNotFound)?;
        if definition.kind() == crate::IndexKind::Unique
            && let Some(failed) = self
                .prepare_unique_ownership(&definition, durability)
                .await?
        {
            return Ok(IndexActivationOutcome::Failed(failed));
        }
        self.update_index_catalog(name, version, durability, |definition| {
            definition.set_activation_fenced()
        })
        .await?;
        let fenced_target = self.committed_high_water_for_build().await?;
        let fenced_progress = self
            .index_build_progress(name, version)
            .await?
            .ok_or(IndexBuildError::ProgressNotInitialized)?;
        if fenced_progress.caught_up_position < fenced_target {
            return Err(IndexBuildError::CatchUpIncomplete {
                caught_up: fenced_progress.caught_up_position,
                target: fenced_target,
            });
        }

        let definition = self
            .index_definition(name, version)
            .await?
            .ok_or(IndexError::DefinitionNotFound)?;
        if definition.kind() == crate::IndexKind::Unique
            && let Some(failed) = self
                .prepare_unique_ownership(&definition, durability)
                .await?
        {
            return Ok(IndexActivationOutcome::Failed(failed));
        }
        self.update_index_catalog(name, version, durability, |definition| {
            definition.activate()
        })
        .await?;
        let active = self
            .index_definition(name, version)
            .await?
            .ok_or(IndexError::DefinitionNotFound)?;
        Ok(IndexActivationOutcome::Active(active))
    }

    /// Makes an index ineligible for queries and write maintenance before
    /// deleting its physical entries, owner claims, and resumable build state.
    pub async fn remove_index(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
    ) -> Result<IndexRemovalReport, IndexBuildError> {
        self.mark_index_deleting(name, version, durability).await?;
        let definition = self
            .index_definition(name, version)
            .await?
            .ok_or(IndexError::DefinitionNotFound)?;
        let entries_removed = self
            .delete_index_range(
                self.keyspace.index_range(
                    self.scope().collection_id().as_str(),
                    definition.physical_name(),
                )?,
                durability,
            )
            .await?;
        let ownership_records_removed = self
            .delete_index_range(
                self.keyspace.unique_index_owners_range(
                    self.scope().collection_id().as_str(),
                    definition.physical_name(),
                )?,
                durability,
            )
            .await?;
        let progress_key = self.progress_key(name, version)?;
        let progress_removed = self.adapter.get(&progress_key).await?.is_some();
        if progress_removed {
            let mut batch = WriteBatch::new();
            batch.delete(progress_key);
            self.adapter.write(batch, durability).await?;
        }
        Ok(IndexRemovalReport {
            entries_removed,
            ownership_records_removed,
            progress_removed,
        })
    }

    async fn ensure_build_progress(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
    ) -> Result<IndexBuildProgress, IndexBuildError> {
        if let Some(progress) = self.index_build_progress(name, version).await? {
            return Ok(progress);
        }
        let snapshot = self.adapter.snapshot().await?;
        let catalog_key = self.index_catalog_key()?;
        let catalog_bytes = snapshot
            .get(&catalog_key)
            .await?
            .ok_or(IndexError::DefinitionNotFound)?;
        let catalog = IndexCatalog::decode(&catalog_bytes)?;
        require_building_definition(&catalog, name, version)?;
        let high_water_key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        let captured_position = snapshot
            .get(&high_water_key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()?
            .unwrap_or_default();
        let progress = IndexBuildProgress {
            name: name.clone(),
            version,
            captured_position,
            last_document_id: None,
            backfill_complete: false,
            caught_up_position: captured_position,
        };
        let progress_key = self.progress_key(name, version)?;
        let mut batch = WriteBatch::new();
        batch.put(&progress_key, progress.encode()?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::ValueEquals {
                        key: catalog_key,
                        value: catalog_bytes,
                    },
                    KeyCondition::Missing { key: progress_key },
                ],
                batch,
                durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(progress),
            CompareAndWriteResult::Conflict { .. } => self
                .index_build_progress(name, version)
                .await?
                .ok_or(IndexBuildError::ProgressChanged),
        }
    }

    async fn store_progress_only(
        &self,
        catalog_key: &[u8],
        catalog_bytes: Option<Vec<u8>>,
        progress: IndexBuildProgress,
        durability: Durability,
    ) -> Result<IndexBuildProgress, IndexBuildError> {
        let progress_key = self.progress_key(&progress.name, progress.version)?;
        let stored = self
            .adapter
            .get(&progress_key)
            .await?
            .ok_or(IndexBuildError::ProgressChanged)?;
        let mut batch = WriteBatch::new();
        batch.put(&progress_key, progress.encode()?);
        let result = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    catalog_condition(catalog_key, catalog_bytes),
                    KeyCondition::ValueEquals {
                        key: progress_key,
                        value: stored,
                    },
                ],
                batch,
                durability,
            })
            .await?;
        if result == CompareAndWriteResult::Applied {
            Ok(progress)
        } else {
            Err(IndexBuildError::ProgressChanged)
        }
    }

    pub(crate) fn progress_key(
        &self,
        name: &IndexName,
        version: IndexVersion,
    ) -> Result<Vec<u8>, IndexBuildError> {
        self.keyspace
            .index_build_progress_key(
                self.scope().collection_id().as_str(),
                name.as_str(),
                version.get(),
            )
            .map_err(Into::into)
    }

    fn ensure_build_capacity(
        &self,
        page_size: NonZeroUsize,
        durability: Durability,
    ) -> Result<(), IndexBuildError> {
        let capabilities = self.adapter.capabilities();
        if capabilities.strongest_durability < durability {
            return Err(IndexBuildError::UnsupportedDurability(durability));
        }
        if page_size.get() >= capabilities.maximum_batch_operations.get() {
            return Err(IndexBuildError::PageTooLarge {
                requested: page_size.get(),
                maximum: capabilities.maximum_batch_operations.get() - 1,
            });
        }
        if page_size > capabilities.maximum_scan_items {
            return Err(IndexBuildError::PageTooLarge {
                requested: page_size.get(),
                maximum: capabilities.maximum_scan_items.get(),
            });
        }
        Ok(())
    }

    fn ensure_catch_up_capacity(
        &self,
        page_size: NonZeroUsize,
        durability: Durability,
    ) -> Result<(), IndexBuildError> {
        let capabilities = self.adapter.capabilities();
        if capabilities.strongest_durability < durability {
            return Err(IndexBuildError::UnsupportedDurability(durability));
        }
        let maximum_by_batch = capabilities
            .maximum_batch_operations
            .get()
            .saturating_sub(1)
            / 2;
        let maximum = maximum_by_batch.min(capabilities.maximum_scan_items.get());
        if page_size.get() > maximum {
            return Err(IndexBuildError::PageTooLarge {
                requested: page_size.get(),
                maximum,
            });
        }
        Ok(())
    }

    async fn committed_high_water_for_build(&self) -> Result<u64, IndexBuildError> {
        let key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(Into::into)
    }

    async fn delete_index_range(
        &self,
        mut range: mako_storage::KeyRange,
        durability: Durability,
    ) -> Result<usize, IndexBuildError> {
        let capabilities = self.adapter.capabilities();
        if capabilities.strongest_durability < durability {
            return Err(IndexBuildError::UnsupportedDurability(durability));
        }
        let limit = NonZeroUsize::new(
            capabilities
                .maximum_scan_items
                .get()
                .min(capabilities.maximum_batch_operations.get()),
        )
        .expect("adapter limits are non-zero");
        let mut removed = 0;
        loop {
            let page = self
                .adapter
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    limit,
                ))
                .await?;
            let Some(last_key) = page.last().map(|entry| entry.key.clone()) else {
                break;
            };
            let mut batch = WriteBatch::with_capacity(page.len());
            for entry in page {
                batch.delete(entry.key);
                removed += 1;
            }
            self.adapter.write(batch, durability).await?;
            let mut next = last_key;
            next.push(0);
            if next >= range.end_exclusive {
                break;
            }
            range.start_inclusive = next;
        }
        Ok(removed)
    }

    async fn prepare_unique_ownership(
        &self,
        definition: &IndexDefinition,
        durability: Durability,
    ) -> Result<Option<IndexDefinition>, IndexBuildError> {
        const MAX_RECONCILE_ATTEMPTS: usize = 16;
        for _ in 0..MAX_RECONCILE_ATTEMPTS {
            let snapshot = self.adapter.snapshot().await?;
            let entry_range = self.keyspace.index_range(
                self.scope().collection_id().as_str(),
                definition.physical_name(),
            )?;
            let entries = scan_snapshot_all(
                snapshot.as_ref(),
                entry_range,
                self.adapter.capabilities().maximum_scan_items,
            )
            .await?;
            let mut expected = BTreeMap::<Vec<u8>, Vec<u8>>::new();
            let mut duplicate_values = 0_u64;
            for entry in entries {
                let decoded = self.keyspace.decode_index_entry_key(
                    self.scope().collection_id().as_str(),
                    definition.physical_name(),
                    &entry.key,
                )?;
                let owner_key = self.keyspace.unique_index_owner_key(
                    self.scope().collection_id().as_str(),
                    definition.physical_name(),
                    &decoded.components,
                )?;
                if expected
                    .insert(owner_key, decoded.document.clone())
                    .is_some_and(|owner| owner != decoded.document)
                {
                    duplicate_values = duplicate_values.saturating_add(1);
                }
            }
            if duplicate_values != 0 {
                return self
                    .fail_duplicate_index(definition, duplicate_values, durability)
                    .await
                    .map(Some);
            }

            let owner_range = self.keyspace.unique_index_owners_range(
                self.scope().collection_id().as_str(),
                definition.physical_name(),
            )?;
            let owners = scan_snapshot_all(
                snapshot.as_ref(),
                owner_range,
                self.adapter.capabilities().maximum_scan_items,
            )
            .await?;
            let actual: BTreeMap<_, _> = owners
                .into_iter()
                .map(|entry| (entry.key, entry.value))
                .collect();
            let mut repairs = Vec::new();
            for (key, expected_owner) in &expected {
                match actual.get(key) {
                    Some(actual_owner) if actual_owner == expected_owner => {}
                    Some(actual_owner) => repairs.push(OwnerRepair::Replace {
                        key: key.clone(),
                        expected: actual_owner.clone(),
                        value: expected_owner.clone(),
                    }),
                    None => repairs.push(OwnerRepair::Insert {
                        key: key.clone(),
                        value: expected_owner.clone(),
                    }),
                }
            }
            for (key, actual_owner) in &actual {
                if !expected.contains_key(key) {
                    repairs.push(OwnerRepair::Delete {
                        key: key.clone(),
                        expected: actual_owner.clone(),
                    });
                }
            }
            if repairs.is_empty() {
                return Ok(None);
            }

            let maximum = self.adapter.capabilities().maximum_batch_operations.get();
            let mut conflicted = false;
            for chunk in repairs.chunks(maximum) {
                let (catalog_key, catalog_bytes, catalog) = self.load_index_catalog().await?;
                require_building_definition(&catalog, definition.name(), definition.version())?;
                let mut conditions = vec![catalog_condition(&catalog_key, catalog_bytes)];
                let mut batch = WriteBatch::with_capacity(chunk.len());
                for repair in chunk {
                    repair.append(&mut conditions, &mut batch);
                }
                if self
                    .adapter
                    .compare_and_write(AtomicWrite {
                        conditions,
                        batch,
                        durability,
                    })
                    .await?
                    != CompareAndWriteResult::Applied
                {
                    conflicted = true;
                    break;
                }
            }
            if !conflicted {
                continue;
            }
        }
        Err(IndexBuildError::OwnershipDidNotStabilize)
    }

    async fn fail_duplicate_index(
        &self,
        definition: &IndexDefinition,
        duplicate_values: u64,
        durability: Durability,
    ) -> Result<IndexDefinition, IndexBuildError> {
        let failure = crate::IndexFailure::new(
            crate::IndexFailureCode::DuplicateValues,
            duplicate_values,
            "unique index build found duplicate indexed values",
        )?;
        self.update_index_catalog(
            definition.name(),
            definition.version(),
            durability,
            |definition| definition.fail(failure.clone()),
        )
        .await?;
        self.index_definition(definition.name(), definition.version())
            .await?
            .ok_or(IndexError::DefinitionNotFound.into())
    }
}

#[derive(Debug)]
enum OwnerRepair {
    Insert {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Replace {
        key: Vec<u8>,
        expected: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        key: Vec<u8>,
        expected: Vec<u8>,
    },
}

impl OwnerRepair {
    fn append(&self, conditions: &mut Vec<KeyCondition>, batch: &mut WriteBatch) {
        match self {
            Self::Insert { key, value } => {
                conditions.push(KeyCondition::Missing { key: key.clone() });
                batch.put(key, value);
            }
            Self::Replace {
                key,
                expected,
                value,
            } => {
                conditions.push(KeyCondition::ValueEquals {
                    key: key.clone(),
                    value: expected.clone(),
                });
                batch.put(key, value);
            }
            Self::Delete { key, expected } => {
                conditions.push(KeyCondition::ValueEquals {
                    key: key.clone(),
                    value: expected.clone(),
                });
                batch.delete(key);
            }
        }
    }
}

async fn scan_snapshot_all(
    snapshot: &dyn mako_storage::KvSnapshot,
    mut range: mako_storage::KeyRange,
    limit: NonZeroUsize,
) -> Result<Vec<mako_storage::KeyValue>, IndexBuildError> {
    let mut output = Vec::new();
    loop {
        let page = snapshot
            .scan(ScanRequest::new(
                range.clone(),
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let full = page.len() == limit.get();
        let Some(last_key) = page.last().map(|entry| entry.key.clone()) else {
            break;
        };
        output.extend(page);
        if !full {
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

fn append_replayed_index_delta(
    batch: &mut WriteBatch,
    keyspace: &mako_storage::TenantKeyspace,
    definition: &IndexDefinition,
    old: Option<&CanonicalDocument>,
    new: &CanonicalDocument,
) -> Result<(), IndexBuildError> {
    let old_key = old
        .filter(|document| !document.is_deleted())
        .map(|document| definition.entry_key(keyspace, document))
        .transpose()?;
    let new_key = (!new.is_deleted())
        .then(|| definition.entry_key(keyspace, new))
        .transpose()?;
    if old_key != new_key {
        if let Some(key) = old_key {
            batch.delete(key);
        }
        if let Some(key) = new_key {
            batch.put(key, []);
        }
    }
    Ok(())
}

fn require_building_definition<'a>(
    catalog: &'a IndexCatalog,
    name: &IndexName,
    version: IndexVersion,
) -> Result<&'a IndexDefinition, IndexBuildError> {
    let definition = catalog
        .definitions()
        .iter()
        .find(|definition| definition.name() == name && definition.version() == version)
        .ok_or(IndexError::DefinitionNotFound)?;
    if definition.state() != IndexState::Building {
        return Err(IndexBuildError::IndexNotBuilding(definition.state()));
    }
    Ok(definition)
}

fn catalog_condition(key: &[u8], stored: Option<Vec<u8>>) -> KeyCondition {
    match stored {
        Some(value) => KeyCondition::ValueEquals {
            key: key.to_vec(),
            value,
        },
        None => KeyCondition::Missing { key: key.to_vec() },
    }
}

#[derive(Debug)]
pub enum IndexBuildError {
    Index(IndexError),
    Scope(DocumentEngineScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Document(DocumentCodecError),
    Sequencer(SequencerError),
    Mutation(MutationError),
    Json(serde_json::Error),
    UnsupportedProgressEncoding(u8),
    InvalidProgress(&'static str),
    IndexNotBuilding(IndexState),
    ProgressNotInitialized,
    BackfillIncomplete,
    CatchUpIncomplete { caught_up: u64, target: u64 },
    RequiredHistoryMissing(u64),
    InvalidChangeHistory,
    OwnershipDidNotStabilize,
    ProgressChanged,
    UnsupportedDurability(Durability),
    PageTooLarge { requested: usize, maximum: usize },
}

impl fmt::Display for IndexBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Index(error) => error.fmt(formatter),
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::Sequencer(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid index-build encoding: {error}"),
            Self::UnsupportedProgressEncoding(version) => {
                write!(
                    formatter,
                    "unsupported index-build progress encoding {version}"
                )
            }
            Self::InvalidProgress(reason) => write!(formatter, "invalid index progress: {reason}"),
            Self::IndexNotBuilding(state) => write!(formatter, "index is {state:?}, not building"),
            Self::ProgressNotInitialized => {
                formatter.write_str("index build progress has not been initialized")
            }
            Self::BackfillIncomplete => formatter.write_str("index backfill is not complete"),
            Self::CatchUpIncomplete { caught_up, target } => write!(
                formatter,
                "index catch-up reached {caught_up} but committed high water is {target}"
            ),
            Self::RequiredHistoryMissing(position) => write!(
                formatter,
                "index catch-up requires retained revision history at position {position}"
            ),
            Self::InvalidChangeHistory => {
                formatter.write_str("change record has inconsistent previous revision metadata")
            }
            Self::OwnershipDidNotStabilize => formatter
                .write_str("unique index ownership did not stabilize under concurrent writes"),
            Self::ProgressChanged => {
                formatter.write_str("index build progress changed concurrently")
            }
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::PageTooLarge { requested, maximum } => write!(
                formatter,
                "index backfill page size {requested} exceeds maximum {maximum}"
            ),
        }
    }
}

impl Error for IndexBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Index(error) => Some(error),
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::Sequencer(error) => Some(error),
            Self::Mutation(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<IndexError> for IndexBuildError {
    fn from(error: IndexError) -> Self {
        Self::Index(error)
    }
}

impl From<DocumentEngineScopeError> for IndexBuildError {
    fn from(error: DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for IndexBuildError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for IndexBuildError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<DocumentCodecError> for IndexBuildError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

impl From<SequencerError> for IndexBuildError {
    fn from(error: SequencerError) -> Self {
        Self::Sequencer(error)
    }
}

impl From<MutationError> for IndexBuildError {
    fn from(error: MutationError) -> Self {
        Self::Mutation(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, IndexField, IndexKind, MutationCommitOutcome,
        MutationId, MutationInput, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
    };

    #[test]
    fn snapshot_pages_persist_capture_and_resume_metadata() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let (engine, scoped, sequencer, validator) = setup(adapter.clone());
            let mut lease = sequencer
                .lease(NonZeroU64::new(2).expect("non-zero"))
                .await
                .expect("lease");
            for id in ["todo-1", "todo-2"] {
                let position = lease.issue().expect("position");
                let outcome = scoped
                    .create_document(MutationInput {
                        mutation_id: MutationId::parse(format!("create-{id}")).expect("mutation"),
                        commit_position: CommitPosition::new(position).expect("position"),
                        document: validator
                            .validate_create(json!({"id": id, "title": id}))
                            .expect("document"),
                        durability: Durability::Memory,
                    })
                    .await
                    .expect("create");
                assert!(matches!(outcome, MutationCommitOutcome::Applied(_)));
            }
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 2);

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

            let first = scoped
                .backfill_index(
                    &name,
                    version,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("first page");
            assert_eq!(first.captured_position(), 2);
            assert_eq!(first.last_document_id().expect("cursor").as_str(), "todo-1");
            assert!(!first.backfill_complete());

            let restarted = engine
                .scope_collection(&tenant(), collection())
                .expect("restarted scope");
            let second = restarted
                .backfill_index(
                    &name,
                    version,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("resumed page");
            assert_eq!(
                second.last_document_id().expect("cursor").as_str(),
                "todo-2"
            );
            let complete = restarted
                .backfill_index(
                    &name,
                    version,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("complete");
            assert!(complete.backfill_complete());

            let range = restarted
                .keyspace
                .index_range("todos", "by-title@1")
                .expect("range");
            assert_eq!(
                adapter
                    .dump()
                    .expect("state")
                    .keys()
                    .filter(|key| range.contains(key))
                    .count(),
                2
            );
        });
    }

    #[test]
    fn change_log_catch_up_precedes_atomic_activation() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let (_, scoped, sequencer, validator) = setup(adapter.clone());
            let mut lease = sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");
            let created = create(&scoped, &validator, &mut lease, "todo-1", "old").await;
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 1);

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
            let backfill = scoped
                .backfill_index(
                    &name,
                    version,
                    NonZeroUsize::new(10).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("backfill");
            assert!(backfill.backfill_complete());
            assert_eq!(backfill.captured_position(), 1);

            let update_position = lease.issue().expect("position");
            let update = MutationInput {
                mutation_id: MutationId::parse("update-during-build").expect("mutation"),
                commit_position: CommitPosition::new(update_position).expect("position"),
                document: validator
                    .validate_create(json!({"id": "todo-1", "title": "new"}))
                    .expect("document"),
                durability: Durability::Memory,
            };
            let MutationCommitOutcome::Applied(_) = scoped
                .update_document(created.revision, update)
                .await
                .expect("update")
            else {
                panic!("update must apply");
            };
            create(&scoped, &validator, &mut lease, "todo-2", "second").await;
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 3);
            assert!(matches!(
                scoped
                    .activate_index(&name, version, Durability::Memory)
                    .await,
                Err(IndexBuildError::CatchUpIncomplete {
                    caught_up: 1,
                    target: 3
                })
            ));

            let first = scoped
                .catch_up_index(
                    &name,
                    version,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("first catch-up");
            assert_eq!(first.caught_up_position(), 2);
            let second = scoped
                .catch_up_index(
                    &name,
                    version,
                    NonZeroUsize::new(1).expect("non-zero"),
                    Durability::Memory,
                )
                .await
                .expect("second catch-up");
            assert_eq!(second.caught_up_position(), 3);
            let IndexActivationOutcome::Active(active) = scoped
                .activate_index(&name, version, Durability::Memory)
                .await
                .expect("activate")
            else {
                panic!("index must activate");
            };
            assert_eq!(active.state(), IndexState::Active);

            let entries = adapter.dump().expect("state");
            let range = scoped
                .keyspace
                .index_range("todos", "by-title@1")
                .expect("range");
            let indexed: Vec<_> = entries.keys().filter(|key| range.contains(key)).collect();
            assert_eq!(indexed.len(), 2);
            let old_component = [crate::IndexValue::String("old".to_owned())
                .encode(crate::IndexDirection::Ascending)];
            let old_key = scoped
                .keyspace
                .index_entry_key("todos", "by-title@1", &old_component, "todo-1")
                .expect("old key");
            assert!(!entries.contains_key(&old_key));
        });
    }

    #[test]
    fn duplicate_unique_build_fails_with_non_sensitive_diagnostics() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let (_, scoped, sequencer, validator) = setup(adapter);
            let mut lease = sequencer
                .lease(NonZeroU64::new(2).expect("non-zero"))
                .await
                .expect("lease");
            create(
                &scoped,
                &validator,
                &mut lease,
                "private-doc-1",
                "classified-value",
            )
            .await;
            create(
                &scoped,
                &validator,
                &mut lease,
                "private-doc-2",
                "classified-value",
            )
            .await;
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 2);

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
            assert!(
                scoped
                    .backfill_index(
                        &name,
                        version,
                        NonZeroUsize::new(10).expect("non-zero"),
                        Durability::Memory,
                    )
                    .await
                    .expect("backfill")
                    .backfill_complete()
            );
            let IndexActivationOutcome::Failed(failed) = scoped
                .activate_index(&name, version, Durability::Memory)
                .await
                .expect("activation outcome")
            else {
                panic!("duplicate build must fail");
            };
            assert_eq!(failed.state(), IndexState::Failed);
            let failure = failed.failure().expect("failure diagnostics");
            assert_eq!(failure.code(), crate::IndexFailureCode::DuplicateValues);
            assert!(failure.affected_values() >= 1);
            assert!(!failure.safe_message().contains("private-doc"));
            assert!(!failure.safe_message().contains("classified-value"));
        });
    }

    #[test]
    fn removal_disables_maintenance_before_cleaning_entries_and_owners() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let (_, scoped, sequencer, validator) = setup(adapter.clone());
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
            let mut lease = sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");
            create(&scoped, &validator, &mut lease, "todo-1", "first").await;
            create(&scoped, &validator, &mut lease, "todo-2", "second").await;

            // A live index is never forgotten: only one removal has emptied.
            assert!(matches!(
                scoped
                    .forget_index(&name, version, Durability::Memory)
                    .await,
                Err(IndexError::InvalidStateTransition { .. })
            ));
            let report = scoped
                .remove_index(&name, version, Durability::Memory)
                .await
                .expect("remove index");
            assert_eq!(report.entries_removed(), 2);
            assert_eq!(report.ownership_records_removed(), 2);
            assert!(!report.progress_removed());
            assert_eq!(
                scoped
                    .index_definition(&name, version)
                    .await
                    .expect("definition")
                    .expect("index")
                    .state(),
                IndexState::Deleting
            );

            create(&scoped, &validator, &mut lease, "todo-3", "first").await;
            let state = adapter.dump().expect("state");
            let entry_range = scoped
                .keyspace
                .index_range("todos", "unique-title@1")
                .expect("entry range");
            let owner_range = scoped
                .keyspace
                .unique_index_owners_range("todos", "unique-title@1")
                .expect("owner range");
            assert!(!state.keys().any(|key| entry_range.contains(key)));
            assert!(!state.keys().any(|key| owner_range.contains(key)));

            // Once its entries are gone the definition itself can go, and
            // writes keep working against the catalog without it. Forgetting
            // again is a no-op, so a retried removal converges.
            scoped
                .forget_index(&name, version, Durability::Memory)
                .await
                .expect("forget index");
            assert!(
                scoped
                    .index_definition(&name, version)
                    .await
                    .expect("definition")
                    .is_none()
            );
            scoped
                .forget_index(&name, version, Durability::Memory)
                .await
                .expect("forget again");
            let mut lease = sequencer
                .lease(NonZeroU64::new(1).expect("non-zero"))
                .await
                .expect("lease");
            create(&scoped, &validator, &mut lease, "todo-4", "first").await;
        });
    }

    async fn create(
        scoped: &ScopedCollectionEngine,
        validator: &DocumentValidator,
        lease: &mut crate::SequenceLease,
        id: &str,
        title: &str,
    ) -> crate::MutationReceipt {
        let position = lease.issue().expect("position");
        let MutationCommitOutcome::Applied(receipt) = scoped
            .create_document(MutationInput {
                mutation_id: MutationId::parse(format!("create-{id}")).expect("mutation"),
                commit_position: CommitPosition::new(position).expect("position"),
                document: validator
                    .validate_create(json!({"id": id, "title": title}))
                    .expect("document"),
                durability: Durability::Memory,
            })
            .await
            .expect("create")
        else {
            panic!("create must apply");
        };
        receipt
    }

    fn setup(
        adapter: Arc<MemoryAdapter>,
    ) -> (
        DocumentEngine,
        ScopedCollectionEngine,
        crate::EnvironmentSequencer,
        DocumentValidator,
    ) {
        let engine = DocumentEngine::new(adapter);
        let tenant = tenant();
        let scoped = engine
            .scope_collection(&tenant, collection())
            .expect("scope");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Memory)
            .expect("sequencer");
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}, "title": {"type": "string"}},
                "required": ["id", "title"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        let validator = DocumentValidator::compile(&metadata).expect("validator");
        (engine, scoped, sequencer, validator)
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    fn collection() -> CollectionScope {
        CollectionScope::new(tenant(), CollectionId::parse("todos").expect("collection"))
    }
}
