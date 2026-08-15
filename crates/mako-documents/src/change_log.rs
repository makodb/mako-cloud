use std::{error::Error, fmt, num::NonZeroUsize};

use mako_storage::{KeyCodecError, ScanDirection, ScanRequest, SequencerKeyKind, StorageError};

use crate::{
    CanonicalDocument, ChangeRecord, DocumentCodecError, MutationError, ScopedCollectionEngine,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ChangeDocument {
    change: ChangeRecord,
    previous_document: Option<CanonicalDocument>,
    document: CanonicalDocument,
}

impl ChangeDocument {
    #[must_use]
    pub fn change(&self) -> &ChangeRecord {
        &self.change
    }

    #[must_use]
    pub fn previous_document(&self) -> Option<&CanonicalDocument> {
        self.previous_document.as_ref()
    }

    #[must_use]
    pub fn document(&self) -> &CanonicalDocument {
        &self.document
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangePage {
    changes: Vec<ChangeDocument>,
    scanned_through: u64,
    captured_high_water: u64,
    exhausted: bool,
}

impl ChangePage {
    #[must_use]
    pub fn changes(&self) -> &[ChangeDocument] {
        &self.changes
    }

    #[must_use]
    pub const fn scanned_through(&self) -> u64 {
        self.scanned_through
    }

    #[must_use]
    pub const fn captured_high_water(&self) -> u64 {
        self.captured_high_water
    }

    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}

impl ScopedCollectionEngine {
    pub async fn capture_committed_high_water(&self) -> Result<u64, ChangeLogError> {
        let key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub async fn read_change_page(
        &self,
        after_position: u64,
        captured_high_water: u64,
        limit: NonZeroUsize,
    ) -> Result<ChangePage, ChangeLogError> {
        let current_high_water = self.capture_committed_high_water().await?;
        if captured_high_water > current_high_water {
            return Err(ChangeLogError::BeyondCommittedHighWater {
                requested: captured_high_water,
                committed: current_high_water,
            });
        }
        if after_position >= captured_high_water {
            return Ok(ChangePage {
                changes: Vec::new(),
                scanned_through: captured_high_water,
                captured_high_water,
                exhausted: true,
            });
        }
        let snapshot = self.adapter.snapshot().await?;
        let range = self.keyspace.changes_after_through(
            self.scope().collection_id().as_str(),
            after_position,
            captured_high_water,
        )?;
        let entries = snapshot
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await?;
        let mut changes = Vec::with_capacity(entries.len());
        let mut last_position = after_position;
        for entry in entries {
            let decoded_key = self
                .keyspace
                .decode_change_key(self.scope().collection_id().as_str(), &entry.key)?;
            let change = ChangeRecord::decode(&entry.value)?;
            if change.commit_position() != decoded_key.position
                || change.document_id().as_bytes() != decoded_key.document
                || change.commit_position() <= last_position
                || change.commit_position() > captured_high_water
            {
                return Err(ChangeLogError::CorruptChangeOrder);
            }
            let revision_key = self.keyspace.revision_key(
                self.scope().collection_id().as_str(),
                change.commit_position(),
                change.document_id(),
                change.revision(),
            )?;
            let document_bytes = snapshot
                .get(&revision_key)
                .await?
                .ok_or(ChangeLogError::MissingRevision(change.commit_position()))?;
            let document = CanonicalDocument::decode(&document_bytes)?;
            if document.commit_position().get() != change.commit_position()
                || document.primary_key().as_str() != change.document_id()
                || document.revision().as_str() != change.revision()
                || document.is_deleted() != change.is_deleted()
            {
                return Err(ChangeLogError::CorruptChangeRevision);
            }
            let previous_document = match (
                change.previous_commit_position(),
                change.previous_revision(),
            ) {
                (Some(position), Some(revision)) => {
                    let revision_key = self.keyspace.revision_key(
                        self.scope().collection_id().as_str(),
                        position,
                        change.document_id(),
                        revision,
                    )?;
                    let previous_bytes = snapshot
                        .get(&revision_key)
                        .await?
                        .ok_or(ChangeLogError::MissingRevision(position))?;
                    let previous = CanonicalDocument::decode(&previous_bytes)?;
                    if previous.commit_position().get() != position
                        || previous.primary_key().as_str() != change.document_id()
                        || previous.revision().as_str() != revision
                    {
                        return Err(ChangeLogError::CorruptChangeRevision);
                    }
                    Some(previous)
                }
                (None, None) => None,
                _ => return Err(ChangeLogError::CorruptChangeRevision),
            };
            last_position = change.commit_position();
            changes.push(ChangeDocument {
                change,
                previous_document,
                document,
            });
        }
        let exhausted = changes.len() < limit.get() || last_position == captured_high_water;
        Ok(ChangePage {
            changes,
            scanned_through: if exhausted {
                captured_high_water
            } else {
                last_position
            },
            captured_high_water,
            exhausted,
        })
    }
}

fn decode_high_water(bytes: &[u8]) -> Result<u64, ChangeLogError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| ChangeLogError::CorruptHighWater)?;
    Ok(u64::from_be_bytes(encoded))
}

#[derive(Debug)]
pub enum ChangeLogError {
    Storage(StorageError),
    KeyCodec(KeyCodecError),
    Mutation(MutationError),
    Document(DocumentCodecError),
    CorruptHighWater,
    BeyondCommittedHighWater { requested: u64, committed: u64 },
    CorruptChangeOrder,
    MissingRevision(u64),
    CorruptChangeRevision,
}

impl fmt::Display for ChangeLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::CorruptHighWater => formatter.write_str("committed high water is corrupt"),
            Self::BeyondCommittedHighWater {
                requested,
                committed,
            } => write!(
                formatter,
                "requested change high water {requested} exceeds committed high water {committed}"
            ),
            Self::CorruptChangeOrder => formatter.write_str("change log ordering is corrupt"),
            Self::MissingRevision(position) => {
                write!(formatter, "change revision {position} is missing")
            }
            Self::CorruptChangeRevision => {
                formatter.write_str("change record and revision disagree")
            }
        }
    }
}

