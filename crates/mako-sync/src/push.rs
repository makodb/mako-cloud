use std::{error::Error, fmt, num::NonZeroU64};

use mako_api::{ApiError, ApiErrorEnvelope, ErrorCode, RetryAdvice};
use mako_documents::{
    CanonicalDocument, CommitPosition, DocumentMutationAuthorizer, DocumentReadAuthorizer,
    DocumentValidator, EnvironmentSequencer, ExpectedRevision, MutationCommitOutcome,
    MutationError, MutationId, MutationRequest, ReadAuthorizationContext, ReadAuthorizationPath,
    RevisionToken, ScopedCollectionEngine, SequencerError,
};
use mako_storage::Durability;
use serde_json::Value;

use crate::{
    AuthenticatedReplicationContext, PushOutcome, PushOutcomeStatus, PushRequest, PushResponse,
    PushRow, ReplicationContractError,
    schema::{SchemaMigrationRequired, require_compatible_schema},
    visibility::replication_document,
};

pub struct PushService<'a> {
    collection: &'a ScopedCollectionEngine,
    sequencer: &'a EnvironmentSequencer,
    validator: &'a DocumentValidator,
    durability: Durability,
}

impl<'a> PushService<'a> {
    #[must_use]
    pub const fn new(
        collection: &'a ScopedCollectionEngine,
        sequencer: &'a EnvironmentSequencer,
        validator: &'a DocumentValidator,
        durability: Durability,
    ) -> Self {
        Self {
            collection,
            sequencer,
            validator,
            durability,
        }
    }

