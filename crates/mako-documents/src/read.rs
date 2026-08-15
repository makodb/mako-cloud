use std::{error::Error, fmt, num::NonZeroUsize};

use mako_api::CollectionScope;
use mako_storage::{
    KeyCodecError, KvSnapshot, ScanDirection, ScanRequest, StorageError, TenantKeyspace,
};

use crate::{
    CanonicalDocument, DocumentCodecError, DocumentId, DocumentReadAuthorizer,
    ReadAuthorizationContext, ReadAuthorizationPath, ScopedCollectionEngine,
};

impl ScopedCollectionEngine {
    pub async fn get_document(
        &self,
        document_id: &DocumentId,
    ) -> Result<Option<CanonicalDocument>, DocumentReadError> {
        let key = self
            .document_key(document_id)
            .map_err(DocumentReadError::Scope)?;
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(CanonicalDocument::decode)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn get_document_authorized(
        &self,
        document_id: &DocumentId,
        authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<Option<CanonicalDocument>, DocumentReadError> {
        let document = self.get_document(document_id).await?;
        Ok(document.filter(|document| {
            authorizer
                .authorize_read(ReadAuthorizationContext::new(
                    self,
                    ReadAuthorizationPath::Point,
                    document,
                ))
                .is_allowed()
        }))
    }

    pub async fn snapshot(&self) -> Result<ScopedCollectionSnapshot, DocumentReadError> {
        Ok(ScopedCollectionSnapshot {
            snapshot: self.adapter.snapshot().await?,
            engine: self.clone(),
            scope: self.scope().clone(),
            keyspace: self.keyspace.clone(),
        })
    }
}

/// A stable read view bound to the same verified collection as its parent engine.
pub struct ScopedCollectionSnapshot {
    pub(crate) snapshot: Box<dyn KvSnapshot>,
    pub(crate) engine: ScopedCollectionEngine,
    scope: CollectionScope,
    keyspace: TenantKeyspace,
}

impl fmt::Debug for ScopedCollectionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopedCollectionSnapshot")
            .field("snapshot_id", &self.snapshot.id())
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl ScopedCollectionSnapshot {
    #[must_use]
    pub fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    #[must_use]
    pub fn snapshot_id(&self) -> u64 {
        self.snapshot.id().0
    }

    pub async fn get_document(
        &self,
        document_id: &DocumentId,
    ) -> Result<Option<CanonicalDocument>, DocumentReadError> {
        let key = self
            .keyspace
            .document_key(self.scope.collection_id().as_str(), document_id.as_str())?;
        self.snapshot
            .get(&key)
            .await?
            .as_deref()
            .map(CanonicalDocument::decode)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn get_documents(
        &self,
        document_ids: &[DocumentId],
    ) -> Result<Vec<Option<CanonicalDocument>>, DocumentReadError> {
        let mut documents = Vec::with_capacity(document_ids.len());
        for document_id in document_ids {
            documents.push(self.get_document(document_id).await?);
        }
        Ok(documents)
    }

    pub async fn browse_primary_keys(
        &self,
        options: PrimaryKeyBrowseOptions,
        after: Option<&DocumentId>,
        authorizer: Option<&dyn DocumentReadAuthorizer>,
    ) -> Result<PrimaryKeyBrowsePage, DocumentReadError> {
        let mut range = self
            .keyspace
            .documents_range(self.scope.collection_id().as_str())?;
        if let Some(after) = after {
            let mut start = self
                .keyspace
                .document_key(self.scope.collection_id().as_str(), after.as_str())?;
            start.push(0);
            if start >= range.end_exclusive {
                return Ok(PrimaryKeyBrowsePage {
                    documents: Vec::new(),
                    next_after: None,
                    snapshot_id: self.snapshot.id().0,
                    exhausted: true,
                });
            }
            range.start_inclusive = start;
        }

        let wanted = options.limit.get().saturating_add(1);
        let mut documents = Vec::with_capacity(wanted);
        let mut bytes = 0_usize;
        let mut has_more = false;
        loop {
            let remaining = wanted.saturating_sub(documents.len()).max(1);
            let scan_limit =
                NonZeroUsize::new(remaining.max(32).min(options.maximum_scan_items.get()))
                    .expect("primary browse scan limit is positive");
            let entries = self
                .snapshot
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    scan_limit,
                ))
                .await?;
            let page_was_full = entries.len() == scan_limit.get();
            let Some(last_key) = entries.last().map(|entry| entry.key.clone()) else {
                break;
            };
            for entry in entries {
                let document = CanonicalDocument::decode(&entry.value)?;
                if (!options.include_retained_tombstones && document.is_deleted())
                    || authorizer.is_some_and(|authorizer| {
                        !authorizer
                            .authorize_read(ReadAuthorizationContext::for_scope(
                                &self.scope,
                                ReadAuthorizationPath::ExplorerBrowse,
                                &document,
                            ))
                            .is_allowed()
                    })
                {
                    continue;
                }
                let encoded_bytes = entry.value.len();
                if bytes.saturating_add(encoded_bytes) > options.maximum_page_bytes.get() {
                    if documents.is_empty() {
                        return Err(DocumentReadError::PageByteLimit);
                    }
                    has_more = true;
                    break;
                }
                bytes = bytes.saturating_add(encoded_bytes);
                documents.push(document);
                if documents.len() == wanted {
                    has_more = true;
                    break;
                }
            }
            if has_more || !page_was_full {
                break;
            }
            let mut start = last_key;
            start.push(0);
            if start >= range.end_exclusive {
                break;
            }
            range.start_inclusive = start;
        }
        if documents.len() > options.limit.get() {
            documents.truncate(options.limit.get());
        }
        let next_after = has_more
            .then(|| {
                documents
                    .last()
                    .map(|document| document.primary_key().clone())
            })
            .flatten();
        Ok(PrimaryKeyBrowsePage {
            documents,
            next_after,
            snapshot_id: self.snapshot.id().0,
            exhausted: !has_more,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimaryKeyBrowseOptions {
    limit: NonZeroUsize,
    maximum_page_bytes: NonZeroUsize,
    maximum_scan_items: NonZeroUsize,
    include_retained_tombstones: bool,
}

impl PrimaryKeyBrowseOptions {
    #[must_use]
    pub fn new(
        limit: NonZeroUsize,
        maximum_page_bytes: NonZeroUsize,
        maximum_scan_items: NonZeroUsize,
        include_retained_tombstones: bool,
    ) -> Self {
        Self {
            limit,
            maximum_page_bytes,
            maximum_scan_items,
            include_retained_tombstones,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrimaryKeyBrowsePage {
    documents: Vec<CanonicalDocument>,
    next_after: Option<DocumentId>,
    snapshot_id: u64,
    exhausted: bool,
}

impl PrimaryKeyBrowsePage {
    #[must_use]
    pub fn documents(&self) -> &[CanonicalDocument] {
        &self.documents
    }

    #[must_use]
    pub fn next_after(&self) -> Option<&DocumentId> {
        self.next_after.as_ref()
    }

    #[must_use]
    pub const fn snapshot_id(&self) -> u64 {
        self.snapshot_id
    }

    #[must_use]
    pub const fn exhausted(&self) -> bool {
        self.exhausted
    }
}

#[derive(Debug)]
pub enum DocumentReadError {
    Scope(crate::DocumentEngineScopeError),
    KeyCodec(KeyCodecError),
    Storage(StorageError),
    Document(DocumentCodecError),
    PageByteLimit,
}

impl fmt::Display for DocumentReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::PageByteLimit => formatter.write_str("document exceeds browse page byte limit"),
        }
    }
}

impl Error for DocumentReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::PageByteLimit => None,
        }
    }
}

impl From<KeyCodecError> for DocumentReadError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for DocumentReadError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<DocumentCodecError> for DocumentReadError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{Durability, KvAdapter, MemoryAdapter, WriteBatch};
    use serde_json::json;

    use super::*;
    use crate::{
        CommitPosition, DocumentEngine, MutationAuthorizationDecision, ReadAuthorizationContext,
        RevisionToken, SchemaVersion,
    };

    struct HideMarked;

    impl DocumentReadAuthorizer for HideMarked {
        fn authorize_read(
            &self,
            context: ReadAuthorizationContext<'_>,
        ) -> MutationAuthorizationDecision {
            if context.document().body().get("value") == Some(&json!("hidden")) {
                MutationAuthorizationDecision::deny("hidden")
            } else {
                MutationAuthorizationDecision::allow("visible")
            }
        }
    }

    fn document(id: &str, revision: &str, position: u64, value: &str) -> CanonicalDocument {
        CanonicalDocument::new(
            DocumentId::parse(id).expect("document id"),
            SchemaVersion::new(1).expect("schema version"),
            RevisionToken::parse(revision).expect("revision"),
            CommitPosition::new(position).expect("position"),
            false,
            json!({"id": id, "value": value}),
        )
        .expect("document")
    }

    #[test]
    fn primary_reads_decode_documents_and_missing_keys() {
        block_on(async {
            let (adapter, scoped) = setup();
            let id = DocumentId::parse("doc-1").expect("document id");
            let key = scoped.document_key(&id).expect("key");
            let mut batch = WriteBatch::new();
            batch.put(
                key,
                document("doc-1", "rev-1", 1, "stored")
                    .encode()
                    .expect("encode"),
            );
            adapter
                .write(batch, Durability::Memory)
                .await
                .expect("write");

            assert_eq!(
                scoped
                    .get_document(&id)
                    .await
                    .expect("read")
                    .expect("document")
                    .revision()
                    .as_str(),
                "rev-1"
            );
            assert!(
                scoped
                    .get_document(&DocumentId::parse("missing").expect("id"))
                    .await
                    .expect("read")
                    .is_none()
            );
        });
    }

    #[test]
    fn multi_key_reads_remain_on_one_snapshot_during_concurrent_writes() {
        block_on(async {
            let (adapter, scoped) = setup();
            let first_id = DocumentId::parse("doc-1").expect("id");
            let second_id = DocumentId::parse("doc-2").expect("id");
            let first_key = scoped.document_key(&first_id).expect("key");
            let second_key = scoped.document_key(&second_id).expect("key");
            let mut initial = WriteBatch::new();
            initial.put(
                &first_key,
                document("doc-1", "rev-old-1", 1, "old")
                    .encode()
                    .expect("encode"),
            );
            initial.put(
                &second_key,
                document("doc-2", "rev-old-2", 2, "old")
                    .encode()
                    .expect("encode"),
            );
            adapter
                .write(initial, Durability::Memory)
                .await
                .expect("initial write");
            let snapshot = scoped.snapshot().await.expect("snapshot");

            let mut updated = WriteBatch::new();
            updated.put(
                &first_key,
                document("doc-1", "rev-new-1", 3, "new")
                    .encode()
                    .expect("encode"),
            );
            updated.put(
                &second_key,
                document("doc-2", "rev-new-2", 4, "new")
                    .encode()
                    .expect("encode"),
            );
            adapter
                .write(updated, Durability::Memory)
                .await
                .expect("updated write");

            let documents = snapshot
                .get_documents(&[first_id.clone(), second_id.clone()])
                .await
                .expect("snapshot read");
            assert_eq!(
                documents[0].as_ref().expect("first").revision().as_str(),
                "rev-old-1"
            );
            assert_eq!(
                documents[1].as_ref().expect("second").revision().as_str(),
                "rev-old-2"
            );
            assert_eq!(
                scoped
                    .get_document(&first_id)
                    .await
                    .expect("live read")
                    .expect("document")
                    .revision()
                    .as_str(),
                "rev-new-1"
            );
        });
    }

    #[test]
    fn primary_browse_is_snapshot_consistent_and_fills_past_hidden_rows() {
        block_on(async {
            let (adapter, scoped) = setup();
            let mut initial = WriteBatch::new();
            for (id, value, position) in [
                ("doc-1", "visible-a", 1),
                ("doc-2", "hidden", 2),
                ("doc-3", "visible-b", 3),
                ("doc-4", "visible-c", 4),
            ] {
                let document = document(id, &format!("rev-{position}"), position, value);
                initial.put(
                    scoped
                        .document_key(document.primary_key())
                        .expect("document key"),
                    document.encode().expect("encode"),
                );
            }
            adapter
                .write(initial, Durability::Memory)
                .await
                .expect("write");
            let snapshot = scoped.snapshot().await.expect("snapshot");

            let mut concurrent = WriteBatch::new();
            let changed = document("doc-3", "rev-new", 5, "changed-after-snapshot");
            concurrent.put(
                scoped
                    .document_key(changed.primary_key())
                    .expect("document key"),
                changed.encode().expect("encode"),
            );
            adapter
                .write(concurrent, Durability::Memory)
                .await
                .expect("concurrent write");

            let options = PrimaryKeyBrowseOptions::new(
                NonZeroUsize::new(2).expect("limit"),
                NonZeroUsize::new(1024 * 1024).expect("bytes"),
                NonZeroUsize::new(32).expect("scan"),
                false,
            );
            let first = snapshot
                .browse_primary_keys(options, None, Some(&HideMarked))
                .await
                .expect("browse");
            assert_eq!(
                first
                    .documents()
                    .iter()
                    .map(|document| document.primary_key().as_str())
                    .collect::<Vec<_>>(),
                ["doc-1", "doc-3"]
            );
            assert_eq!(
                first.documents()[1].body().get("value"),
                Some(&json!("visible-b"))
            );
            let second = snapshot
                .browse_primary_keys(options, first.next_after(), Some(&HideMarked))
                .await
                .expect("second browse");
            assert_eq!(second.documents().len(), 1);
            assert_eq!(second.documents()[0].primary_key().as_str(), "doc-4");
            assert!(second.exhausted());
        });
    }

    fn setup() -> (Arc<MemoryAdapter>, ScopedCollectionEngine) {
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
        (adapter, scoped)
    }
}
