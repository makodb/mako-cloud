use std::{error::Error, fmt, num::NonZeroUsize};

use mako_documents::{
    ChangeLogError, CheckpointStatus, DocumentReadAuthorizer, ReadAuthorizationPath,
    RetentionError, SchemaVersion, ScopedCollectionEngine,
};

use crate::{
    AuthenticatedReplicationContext, PullRequest, PullResponse, ReplicationContractError,
    ReplicationTokenCodec, ReplicationTokenCodecError,
    schema::{SchemaMigrationRequired, require_compatible_schema},
    visibility::replication_change,
};

pub struct PullService<'a> {
    collection: &'a ScopedCollectionEngine,
    tokens: ReplicationTokenCodec<'a>,
    required_schema_version: SchemaVersion,
}

impl<'a> PullService<'a> {
    #[must_use]
    pub const fn new(
        collection: &'a ScopedCollectionEngine,
        tokens: ReplicationTokenCodec<'a>,
        required_schema_version: SchemaVersion,
    ) -> Self {
        Self {
            collection,
            tokens,
            required_schema_version,
        }
    }

    pub async fn initial_pull(
        &self,
        context: &AuthenticatedReplicationContext,
        request: &PullRequest,
        authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<PullResponse, PullError> {
        if request.checkpoint.is_some() {
            return Err(PullError::ExpectedNullCheckpoint);
        }
        self.pull_authorized(context, request, authorizer).await
    }

    pub async fn pull_authorized(
        &self,
        context: &AuthenticatedReplicationContext,
        request: &PullRequest,
        authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<PullResponse, PullError> {
        request.validate()?;
        require_compatible_schema(
            self.required_schema_version,
            request.schema_version,
            context,
        )?;
        if self.collection.scope().tenant() != context.tenant()
            || self.collection.scope().collection_id() != context.collection_id()
        {
            return Err(PullError::ScopeMismatch);
        }
        let mut scanned_through = match &request.checkpoint {
            Some(checkpoint) => {
                let position = self
                    .tokens
                    .decode_checkpoint(
                        checkpoint,
                        context,
                        request.schema_version,
                        request.filter.as_ref(),
                    )?
                    .scanned_position();
                if let CheckpointStatus::Expired { minimum_position } =
                    self.collection.checkpoint_status(position).await?
                {
                    return Err(PullError::CheckpointExpired { minimum_position });
                }
                position
            }
            None => 0,
        };
        let captured_high_water = self.collection.capture_committed_high_water().await?;
        if scanned_through > captured_high_water {
            return Err(PullError::CheckpointBeyondHighWater);
        }
        let mut documents = Vec::with_capacity(request.batch_size);
        while documents.len() < request.batch_size && scanned_through < captured_high_water {
            let page = self
                .collection
                .read_change_page(
                    scanned_through,
                    captured_high_water,
                    NonZeroUsize::new(request.batch_size)
                        .expect("validated pull batch size is positive"),
                )
                .await?;
            for change in page.changes() {
                scanned_through = change.change().commit_position();
                if let Some(document) = replication_change(
                    self.collection,
                    ReadAuthorizationPath::ReplicationPull,
                    authorizer,
                    change,
                    request.filter.as_ref(),
                ) {
                    documents.push(document);
                    if documents.len() == request.batch_size {
                        break;
                    }
                }
            }
            if page.is_exhausted() {
                scanned_through = captured_high_water;
                break;
            }
            if page.changes().is_empty() {
                return Err(PullError::ChangeScanDidNotAdvance);
            }
        }
        let checkpoint = self.tokens.encode_checkpoint(
            context,
            request.schema_version,
            scanned_through,
            None,
            request.filter.as_ref(),
        )?;
        Ok(PullResponse {
            documents,
            checkpoint,
        })
    }
}

#[derive(Debug)]
pub enum PullError {
    Contract(ReplicationContractError),
    ChangeLog(ChangeLogError),
    Token(ReplicationTokenCodecError),
    Retention(RetentionError),
    SchemaMigrationRequired(SchemaMigrationRequired),
    ExpectedNullCheckpoint,
    ScopeMismatch,
    ChangeScanDidNotAdvance,
    CheckpointExpired { minimum_position: u64 },
    CheckpointBeyondHighWater,
}

impl fmt::Display for PullError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => error.fmt(formatter),
            Self::ChangeLog(error) => error.fmt(formatter),
            Self::Token(error) => error.fmt(formatter),
            Self::Retention(error) => error.fmt(formatter),
            Self::SchemaMigrationRequired(error) => error.fmt(formatter),
            Self::ExpectedNullCheckpoint => {
                formatter.write_str("initial pull requires a null checkpoint")
            }
            Self::ScopeMismatch => formatter.write_str("replication collection scope mismatch"),
            Self::ChangeScanDidNotAdvance => {
                formatter.write_str("replication change scan did not advance")
            }
            Self::CheckpointExpired { minimum_position } => write!(
                formatter,
                "replication checkpoint expired before position {minimum_position}"
            ),
            Self::CheckpointBeyondHighWater => {
                formatter.write_str("replication checkpoint exceeds committed high water")
            }
        }
    }
}