impl Error for ChangeLogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Mutation(error) => Some(error),
            Self::Document(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StorageError> for ChangeLogError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<KeyCodecError> for ChangeLogError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<MutationError> for ChangeLogError {
    fn from(error: MutationError) -> Self {
        Self::Mutation(error)
    }
}

impl From<DocumentCodecError> for ChangeLogError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, sync::Arc};

    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{Durability, KvAdapter, MemoryAdapter, WriteBatch};
    use serde_json::json;

    use super::*;
    use crate::{CommitPosition, DocumentEngine, DocumentId, RevisionToken, SchemaVersion};

    #[test]
    fn captured_high_water_excludes_later_writes_and_pages_in_commit_order() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant();
            let scoped = DocumentEngine::new(adapter.clone())
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("scope");
            write_change(adapter.as_ref(), &scoped, 1, "doc-a").await;
            write_change(adapter.as_ref(), &scoped, 2, "doc-b").await;
            write_high_water(adapter.as_ref(), &scoped, 2).await;
            let captured = scoped
                .capture_committed_high_water()
                .await
                .expect("capture");
            write_change(adapter.as_ref(), &scoped, 3, "doc-c").await;
            write_high_water(adapter.as_ref(), &scoped, 3).await;

            let first = scoped
                .read_change_page(0, captured, NonZeroUsize::new(1).expect("limit"))
                .await
                .expect("first");
            assert_eq!(first.changes()[0].change().document_id(), "doc-a");
            assert_eq!(first.scanned_through(), 1);
            assert!(!first.is_exhausted());
            let second = scoped
                .read_change_page(
                    first.scanned_through(),
                    captured,
                    NonZeroUsize::new(10).expect("limit"),
                )
                .await
                .expect("second");
            assert_eq!(second.changes()[0].change().document_id(), "doc-b");
            assert_eq!(second.scanned_through(), 2);
            assert!(second.is_exhausted());
            assert!(
                second
                    .changes()
                    .iter()
                    .all(|change| change.change().document_id() != "doc-c")
            );
        });
    }

    async fn write_change(
        adapter: &MemoryAdapter,
        scoped: &ScopedCollectionEngine,
        position: u64,
        document_id: &str,
    ) {
        let document = CanonicalDocument::new(
            DocumentId::parse(document_id).expect("document"),
            SchemaVersion::new(1).expect("schema"),
            RevisionToken::parse(format!("rev-{position}")).expect("revision"),
            CommitPosition::new(position).expect("position"),
            false,
            json!({"id": document_id, "value": position}),
        )
        .expect("document");
        let change = json!({
            "encoding_version": 2,
            "document_id": document_id,
            "previous_revision": null,
            "previous_commit_position": null,
            "revision": format!("rev-{position}"),
            "commit_position": position,
            "_deleted": false
        });
        let mut batch = WriteBatch::new();
        batch.put(
            scoped
                .keyspace
                .change_key("todos", position, document_id)
                .expect("change key"),
            serde_json::to_vec(&change).expect("change"),
        );
        batch.put(
            scoped
                .keyspace
                .revision_key("todos", position, document_id, format!("rev-{position}"))
                .expect("revision key"),
            document.encode().expect("document"),
        );
        adapter
            .write(batch, Durability::Memory)
            .await
            .expect("write");
    }

    async fn write_high_water(
        adapter: &MemoryAdapter,
        scoped: &ScopedCollectionEngine,
        position: u64,
    ) {
        let mut batch = WriteBatch::new();
        batch.put(
            scoped.keyspace.sequencer_key(SequencerKeyKind::HighWater),
            position.to_be_bytes(),
        );
        adapter
            .write(batch, Durability::Memory)
            .await
            .expect("high water");
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
