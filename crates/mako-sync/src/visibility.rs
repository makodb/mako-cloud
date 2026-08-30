use mako_documents::{
    CanonicalDocument, ChangeDocument, DocumentReadAuthorizer, ReadAuthorizationContext,
    ReadAuthorizationPath, ScopedCollectionEngine,
};
use serde_json::Value;

use crate::ReplicationFilter;

/// The change one replication scope sees, or `None` when it sees nothing.
///
/// A filter narrows the scope after the policy has decided, and it narrows it
/// the same way the policy does: a document that used to be in the scope and
/// no longer is comes back as a tombstone. Without that, a transaction moved
/// to another household -- or one whose household field is corrected -- would
/// sit in the first household's local database for ever, because nothing
/// would ever tell that database it left.
pub(crate) fn replication_change(
    collection: &ScopedCollectionEngine,
    path: ReadAuthorizationPath,
    authorizer: &dyn DocumentReadAuthorizer,
    change: &ChangeDocument,
    filter: Option<&ReplicationFilter>,
) -> Option<Value> {
    let previous = change.previous_document();
    let current = change.document();
    let in_scope = |document: &CanonicalDocument| {
        is_readable(collection, path, authorizer, document)
            && filter.is_none_or(|filter| filter.matches(document.body()))
    };
    let previous_visible =
        previous.is_some_and(|document| !document.is_deleted() && in_scope(document));
    let current_visible = !current.is_deleted() && in_scope(current);

    if current_visible {
        Some(replication_document(current))
    } else if previous_visible {
        Some(if current.is_deleted() {
            replication_document(current)
        } else {
            synthetic_tombstone(
                previous.expect("previous visibility requires a previous document"),
                current,
            )
        })
    } else {
        None
    }
}

pub(crate) fn replication_document(document: &CanonicalDocument) -> Value {
    replication_state(document.body().clone(), document.is_deleted(), document)
}

fn synthetic_tombstone(previous: &CanonicalDocument, current: &CanonicalDocument) -> Value {
    // The previous state was already visible to this caller. Reusing it retains
    // field and composite primary keys without exposing the protected new state.
    replication_state(previous.body().clone(), true, current)
}

fn replication_state(
    mut body: serde_json::Map<String, Value>,
    deleted: bool,
    revision_source: &CanonicalDocument,
) -> Value {
    body.insert("_deleted".to_owned(), Value::Bool(deleted));
    body.insert(
        "_rev".to_owned(),
        Value::String(revision_source.revision().as_str().to_owned()),
    );
    Value::Object(body)
}

fn is_readable(
    collection: &ScopedCollectionEngine,
    path: ReadAuthorizationPath,
    authorizer: &dyn DocumentReadAuthorizer,
    document: &CanonicalDocument,
) -> bool {
    authorizer
        .authorize_read(ReadAuthorizationContext::new(collection, path, document))
        .is_allowed()
}