    pub async fn push(
        &self,
        context: &AuthenticatedReplicationContext,
        request: &PushRequest,
        mutation_authorizer: &dyn DocumentMutationAuthorizer,
        read_authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<PushResponse, PushError> {
        request.validate()?;
        require_compatible_schema(
            self.validator.schema_version(),
            request.schema_version,
            context,
        )?;
        if self.collection.scope().tenant() != context.tenant()
            || self.collection.scope().collection_id() != context.collection_id()
        {
            return Err(PushError::ScopeMismatch);
        }
        let mut lease = self
            .sequencer
            .lease(NonZeroU64::new(request.rows.len() as u64).expect("validated non-empty push"))
            .await?;
        let mut outcomes = Vec::with_capacity(request.rows.len());
        for row in &request.rows {
            let position = lease.issue().expect("lease covers every validated row");
            match self
                .push_row(
                    context,
                    request.schema_version,
                    row,
                    position,
                    mutation_authorizer,
                    read_authorizer,
                )
                .await
            {
                Ok(RowResult::Committed(outcome)) => outcomes.push(outcome),
                Ok(RowResult::NotCommitted(outcome)) => {
                    self.sequencer.mark_aborted(position).await?;
                    outcomes.push(outcome);
                }
                Err(error) => {
                    self.sequencer.mark_aborted(position).await?;
                    self.sequencer.abort_remaining(&mut lease).await?;
                    self.sequencer.recover_high_water().await?;
                    return Err(error);
                }
            }
        }
        self.sequencer.abort_remaining(&mut lease).await?;
        self.sequencer.recover_high_water().await?;
        Ok(PushResponse { outcomes })
    }

    async fn push_row(
        &self,
        context: &AuthenticatedReplicationContext,
        schema_version: u64,
        row: &PushRow,
        position: u64,
        mutation_authorizer: &dyn DocumentMutationAuthorizer,
        read_authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<RowResult, PushError> {
        let (body, deleted) = match replication_body(&row.new_document_state) {
            Ok(value) => value,
            Err(()) => return Ok(RowResult::NotCommitted(denied_invalid(context, row))),
        };
        let candidate = match self.validator.validate_create(Value::Object(body.clone())) {
            Ok(candidate) => candidate,
            Err(_) => return Ok(RowResult::NotCommitted(denied_invalid(context, row))),
        };
        if candidate.schema_version().get() != schema_version {
            return Ok(RowResult::NotCommitted(denied_invalid(context, row)));
        }
        let current = self
            .collection
            .get_document(candidate.primary_key())
            .await
            .map_err(PushError::Read)?;
        let document = match current.as_ref() {
            Some(current) => match self.validator.validate_update(current, Value::Object(body)) {
                Ok(document) => document,
                Err(_) => return Ok(RowResult::NotCommitted(denied_invalid(context, row))),
            },
            None => candidate,
        };
        let expected_revision = match &row.assumed_master_state {
            None => ExpectedRevision::Missing,
            Some(assumed) => match assumed_revision(assumed) {
                Some(revision) => ExpectedRevision::Exact(revision),
                // RxDB names no revision for a document it has already pushed:
                // the master state it assumes is the state it pushed, and the
                // service's revision token was never part of that. Hold the
                // assumed state against the document itself instead -- the
                // comparison RxDB's own conflict handling makes -- so the
                // second edit of a document is not refused as malformed.
                None => match assumed_state_matches(assumed, current.as_ref()) {
                    None => return Ok(RowResult::NotCommitted(denied_invalid(context, row))),
                    Some(false) => {
                        return Ok(RowResult::NotCommitted(self.conflict_outcome(
                            context,
                            row,
                            current.as_ref(),
                            read_authorizer,
                        )));
                    }
                    Some(true) => current
                        .as_ref()
                        .map_or(ExpectedRevision::Missing, |current| {
                            ExpectedRevision::Exact(current.revision().clone())
                        }),
                },
            },
        };
        let mutation_id = MutationId::parse(row.mutation_id.clone())?;
        let document_id = document.primary_key().clone();
        let request = MutationRequest {
            revision: mutation_id.revision_for(document.primary_key()),
            mutation_id,
            expected_revision,
            commit_position: CommitPosition::new(position)?,
            deleted,
            document,
            durability: self.durability,
        };
        match self
            .collection
            .commit_mutation_authorized(request, mutation_authorizer)
            .await
        {
            Ok(MutationCommitOutcome::Applied(_)) => {
                Ok(RowResult::Committed(accepted(&row.mutation_id)))
            }
            Ok(MutationCommitOutcome::Replayed(_)) => {
                Ok(RowResult::NotCommitted(accepted(&row.mutation_id)))
            }
            Ok(MutationCommitOutcome::RevisionConflict { .. }) => {
                let current = self
                    .collection
                    .get_document(&document_id)
                    .await
                    .map_err(PushError::Read)?;
                Ok(RowResult::NotCommitted(self.conflict_outcome(
                    context,
                    row,
                    current.as_ref(),
                    read_authorizer,
                )))
            }
            Err(MutationError::AuthorizationDenied { .. }) => {
                Ok(RowResult::NotCommitted(denied_policy(context, row)))
            }
            Err(MutationError::IdempotencyMismatch) => {
                Ok(RowResult::NotCommitted(denied_idempotency(context, row)))
            }
            Err(error) => Err(PushError::Mutation(error)),
        }
    }

    /// The master state a conflicting push is told about: the document as this
    /// caller may read it, nothing when reading it is denied, and an explicit
    /// absence when there is no document.
    fn conflict_outcome(
        &self,
        context: &AuthenticatedReplicationContext,
        row: &PushRow,
        current: Option<&CanonicalDocument>,
        read_authorizer: &dyn DocumentReadAuthorizer,
    ) -> PushOutcome {
        match current {
            Some(current)
                if read_authorizer
                    .authorize_read(ReadAuthorizationContext::new(
                        self.collection,
                        ReadAuthorizationPath::ConflictResponse,
                        current,
                    ))
                    .is_allowed() =>
            {
                conflict(&row.mutation_id, Some(replication_document(current)))
            }
            Some(_) => denied_policy(context, row),
            None => conflict(&row.mutation_id, None),
        }
    }
}

/// Whether an assumed master state that names no revision describes the
/// document as it is. `None` when the state is not a document at all.
fn assumed_state_matches(assumed: &Value, current: Option<&CanonicalDocument>) -> Option<bool> {
    let (mut body, deleted) = replication_body(assumed).ok()?;
    // RxDB's own bookkeeping never reaches the service's documents.
    body.remove("_meta");
    body.remove("_attachments");
    Some(match current {
        None => deleted,
        Some(current) => current.is_deleted() == deleted && current.body() == &body,
    })
}

enum RowResult {
    Committed(PushOutcome),
    NotCommitted(PushOutcome),
}

fn replication_body(value: &Value) -> Result<(serde_json::Map<String, Value>, bool), ()> {
    let mut body = value.as_object().cloned().ok_or(())?;
    let deleted = match body.remove("_deleted") {
        Some(Value::Bool(deleted)) => deleted,
        Some(_) => return Err(()),
        None => false,
    };
    body.remove("_rev");
    Ok((body, deleted))
}

fn assumed_revision(value: &Value) -> Option<RevisionToken> {
    value
        .as_object()?
        .get("_rev")?
        .as_str()
        .and_then(|revision| RevisionToken::parse(revision).ok())
}

fn accepted(mutation_id: &str) -> PushOutcome {
    PushOutcome {
        mutation_id: mutation_id.to_owned(),
        status: PushOutcomeStatus::Accepted,
        master_state: None,
        error: None,
    }
}

fn conflict(mutation_id: &str, master_state: Option<Value>) -> PushOutcome {
    PushOutcome {
        mutation_id: mutation_id.to_owned(),
        status: PushOutcomeStatus::Conflict,
        master_state,
        error: None,
    }
}

fn denied_invalid(context: &AuthenticatedReplicationContext, row: &PushRow) -> PushOutcome {
    denied(
        context,
        row,
        ErrorCode::InvalidRequest,
        "pushed document state is invalid",
    )
}

fn denied_policy(context: &AuthenticatedReplicationContext, row: &PushRow) -> PushOutcome {
    denied(
        context,
        row,
        ErrorCode::PermissionDenied,
        "document mutation is not permitted",
    )
}

fn denied_idempotency(context: &AuthenticatedReplicationContext, row: &PushRow) -> PushOutcome {
    denied(
        context,
        row,
        ErrorCode::Conflict,
        "mutation identifier was reused with different content",
    )
}

fn denied(
    context: &AuthenticatedReplicationContext,
    row: &PushRow,
    code: ErrorCode,
    message: &'static str,
) -> PushOutcome {
    PushOutcome {
        mutation_id: row.mutation_id.clone(),
        status: PushOutcomeStatus::Denied,
        master_state: None,
        error: Some(ApiErrorEnvelope::new(ApiError::new(
            code,
            message,
            context.request_id(),
            RetryAdvice::Never,
        ))),
    }
}

#[derive(Debug)]
pub enum PushError {
    Contract(ReplicationContractError),
    SchemaMigrationRequired(SchemaMigrationRequired),
    ScopeMismatch,
    Document(mako_documents::DocumentCodecError),
    Mutation(MutationError),
    Read(mako_documents::DocumentReadError),
    Sequencer(SequencerError),
}

impl fmt::Display for PushError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => error.fmt(formatter),
            Self::SchemaMigrationRequired(error) => error.fmt(formatter),
            Self::ScopeMismatch => formatter.write_str("replication collection scope mismatch"),
            Self::Document(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Sequencer(error) => error.fmt(formatter),
        }
    }
}

