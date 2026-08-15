use mako_documents::{
    CanonicalDocument, ChangeDocument, DocumentReadAuthorizer, ReadAuthorizationContext,
    ReadAuthorizationPath, ScopedCollectionEngine,
};
use serde_json::Value;

pub(crate) fn replication_change(
    collection: &ScopedCollectionEngine,
    path: ReadAuthorizationPath,
    authorizer: &dyn DocumentReadAuthorizer,
    change: &ChangeDocument,
) -> Option<Value> {
    let previous = change.previous_document();
    let current = change.document();
    let previous_visible = previous.is_some_and(|document| {
        !document.is_deleted() && is_readable(collection, path, authorizer, document)
    });
    let current_visible =
        !current.is_deleted() && is_readable(collection, path, authorizer, current);

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