impl Error for PullError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::ChangeLog(error) => Some(error),
            Self::Token(error) => Some(error),
            Self::Retention(error) => Some(error),
            Self::SchemaMigrationRequired(error) => Some(error),
            Self::ExpectedNullCheckpoint
            | Self::ScopeMismatch
            | Self::ChangeScanDidNotAdvance
            | Self::CheckpointExpired { .. }
            | Self::CheckpointBeyondHighWater => None,
        }
    }
}

impl From<ReplicationContractError> for PullError {
    fn from(error: ReplicationContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ChangeLogError> for PullError {
    fn from(error: ChangeLogError) -> Self {
        Self::ChangeLog(error)
    }
}

impl From<ReplicationTokenCodecError> for PullError {
    fn from(error: ReplicationTokenCodecError) -> Self {
        Self::Token(error)
    }
}

impl From<RetentionError> for PullError {
    fn from(error: RetentionError) -> Self {
        Self::Retention(error)
    }
}

impl From<SchemaMigrationRequired> for PullError {
    fn from(error: SchemaMigrationRequired) -> Self {
        Self::SchemaMigrationRequired(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use mako_api::{
        CollectionId, CollectionScope, EnvironmentId, ErrorCode, ProjectId, RetryAdvice,
        SafeDetail, TenantScope,
    };
    use mako_documents::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, MutationAuthorizationDecision, MutationCommitOutcome,
        MutationId, MutationInput, PrimaryKeyDefinition, ReadAuthorizationContext,
        SchemaCompatibility, SchemaVersion,
    };
    use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::{OpaqueCheckpoint, ReplicationFilter, ReplicationTokenKey};

    #[test]
    fn null_checkpoint_pull_captures_high_water_and_returns_commit_order() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant();
            let engine = DocumentEngine::new(adapter);
            let collection_scope = CollectionScope::new(
                tenant.clone(),
                CollectionId::parse("todos").expect("collection"),
            );
            let collection = engine
                .scope_collection(&tenant, collection_scope)
                .expect("scope");
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let validator = validator();
            let mut lease = sequencer
                .lease(NonZeroU64::new(4).expect("lease"))
                .await
                .expect("lease");
            for (id, value) in [("todo-a", 1), ("todo-b", 2), ("todo-c", 3), ("todo-d", 4)] {
                let position = lease.issue().expect("position");
                collection
                    .create_document(MutationInput {
                        mutation_id: MutationId::parse(format!("mutation-{id}")).expect("mutation"),
                        commit_position: CommitPosition::new(position).expect("position"),
                        document: validator
                            .validate_create(json!({"id": id, "value": value}))
                            .expect("document"),
                        durability: Durability::Memory,
                    })
                    .await
                    .expect("create");
            }
            assert_eq!(sequencer.recover_high_water().await.expect("high water"), 4);

            let context = context();
            let key = ReplicationTokenKey::from_bytes([9; 32]);
            let service = PullService::new(
                &collection,
                ReplicationTokenCodec::new(&key),
                SchemaVersion::new(1).expect("schema version"),
            );
            let allow = AllowAll;
            let mismatch = service
                .initial_pull(
                    &context,
                    &PullRequest {
                        checkpoint: None,
                        schema_version: 2,
                        batch_size: 10,
                        filter: None,
                    },
                    &allow,
                )
                .await
                .expect_err("outdated schema must be rejected");
            let PullError::SchemaMigrationRequired(mismatch) = mismatch else {
                panic!("expected schema migration error");
            };
            assert_eq!(mismatch.required_schema_version().get(), 1);
            let api_error = mismatch.api_error();
            assert_eq!(api_error.error.code, ErrorCode::SchemaMismatch);
            assert_eq!(api_error.error.retry, RetryAdvice::Never);
            assert_eq!(
                api_error.error.details.get("requiredSchemaVersion"),
                Some(&SafeDetail::String("1".to_owned()))
            );
            let response = service
                .initial_pull(
                    &context,
                    &PullRequest {
                        checkpoint: None,
                        schema_version: 1,
                        batch_size: 10,
                        filter: None,
                    },
                    &allow,
                )
                .await
                .expect("pull");
            assert_eq!(response.documents.len(), 4);
            assert_eq!(response.documents[0]["id"], "todo-a");
            assert_eq!(response.documents[1]["id"], "todo-b");
            assert_eq!(
                ReplicationTokenCodec::new(&key)
                    .decode_checkpoint(&response.checkpoint, &context, 1, None)
                    .expect("checkpoint")
                    .scanned_position(),
                4
            );

            let visible = service
                .pull_authorized(
                    &context,
                    &PullRequest {
                        checkpoint: None,
                        schema_version: 1,
                        batch_size: 2,
                        filter: None,
                    },
                    &EvenValues,
                )
                .await
                .expect("authorized pull");
            assert_eq!(
                visible
                    .documents
                    .iter()
                    .map(|document| document["id"].as_str().expect("id"))
                    .collect::<Vec<_>>(),
                vec!["todo-b", "todo-d"]
            );
            assert_eq!(
                ReplicationTokenCodec::new(&key)
                    .decode_checkpoint(&visible.checkpoint, &context, 1, None)
                    .expect("visible checkpoint")
                    .scanned_position(),
                4
            );

            let expired_checkpoint = ReplicationTokenCodec::new(&key)
                .encode_checkpoint(&context, 1, 1, None, None)
                .expect("old checkpoint");
            collection
                .compact_through(2, Durability::Memory)
                .await
                .expect("compact");
            assert!(matches!(
                service
                    .pull_authorized(
                        &context,
                        &PullRequest {
                            checkpoint: Some(expired_checkpoint),
                            schema_version: 1,
                            batch_size: 2,
                            filter: None,
                        },
                        &allow,
                    )
                    .await,
                Err(PullError::CheckpointExpired {
                    minimum_position: 2
                })
            ));
        });
    }