impl Error for PushError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::SchemaMigrationRequired(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::Mutation(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Sequencer(error) => Some(error),
            Self::ScopeMismatch => None,
        }
    }
}

impl From<ReplicationContractError> for PushError {
    fn from(error: ReplicationContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<SchemaMigrationRequired> for PushError {
    fn from(error: SchemaMigrationRequired) -> Self {
        Self::SchemaMigrationRequired(error)
    }
}

impl From<mako_documents::DocumentCodecError> for PushError {
    fn from(error: mako_documents::DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

impl From<MutationError> for PushError {
    fn from(error: MutationError) -> Self {
        Self::Mutation(error)
    }
}

impl From<SequencerError> for PushError {
    fn from(error: SequencerError) -> Self {
        Self::Sequencer(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_documents::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, DocumentEngine,
        DocumentId, MutationAuthorizationContext, MutationAuthorizationDecision,
        PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
    };
    use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
    use mako_storage::MemoryAdapter;
    use serde_json::json;

    use super::*;

    struct AllowAll;

    impl DocumentMutationAuthorizer for AllowAll {
        fn authorize(&self, _: MutationAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            MutationAuthorizationDecision::allow("test_allow")
        }
    }

    impl DocumentReadAuthorizer for AllowAll {
        fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            MutationAuthorizationDecision::allow("test_allow")
        }
    }

    struct DenyAll;

    impl DocumentMutationAuthorizer for DenyAll {
        fn authorize(&self, _: MutationAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            MutationAuthorizationDecision::deny("test_deny")
        }
    }

    impl DocumentReadAuthorizer for DenyAll {
        fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            MutationAuthorizationDecision::deny("test_deny")
        }
    }

    #[test]
    fn bounded_push_commits_rows_and_returns_only_readable_conflicts() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let engine = DocumentEngine::new(Arc::new(MemoryAdapter::new()));
            let collection = engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("collection");
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let validator = validator();
            let service = PushService::new(&collection, &sequencer, &validator, Durability::Memory);
            let context = context();
            let create = PushRequest {
                schema_version: 1,
                rows: vec![
                    PushRow {
                        mutation_id: "mutation-create-a".to_owned(),
                        assumed_master_state: None,
                        new_document_state: json!({"id": "todo-a", "value": 1}),
                    },
                    PushRow {
                        mutation_id: "mutation-create-b".to_owned(),
                        assumed_master_state: None,
                        new_document_state: json!({"id": "todo-b", "value": 2}),
                    },
                ],
            };
            let mut incompatible = create.clone();
            incompatible.schema_version = 2;
            assert!(matches!(
                service
                    .push(&context, &incompatible, &AllowAll, &AllowAll)
                    .await,
                Err(PushError::SchemaMigrationRequired(_))
            ));
            assert_eq!(
                sequencer.committed_high_water().await.expect("high water"),
                0
            );
            let response = service
                .push(&context, &create, &AllowAll, &AllowAll)
                .await
                .expect("push");
            assert!(
                response
                    .outcomes
                    .iter()
                    .all(|outcome| outcome.status == PushOutcomeStatus::Accepted)
            );
            assert_eq!(
                sequencer.committed_high_water().await.expect("high water"),
                2
            );
            let replay = service
                .push(&context, &create, &AllowAll, &AllowAll)
                .await
                .expect("retry");
            assert!(
                replay
                    .outcomes
                    .iter()
                    .all(|outcome| outcome.status == PushOutcomeStatus::Accepted)
            );
            assert_eq!(
                sequencer
                    .committed_high_water()
                    .await
                    .expect("retry high water"),
                4
            );
            let current = collection
                .get_document(&DocumentId::parse("todo-a").expect("document"))
                .await
                .expect("read")
                .expect("document");
            assert_eq!(current.commit_position().get(), 1);
            let stale = PushRequest {
                schema_version: 1,
                rows: vec![PushRow {
                    mutation_id: "mutation-stale-aa".to_owned(),
                    assumed_master_state: Some(json!({
                        "id": "todo-a",
                        "value": 0,
                        "_rev": "stale-revision"
                    })),
                    new_document_state: json!({"id": "todo-a", "value": 3}),
                }],
            };
            let readable = service
                .push(&context, &stale, &AllowAll, &AllowAll)
                .await
                .expect("readable conflict");
            assert_eq!(readable.outcomes[0].status, PushOutcomeStatus::Conflict);
            assert_eq!(
                readable.outcomes[0].master_state.as_ref().expect("master")["value"],
                1
            );
            let hidden = PushRequest {
                schema_version: 1,
                rows: vec![PushRow {
                    mutation_id: "mutation-stale-bb".to_owned(),
                    assumed_master_state: Some(json!({
                        "id": "todo-a",
                        "_rev": "stale-revision"
                    })),
                    new_document_state: json!({"id": "todo-a", "value": 4}),
                }],
            };
            let hidden = service
                .push(&context, &hidden, &AllowAll, &DenyAll)
                .await
                .expect("hidden conflict");
            assert_eq!(hidden.outcomes[0].status, PushOutcomeStatus::Denied);
            assert!(hidden.outcomes[0].master_state.is_none());
            let hidden_wire = serde_json::to_string(&hidden).expect("serialize hidden response");
            assert!(!hidden_wire.contains("todo-a"));
            assert!(!hidden_wire.contains("stale-revision"));

            let denied = PushRequest {
                schema_version: 1,
                rows: vec![PushRow {
                    mutation_id: "mutation-denied-aa".to_owned(),
                    assumed_master_state: Some(replication_document(&current)),
                    new_document_state: json!({"id": "todo-a", "value": 5}),
                }],
            };
            let denied = service
                .push(&context, &denied, &DenyAll, &AllowAll)
                .await
                .expect("policy denial");
            assert_eq!(denied.outcomes[0].status, PushOutcomeStatus::Denied);
            assert!(denied.outcomes[0].master_state.is_none());
            assert_eq!(hidden.outcomes[0].error, denied.outcomes[0].error);
        });
    }

