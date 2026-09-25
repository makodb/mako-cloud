//! How a collection's stored documents fare against a schema.

use std::{error::Error, fmt, num::NonZeroUsize};

use mako_storage::{ScanDirection, ScanRequest, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CanonicalDocument, CollectionMetadata, DocumentCodecError, DocumentEngineScopeError,
    DocumentValidationError, DocumentValidator, ScopedCollectionEngine,
};

const PAGE_SIZE: usize = 256;
/// Failing documents named in one check; the count of checked ones is exact.
pub const MAX_REPORTED_FAILURES: usize = 100;

/// Every live document read, and the primary keys of up to
/// [`MAX_REPORTED_FAILURES`] that the schema refuses. Deleted documents are
/// tombstones with no body, and a schema says nothing about them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StoredDocumentCheck {
    pub documents_checked: u64,
    pub failing: Vec<String>,
}

impl StoredDocumentCheck {
    #[must_use]
    pub fn passes(&self) -> bool {
        self.failing.is_empty()
    }
}

#[derive(Debug)]
pub enum StoredDocumentCheckError {
    Scope(DocumentEngineScopeError),
    Schema(DocumentValidationError),
    Storage(StorageError),
    Codec(DocumentCodecError),
}

impl fmt::Display for StoredDocumentCheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Scope(_) => "collection scope is invalid",
            Self::Schema(_) => "schema could not be compiled",
            Self::Storage(_) => "stored documents could not be read",
            Self::Codec(_) => "a stored document could not be decoded",
        })
    }
}

impl Error for StoredDocumentCheckError {}

impl ScopedCollectionEngine {
    /// Reads every stored document of this collection and validates its body
    /// against `metadata`'s schema, as a write under that schema would be.
    pub async fn check_stored_documents(
        &self,
        metadata: &CollectionMetadata,
    ) -> Result<StoredDocumentCheck, StoredDocumentCheckError> {
        let validator =
            DocumentValidator::compile(metadata).map_err(StoredDocumentCheckError::Schema)?;
        let mut range = self
            .documents_range()
            .map_err(StoredDocumentCheckError::Scope)?;
        let page_size =
            NonZeroUsize::new(PAGE_SIZE.min(self.adapter.capabilities().maximum_scan_items.get()))
                .expect("page size is positive");
        let mut check = StoredDocumentCheck {
            documents_checked: 0,
            failing: Vec::new(),
        };
        loop {
            let page = self
                .adapter
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    page_size,
                ))
                .await
                .map_err(StoredDocumentCheckError::Storage)?;
            for entry in &page {
                let document = CanonicalDocument::decode(&entry.value)
                    .map_err(StoredDocumentCheckError::Codec)?;
                if document.is_deleted() {
                    continue;
                }
                check.documents_checked = check.documents_checked.saturating_add(1);
                if validator
                    .validate_update(&document, Value::Object(document.body().clone()))
                    .is_err()
                    && check.failing.len() < MAX_REPORTED_FAILURES
                {
                    check
                        .failing
                        .push(document.primary_key().as_str().to_owned());
                }
            }
            let Some(last) = page.last() else { break };
            if page.len() < page_size.get() {
                break;
            }
            range.start_inclusive = last.key.clone();
            range.start_inclusive.push(0);
            if range.start_inclusive >= range.end_exclusive {
                break;
            }
        }
        Ok(check)
    }
}