    #[test]
    fn pull_delivers_stored_and_visibility_transition_tombstones() {
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
            let validator = visibility_validator();
            let mut lease = sequencer
                .lease(NonZeroU64::new(4).expect("lease"))
                .await
                .expect("lease");
            let create_position = lease.issue().expect("create position");
            let MutationCommitOutcome::Applied(created) = collection
                .create_document(visibility_input(
                    &validator,
                    create_position,
                    "create-visible",
                    json!({"id": "todo-move", "team": "blue", "secret": "known-old"}),
                ))
                .await
                .expect("create visible")
            else {
                panic!("create must apply");
            };
            sequencer.recover_high_water().await.expect("high water");

            let context = context();
            let key = ReplicationTokenKey::from_bytes([7; 32]);
            let service = PullService::new(
                &collection,
                ReplicationTokenCodec::new(&key),
                SchemaVersion::new(1).expect("schema version"),
            );
            let blue = BlueTeam;
            let initial = service
                .initial_pull(
                    &context,
                    &PullRequest {
                        checkpoint: None,
                        schema_version: 1,
                        batch_size: 10,
                        filter: None,
                    },
                    &blue,
                )
                .await
                .expect("initial pull");
            assert_eq!(initial.documents[0]["_deleted"], false);

            let update_position = lease.issue().expect("update position");
            let MutationCommitOutcome::Applied(_) = collection
                .update_document(
                    created.revision,
                    visibility_input(
                        &validator,
                        update_position,
                        "move-protected",
                        json!({
                            "id": "todo-move",
                            "team": "red",
                            "secret": "protected-new"
                        }),
                    ),
                )
                .await
                .expect("move protected")
            else {
                panic!("update must apply");
            };
            sequencer.recover_high_water().await.expect("high water");
            let transition = service
                .pull_authorized(
                    &context,
                    &PullRequest {
                        checkpoint: Some(initial.checkpoint),
                        schema_version: 1,
                        batch_size: 10,
                        filter: None,
                    },
                    &blue,
                )
                .await
                .expect("transition pull");
            assert_eq!(transition.documents.len(), 1);
            assert_eq!(transition.documents[0]["id"], "todo-move");
            assert_eq!(transition.documents[0]["_deleted"], true);
            assert_eq!(transition.documents[0]["secret"], "known-old");
            assert_ne!(transition.documents[0]["secret"], "protected-new");

            let stored_position = lease.issue().expect("stored position");
            let MutationCommitOutcome::Applied(stored) = collection
                .create_document(visibility_input(
                    &validator,
                    stored_position,
                    "create-for-delete",
                    json!({"id": "todo-delete", "team": "blue", "secret": "known-delete"}),
                ))
                .await
                .expect("create for delete")
            else {
                panic!("create must apply");
            };
            sequencer.recover_high_water().await.expect("high water");
            let before_delete = service
                .pull_authorized(
                    &context,
                    &PullRequest {
                        checkpoint: Some(transition.checkpoint),
                        schema_version: 1,
                        batch_size: 10,
                        filter: None,
                    },
                    &blue,
                )
                .await
                .expect("before delete");
            assert_eq!(before_delete.documents[0]["id"], "todo-delete");

            let delete_position = lease.issue().expect("delete position");
            let MutationCommitOutcome::Applied(deleted) = collection
                .delete_document(
                    stored.revision,
                    visibility_input(
                        &validator,
                        delete_position,
                        "delete-visible",
                        json!({"id": "todo-delete", "team": "blue", "secret": "known-delete"}),
                    ),
                )
                .await
                .expect("delete visible")
            else {
                panic!("delete must apply");
            };
            sequencer.recover_high_water().await.expect("high water");
            let deletion = service
                .pull_authorized(
                    &context,
                    &PullRequest {
                        checkpoint: Some(before_delete.checkpoint),
                        schema_version: 1,
                        batch_size: 10,
                        filter: None,
                    },
                    &blue,
                )
                .await
                .expect("deletion pull");
            assert_eq!(deletion.documents.len(), 1);
            assert_eq!(deletion.documents[0]["id"], "todo-delete");
            assert_eq!(deletion.documents[0]["_deleted"], true);
            assert_eq!(deletion.documents[0]["_rev"], deleted.revision.as_str());
        });
    }

    struct AllowAll;

    impl DocumentReadAuthorizer for AllowAll {
        fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            MutationAuthorizationDecision::allow("test_allow")
        }
    }

    struct EvenValues;

    impl DocumentReadAuthorizer for EvenValues {
        fn authorize_read(
            &self,
            context: ReadAuthorizationContext<'_>,
        ) -> MutationAuthorizationDecision {
            if context.document().body()["value"]
                .as_u64()
                .is_some_and(|value| value % 2 == 0)
            {
                MutationAuthorizationDecision::allow("test_allow")
            } else {
                MutationAuthorizationDecision::deny("test_deny")
            }
        }
    }

    struct BlueTeam;

    impl DocumentReadAuthorizer for BlueTeam {
        fn authorize_read(
            &self,
            context: ReadAuthorizationContext<'_>,
        ) -> MutationAuthorizationDecision {
            if context.document().body()["team"] == "blue" {
                MutationAuthorizationDecision::allow("test_blue_team")
            } else {
                MutationAuthorizationDecision::deny("test_other_team")
            }
        }
    }

    fn visibility_input(
        validator: &DocumentValidator,
        position: u64,
        mutation_id: &str,
        document: serde_json::Value,
    ) -> MutationInput {
        MutationInput {
            mutation_id: MutationId::parse(mutation_id).expect("mutation"),
            commit_position: CommitPosition::new(position).expect("position"),
            document: validator.validate_create(document).expect("document"),
            durability: Durability::Memory,
        }
    }

    fn visibility_validator() -> DocumentValidator {
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "required": ["id", "team", "secret"],
                "properties": {
                    "id": {"type": "string"},
                    "team": {"type": "string"},
                    "secret": {"type": "string"}
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

    /// A validator whose documents carry the field a filter narrows on.
    fn household_validator() -> DocumentValidator {
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "required": ["id", "household_id"],
                "properties": {
                    "id": {"type": "string"},
                    "household_id": {"type": "string"}
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

    /// A user in two households reads the documents of both, so without a
    /// filter a per-household local database receives the other's and
    /// discards them -- paying for the transfer and re-examining every one of
    /// them on every pull. The filter narrows the scope after the policy, and
    /// narrows it the same way: a document that leaves the filter comes back
    /// as a tombstone, because otherwise the database it left would keep it
    /// for ever.
    #[test]
    fn a_filter_narrows_a_scope_tombstones_what_leaves_it_and_binds_the_checkpoint() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let tenant = tenant();
            let engine = DocumentEngine::new(adapter);
            let collection_scope = CollectionScope::new(
                tenant.clone(),
                CollectionId::parse("todos").expect("collection"),
            );
            let collection = engine
                .scope_collection(&tenant, collection_scope)
                .expect("scope");
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let validator = household_validator();
            let mut lease = sequencer
                .lease(NonZeroU64::new(8).expect("lease"))
                .await
                .expect("lease");
            let mut moved_revision = None;
            for (id, household) in [
                ("one-a", "hh_one"),
                ("two-a", "hh_two"),
                ("one-b", "hh_one"),
                ("two-b", "hh_two"),
            ] {
                let position = lease.issue().expect("position");
                let outcome = collection
                    .create_document(MutationInput {
                        mutation_id: MutationId::parse(format!("create-{id}")).expect("mutation"),
                        commit_position: CommitPosition::new(position).expect("position"),
                        document: validator
                            .validate_create(json!({"id": id, "household_id": household}))
                            .expect("document"),
                        durability: Durability::Memory,
                    })
                    .await
                    .expect("create");
                let MutationCommitOutcome::Applied(applied) = outcome else {
                    panic!("create must apply");
                };
                if id == "one-a" {
                    moved_revision = Some(applied.revision);
                }
            }
            sequencer.recover_high_water().await.expect("high water");

            let context = context();
            let key = ReplicationTokenKey::from_bytes([9; 32]);
            let service = PullService::new(
                &collection,
                ReplicationTokenCodec::new(&key),
                SchemaVersion::new(1).expect("schema version"),
            );
            let allow = AllowAll;
            let filter = ReplicationFilter {
                field: "household_id".to_owned(),
                value: "hh_one".to_owned(),
            };
            let request = |checkpoint: Option<OpaqueCheckpoint>| PullRequest {
                checkpoint,
                schema_version: 1,
                batch_size: 10,
                filter: Some(filter.clone()),
            };

            // Only one household's documents, and the checkpoint still stands
            // past everything scanned -- a page of the other household's
            // changes must not stall the scan.
            let response = service
                .initial_pull(&context, &request(None), &allow)
                .await
                .expect("pull");
            let ids: Vec<&str> = response
                .documents
                .iter()
                .map(|document| document["id"].as_str().expect("id"))
                .collect();
            assert_eq!(ids, ["one-a", "one-b"]);
            assert_eq!(
                ReplicationTokenCodec::new(&key)
                    .decode_checkpoint(&response.checkpoint, &context, 1, Some(&filter))
                    .expect("checkpoint")
                    .scanned_position(),
                4,
                "the scan advances past the changes the filter passed over"
            );

            // A checkpoint from this scope is not a checkpoint for another:
            // resuming it unfiltered would skip everything the filter passed
            // over, and it is refused rather than silently doing so.
            let unfiltered = PullRequest {
                checkpoint: Some(response.checkpoint.clone()),
                schema_version: 1,
                batch_size: 10,
                filter: None,
            };
            assert!(matches!(
                service.pull_authorized(&context, &unfiltered, &allow).await,
                Err(PullError::Token(_))
            ));

            // The document moves to the other household. The scope it left
            // is told, as a tombstone; the scope it joined receives it.
            let position = lease.issue().expect("position");
            collection
                .update_document(
                    moved_revision.expect("the moved document was created"),
                    MutationInput {
                        mutation_id: MutationId::parse("move-one-a").expect("mutation"),
                        commit_position: CommitPosition::new(position).expect("position"),
                        document: validator
                            .validate_create(json!({"id": "one-a", "household_id": "hh_two"}))
                            .expect("document"),
                        durability: Durability::Memory,
                    },
                )
                .await
                .expect("update");
            sequencer.recover_high_water().await.expect("high water");

            let after = service
                .pull_authorized(&context, &request(Some(response.checkpoint)), &allow)
                .await
                .expect("pull");
            assert_eq!(after.documents.len(), 1);
            assert_eq!(after.documents[0]["id"], "one-a");
            assert_eq!(
                after.documents[0]["_deleted"], true,
                "a document that leaves the filter is a tombstone in the scope it left"
            );
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