    #[test]
    fn a_push_naming_no_revision_is_held_against_the_document_itself() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let engine = DocumentEngine::new(Arc::new(MemoryAdapter::new()));
            let collection = engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("collection");
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let validator = validator();
            let service = PushService::new(&collection, &sequencer, &validator, Durability::Memory);
            let context = context();
            let push = |mutation: &str, assumed: Option<Value>, next: Value| PushRequest {
                schema_version: 1,
                rows: vec![PushRow {
                    mutation_id: mutation.to_owned(),
                    assumed_master_state: assumed,
                    new_document_state: next,
                }],
            };

            let created = service
                .push(
                    &context,
                    &push(
                        "mutation-create-a-1",
                        None,
                        json!({"id": "todo-a", "value": 1}),
                    ),
                    &AllowAll,
                    &AllowAll,
                )
                .await
                .expect("create");
            assert_eq!(created.outcomes[0].status, PushOutcomeStatus::Accepted);

            // RxDB assumes the master is what it pushed -- no `_rev` in sight.
            let second = service
                .push(
                    &context,
                    &push(
                        "mutation-second-edit-a",
                        Some(json!({"id": "todo-a", "value": 1, "_deleted": false})),
                        json!({"id": "todo-a", "value": 2}),
                    ),
                    &AllowAll,
                    &AllowAll,
                )
                .await
                .expect("second push");
            assert_eq!(second.outcomes[0].status, PushOutcomeStatus::Accepted);

