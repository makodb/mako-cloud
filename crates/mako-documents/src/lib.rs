//! Versioned JSON document, schema, index, and change-log engine.

#![forbid(unsafe_code)]

mod authorization;
mod change_log;
mod collection;
mod document;
mod engine;
mod index;
mod index_build;
mod mutation;
mod query;
mod read;
mod retention;
mod sequencer;
mod stored_check;
mod validation;

pub use authorization::{
    DocumentMutationAuthorizer, DocumentReadAuthorizer, MutationAuthorizationContext,
    MutationAuthorizationDecision, MutationAuthorizationOperation, ReadAuthorizationContext,
    ReadAuthorizationPath,
};
pub use change_log::{ChangeDocument, ChangeLogError, ChangePage};
pub use collection::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataError,
    CollectionMetadataInstallError, CollectionMetadataInstallOutcome, CollectionMetadataLoadError,
    CollectionMetadataVersion, PrimaryKeyDefinition, SchemaCompatibility,
};
pub use document::{
    CanonicalDocument, CommitPosition, DocumentCodecError, DocumentId, RevisionToken, SchemaVersion,
};
pub use engine::{DocumentEngine, DocumentEngineScopeError, ScopedCollectionEngine};
pub use index::{
    IndexDefinition, IndexDirection, IndexError, IndexFailure, IndexFailureCode, IndexField,
    IndexKind, IndexName, IndexNumber, IndexState, IndexValue, IndexVersion,
    MAX_INDEXED_VALUE_BYTES,
};
pub use index_build::{
    IndexActivationOutcome, IndexBuildError, IndexBuildProgress, IndexRemovalReport,
};
pub use mutation::{
    ChangeRecord, ExpectedRevision, MutationCommitOutcome, MutationError, MutationId,
    MutationInput, MutationReceipt, MutationRequest,
};
pub use query::{
    IndexedQuery, MAX_TRUSTED_QUERY_LIMIT, QueryBound, QueryCursor, QueryPage, QueryPlan,
    QueryPredicate, QuerySort, RequiredIndexShape, TrustedQuery, TrustedQueryError,
};
pub use read::{
    DocumentReadError, PrimaryKeyBrowseOptions, PrimaryKeyBrowsePage, ScopedCollectionSnapshot,
};
pub use retention::{CheckpointStatus, CompactionMode, CompactionReport, RetentionError};
pub use sequencer::{
    EnvironmentSequencer, FinalizeOutcome, SequenceDisposition, SequenceLease, SequencerError,
};
pub use stored_check::{MAX_REPORTED_FAILURES, StoredDocumentCheck, StoredDocumentCheckError};
pub use validation::{
    DocumentValidationError, DocumentValidator, SchemaValidationIssue, ValidatedDocumentBody,
};

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "documents";