            // An assumed state that no longer describes the document is a
            // conflict, answered with the document as it is.
            let stale = service
                .push(
                    &context,
                    &push(
                        "mutation-stale-edit-a",
                        Some(json!({"id": "todo-a", "value": 1, "_deleted": false})),
                        json!({"id": "todo-a", "value": 3}),
                    ),
                    &AllowAll,
                    &AllowAll,
                )
                .await
                .expect("stale push");
            assert_eq!(stale.outcomes[0].status, PushOutcomeStatus::Conflict);
            assert_eq!(
                stale.outcomes[0].master_state.as_ref().expect("master")["value"],
                2
            );

            // A state that is not a document at all is still refused.
            let malformed = service
                .push(
                    &context,
                    &push(
                        "mutation-malformed-a",
                        Some(json!({"id": "todo-a", "_deleted": "no"})),
                        json!({"id": "todo-a", "value": 4}),
                    ),
                    &AllowAll,
                    &AllowAll,
                )
                .await
                .expect("malformed push");
            assert_eq!(malformed.outcomes[0].status, PushOutcomeStatus::Denied);
        });
    }

    fn validator() -> DocumentValidator {
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "required": ["id", "value"],
                "properties": {
                    "id": {"type": "string"},
                    "value": {"type": "integer"}
                },
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        DocumentValidator::compile(&metadata).expect("validator")
    }

    fn context() -> AuthenticatedReplicationContext {
        AuthenticatedReplicationContext::new(
            tenant(),
            CollectionId::parse("todos").expect("collection"),
            AppUserId::parse("usr_abcdefgh").expect("user"),
            SessionId::parse("ses_abcdefgh").expect("session"),
            "member",
            AccessAuthorizationEpochs {
                environment: 1,
                user: 1,
            },
            "req_abcdefgh",
        )
        .expect("context")
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
