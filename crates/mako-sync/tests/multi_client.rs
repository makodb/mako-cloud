use std::{num::NonZeroU64, num::NonZeroUsize, sync::Arc};

use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
use mako_documents::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
    DocumentEngine, DocumentMutationAuthorizer, DocumentReadAuthorizer, DocumentValidator,
    EnvironmentSequencer, MutationAuthorizationContext, MutationAuthorizationDecision,
    MutationCommitOutcome, MutationId, MutationInput, MutationReceipt, PrimaryKeyDefinition,
    ReadAuthorizationContext, RevisionToken, SchemaCompatibility, SchemaVersion,
    ScopedCollectionEngine,
};
use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
use mako_storage::{Durability, MemoryAdapter, RocksDbAdapter, RocksDbConfig};
use mako_sync::{
    AuthenticatedReplicationContext, LiveStreamEvent, LiveStreamLimits, LiveStreamRequest,
    LiveStreamSession, PullError, PullRequest, PullService, PushOutcomeStatus, PushRequest,
    PushRow, PushService, ReplicationTokenCodec, ReplicationTokenKey, ResyncReason,
};
use proptest::prelude::*;
use serde_json::{Value, json};

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

struct Team(&'static str);

impl DocumentReadAuthorizer for Team {
    fn authorize_read(
        &self,
        context: ReadAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision {
        if context.document().body()["team"] == self.0 {
            MutationAuthorizationDecision::allow("test_team_allow")
        } else {
            MutationAuthorizationDecision::deny("test_team_deny")
        }
    }
}

#[test]
fn concurrent_offline_clients_conflict_and_winner_retry_is_idempotent() {
    futures::executor::block_on(async {
        let fixture = Fixture::new();
        let service = PushService::new(
            &fixture.collection,
            &fixture.sequencer,
            &fixture.validator,
            Durability::Memory,
        );
        let allow = AllowAll;
        let create = PushRequest {
            schema_version: 1,
            rows: vec![PushRow {
                mutation_id: "client-create-todo".to_owned(),
                assumed_master_state: None,
                new_document_state: json!({"id": "todo-race", "value": 0, "team": "blue"}),
            }],
        };
        assert_eq!(
            service
                .push(&fixture.blue_context, &create, &allow, &allow)
                .await
                .expect("create")
                .outcomes[0]
                .status,
            PushOutcomeStatus::Accepted
        );
        let base = fixture
            .collection
            .get_document(&mako_documents::DocumentId::parse("todo-race").expect("id"))
            .await
            .expect("read")
            .expect("base");
        let assumed = replication_state(&base);
        let client_a = PushRequest {
            schema_version: 1,
            rows: vec![PushRow {
                mutation_id: "client-a-offline-write".to_owned(),
                assumed_master_state: Some(assumed.clone()),
                new_document_state: json!({"id": "todo-race", "value": 1, "team": "blue"}),
            }],
        };
        let client_b = PushRequest {
            schema_version: 1,
            rows: vec![PushRow {
                mutation_id: "client-b-offline-write".to_owned(),
                assumed_master_state: Some(assumed),
                new_document_state: json!({"id": "todo-race", "value": 2, "team": "blue"}),
            }],
        };
        let (a, b) = futures::join!(
            service.push(&fixture.blue_context, &client_a, &allow, &allow),
            service.push(&fixture.red_context, &client_b, &allow, &allow),
        );
        let a = a.expect("client a");
        let b = b.expect("client b");
        let (winner, winner_response, loser_response) =
            if a.outcomes[0].status == PushOutcomeStatus::Accepted {
                (&client_a, &a, &b)
            } else {
                (&client_b, &b, &a)
            };
        assert_eq!(
            winner_response.outcomes[0].status,
            PushOutcomeStatus::Accepted
        );
        assert_eq!(
            loser_response.outcomes[0].status,
            PushOutcomeStatus::Conflict
        );
        assert!(loser_response.outcomes[0].master_state.is_some());

        assert_eq!(
            service
                .push(&fixture.blue_context, winner, &allow, &allow)
                .await
                .expect("winner retry")
                .outcomes[0]
                .status,
            PushOutcomeStatus::Accepted
        );
        let high_water = fixture
            .collection
            .capture_committed_high_water()
            .await
            .expect("high water");
        let page = fixture
            .collection
            .read_change_page(0, high_water, NonZeroUsize::new(10).expect("page"))
            .await
            .expect("changes");
        assert_eq!(page.changes().len(), 2, "retry must not duplicate history");
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn dropped_push_responses_can_be_retried_in_arbitrary_row_order(
        priorities in proptest::array::uniform5(any::<u16>()),
    ) {
        futures::executor::block_on(async {
            let fixture = Fixture::new();
            let service = PushService::new(
                &fixture.collection,
                &fixture.sequencer,
                &fixture.validator,
                Durability::Memory,
            );
            let rows = (0..5)
                .map(|index| PushRow {
                    mutation_id: format!("chaos-mutation-{index}"),
                    assumed_master_state: None,
                    new_document_state: json!({
                        "id": format!("chaos-todo-{index}"),
                        "value": index,
                        "team": "blue"
                    }),
                })
                .collect::<Vec<_>>();
            let first = PushRequest {
                schema_version: 1,
                rows: rows.clone(),
            };
            let dropped_response = service
                .push(&fixture.blue_context, &first, &AllowAll, &AllowAll)
                .await
                .expect("first push commits before response loss");
            prop_assert!(dropped_response
                .outcomes
                .iter()
                .all(|outcome| outcome.status == PushOutcomeStatus::Accepted));

            let mut order = (0..rows.len()).collect::<Vec<_>>();
            order.sort_by_key(|index| (priorities[*index], *index));
            let retry = PushRequest {
                schema_version: 1,
                rows: order.iter().map(|index| rows[*index].clone()).collect(),
            };
            for _ in 0..2 {
                let response = service
                    .push(&fixture.blue_context, &retry, &AllowAll, &AllowAll)
                    .await
                    .expect("reordered retry");
                prop_assert!(response
                    .outcomes
                    .iter()
                    .all(|outcome| outcome.status == PushOutcomeStatus::Accepted));
            }

            let high_water = fixture
                .collection
                .capture_committed_high_water()
                .await
                .expect("high water");
            let page = fixture
                .collection
                .read_change_page(0, high_water, NonZeroUsize::new(100).expect("page"))
                .await
                .expect("changes");
            prop_assert_eq!(page.changes().len(), rows.len());
            for index in order {
                let document = fixture
                    .collection
                    .get_document(
                        &mako_documents::DocumentId::parse(format!("chaos-todo-{index}"))
                            .expect("document id"),
                    )
                    .await
                    .expect("read")
                    .expect("document");
                prop_assert_eq!(
                    document.body()["value"].as_u64(),
                    u64::try_from(index).ok(),
                );
            }
            Ok(())
        })?;
    }
}

#[test]
fn clients_handle_hidden_visibility_delete_reconnect_and_expired_checkpoint() {
    futures::executor::block_on(async {
        let fixture = Fixture::new();
        let key = ReplicationTokenKey::from_bytes([11; 32]);
        let pull = PullService::new(
            &fixture.collection,
            ReplicationTokenCodec::new(&key),
            SchemaVersion::new(1).expect("schema"),
        );
        let blue = Team("blue");
        let red = Team("red");
        let mut lease = fixture
            .sequencer
            .lease(NonZeroU64::new(5).expect("lease"))
            .await
            .expect("lease");
        let created = create(
            &fixture,
            &mut lease,
            "move-create",
            json!({"id": "todo-move", "value": 1, "team": "blue"}),
        )
        .await;
        let blue_initial = pull
            .initial_pull(
                &fixture.blue_context,
                &PullRequest {
                    checkpoint: None,
                    schema_version: 1,
                    batch_size: 10,
                },
                &blue,
            )
            .await
            .expect("blue initial");
        assert_eq!(blue_initial.documents[0]["id"], "todo-move");

        update(
            &fixture,
            &mut lease,
            created.revision,
            "move-to-red",
            json!({"id": "todo-move", "value": 2, "team": "red"}),
        )
        .await;
        let blue_revoked = pull
            .pull_authorized(
                &fixture.blue_context,
                &PullRequest {
                    checkpoint: Some(blue_initial.checkpoint),
                    schema_version: 1,
                    batch_size: 10,
                },
                &blue,
            )
            .await
            .expect("blue transition");
        assert_eq!(blue_revoked.documents[0]["id"], "todo-move");
        assert_eq!(blue_revoked.documents[0]["_deleted"], true);

        let red_initial = pull
            .initial_pull(
                &fixture.red_context,
                &PullRequest {
                    checkpoint: None,
                    schema_version: 1,
                    batch_size: 10,
                },
                &red,
            )
            .await
            .expect("red initial");
        assert_eq!(red_initial.documents.len(), 1);
        assert_eq!(red_initial.documents[0]["value"], 2);

        create(
            &fixture,
            &mut lease,
            "hidden-red-create",
            json!({"id": "todo-hidden", "value": 3, "team": "red"}),
        )
        .await;
        create(
            &fixture,
            &mut lease,
            "visible-blue-create",
            json!({"id": "todo-visible", "value": 4, "team": "blue"}),
        )
        .await;
        let blue_filled = pull
            .pull_authorized(
                &fixture.blue_context,
                &PullRequest {
                    checkpoint: Some(blue_revoked.checkpoint.clone()),
                    schema_version: 1,
                    batch_size: 1,
                },
                &blue,
            )
            .await
            .expect("blue skips hidden");
        assert_eq!(blue_filled.documents[0]["id"], "todo-visible");

        let red_caught_up = pull
            .pull_authorized(
                &fixture.red_context,
                &PullRequest {
                    checkpoint: Some(red_initial.checkpoint),
                    schema_version: 1,
                    batch_size: 10,
                },
                &red,
            )
            .await
            .expect("red catch up");
        assert_eq!(red_caught_up.documents[0]["id"], "todo-hidden");
        let moved = fixture
            .collection
            .get_document(&mako_documents::DocumentId::parse("todo-move").expect("id"))
            .await
            .expect("read")
            .expect("moved");
        delete(
            &fixture,
            &mut lease,
            moved.revision().clone(),
            "delete-red",
            json!({"id": "todo-move", "value": 2, "team": "red"}),
        )
        .await;

        let mut live = LiveStreamSession::open(
            &fixture.collection,
            ReplicationTokenCodec::new(&key),
            fixture.red_context.clone(),
            &red,
            SchemaVersion::new(1).expect("schema"),
            &LiveStreamRequest {
                schema_version: 1,
                checkpoint: Some(red_caught_up.checkpoint),
                cursor: None,
            },
            LiveStreamLimits::new(
                NonZeroUsize::new(10).expect("batch"),
                NonZeroUsize::new(10).expect("buffer"),
            ),
        )
        .await
        .expect("live");
        live.poll().await.expect("live deletion");
        let LiveStreamEvent::Documents {
            documents, cursor, ..
        } = live.drain_events().next().expect("event")
        else {
            panic!("expected live documents");
        };
        assert_eq!(documents[0]["id"], "todo-move");
        assert_eq!(documents[0]["_deleted"], true);
        let mut reconnect = LiveStreamSession::open(
            &fixture.collection,
            ReplicationTokenCodec::new(&key),
            fixture.red_context.clone(),
            &red,
            SchemaVersion::new(1).expect("schema"),
            &LiveStreamRequest {
                schema_version: 1,
                checkpoint: None,
                cursor: Some(cursor),
            },
            LiveStreamLimits::new(
                NonZeroUsize::new(10).expect("batch"),
                NonZeroUsize::new(10).expect("buffer"),
            ),
        )
        .await
        .expect("reconnect");
        assert!(matches!(
            reconnect.drain_events().next(),
            Some(LiveStreamEvent::Resync {
                reason: ResyncReason::Reconnected
            })
        ));

        fixture
            .collection
            .compact_through(5, Durability::Memory)
            .await
            .expect("compact");
        assert!(matches!(
            pull.pull_authorized(
                &fixture.blue_context,
                &PullRequest {
                    checkpoint: Some(blue_filled.checkpoint),
                    schema_version: 1,
                    batch_size: 10,
                },
                &blue,
            )
            .await,
            Err(PullError::CheckpointExpired {
                minimum_position: 5
            })
        ));
    });
}

#[test]
fn checkpoint_resume_survives_a_rocksdb_service_restart() {
    futures::executor::block_on(async {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("rocksdb");
        let tenant = tenant();
        let context = context(&tenant, "usr_restart1", "ses_restart1", "req_restart1");
        let key = ReplicationTokenKey::from_bytes([13; 32]);
        let adapter = Arc::new(
            RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("open initial RocksDB"),
        );
        let engine = DocumentEngine::new(adapter.clone());
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
            .scope_sequencer(&tenant, &tenant, Durability::Wal)
            .expect("sequencer");
        let validator = validator();
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("lease"))
            .await
            .expect("lease");
        let position = lease.issue().expect("position");
        collection
            .create_document(MutationInput {
                mutation_id: MutationId::parse("restart-create-todo").expect("mutation"),
                commit_position: CommitPosition::new(position).expect("position"),
                document: validator
                    .validate_create(json!({"id": "todo-restart", "value": 1, "team": "blue"}))
                    .expect("document"),
                durability: Durability::Wal,
            })
            .await
            .expect("create");
        sequencer.recover_high_water().await.expect("high water");
        let checkpoint = PullService::new(
            &collection,
            ReplicationTokenCodec::new(&key),
            SchemaVersion::new(1).expect("schema"),
        )
        .initial_pull(
            &context,
            &PullRequest {
                checkpoint: None,
                schema_version: 1,
                batch_size: 10,
            },
            &AllowAll,
        )
        .await
        .expect("initial pull")
        .checkpoint;
        drop(collection);
        drop(sequencer);
        drop(engine);
        drop(adapter);

        let adapter =
            Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("reopen RocksDB"));
        let engine = DocumentEngine::new(adapter);
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
            .scope_sequencer(&tenant, &tenant, Durability::Wal)
            .expect("sequencer");
        assert_eq!(sequencer.recover_high_water().await.expect("recovered"), 1);
        let current = collection
            .get_document(&mako_documents::DocumentId::parse("todo-restart").expect("id"))
            .await
            .expect("read")
            .expect("document");
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("lease"))
            .await
            .expect("lease");
        let position = lease.issue().expect("position");
        collection
            .update_document(
                current.revision().clone(),
                MutationInput {
                    mutation_id: MutationId::parse("restart-update-todo").expect("mutation"),
                    commit_position: CommitPosition::new(position).expect("position"),
                    document: validator
                        .validate_create(json!({"id": "todo-restart", "value": 2, "team": "blue"}))
                        .expect("document"),
                    durability: Durability::Wal,
                },
            )
            .await
            .expect("update");
        sequencer.recover_high_water().await.expect("high water");
        let resumed = PullService::new(
            &collection,
            ReplicationTokenCodec::new(&key),
            SchemaVersion::new(1).expect("schema"),
        )
        .pull_authorized(
            &context,
            &PullRequest {
                checkpoint: Some(checkpoint),
                schema_version: 1,
                batch_size: 10,
            },
            &AllowAll,
        )
        .await
        .expect("resume after restart");
        assert_eq!(resumed.documents.len(), 1);
        assert_eq!(resumed.documents[0]["value"], 2);
    });
}

struct Fixture {
    collection: ScopedCollectionEngine,
    sequencer: EnvironmentSequencer,
    validator: DocumentValidator,
    blue_context: AuthenticatedReplicationContext,
    red_context: AuthenticatedReplicationContext,
}

impl Fixture {
    fn new() -> Self {
        let tenant = tenant();
        let engine = DocumentEngine::new(Arc::new(MemoryAdapter::new()));
        Self {
            collection: engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("collection"),
            sequencer: engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer"),
            validator: validator(),
            blue_context: context(&tenant, "usr_blueuser", "ses_blueuser", "req_blueuser"),
            red_context: context(&tenant, "usr_red_user", "ses_red_user", "req_red_user"),
        }
    }
}

async fn create(
    fixture: &Fixture,
    lease: &mut mako_documents::SequenceLease,
    mutation_id: &str,
    document: Value,
) -> MutationReceipt {
    let MutationCommitOutcome::Applied(receipt) = fixture
        .collection
        .create_document(input(fixture, lease, mutation_id, document))
        .await
        .expect("create")
    else {
        panic!("create must apply");
    };
    fixture
        .sequencer
        .recover_high_water()
        .await
        .expect("high water");
    receipt
}

async fn update(
    fixture: &Fixture,
    lease: &mut mako_documents::SequenceLease,
    revision: RevisionToken,
    mutation_id: &str,
    document: Value,
) -> MutationReceipt {
    let MutationCommitOutcome::Applied(receipt) = fixture
        .collection
        .update_document(revision, input(fixture, lease, mutation_id, document))
        .await
        .expect("update")
    else {
        panic!("update must apply");
    };
    fixture
        .sequencer
        .recover_high_water()
        .await
        .expect("high water");
    receipt
}

async fn delete(
    fixture: &Fixture,
    lease: &mut mako_documents::SequenceLease,
    revision: RevisionToken,
    mutation_id: &str,
    document: Value,
) -> MutationReceipt {
    let MutationCommitOutcome::Applied(receipt) = fixture
        .collection
        .delete_document(revision, input(fixture, lease, mutation_id, document))
        .await
        .expect("delete")
    else {
        panic!("delete must apply");
    };
    fixture
        .sequencer
        .recover_high_water()
        .await
        .expect("high water");
    receipt
}

fn input(
    fixture: &Fixture,
    lease: &mut mako_documents::SequenceLease,
    mutation_id: &str,
    document: Value,
) -> MutationInput {
    MutationInput {
        mutation_id: MutationId::parse(mutation_id).expect("mutation"),
        commit_position: CommitPosition::new(lease.issue().expect("position")).expect("position"),
        document: fixture
            .validator
            .validate_create(document)
            .expect("document"),
        durability: Durability::Memory,
    }
}

fn replication_state(document: &mako_documents::CanonicalDocument) -> Value {
    let mut body = document.body().clone();
    body.insert(
        "_rev".to_owned(),
        Value::String(document.revision().as_str().to_owned()),
    );
    body.insert("_deleted".to_owned(), Value::Bool(document.is_deleted()));
    Value::Object(body)
}

fn validator() -> DocumentValidator {
    DocumentValidator::compile(
        &CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata"),
            SchemaVersion::new(1).expect("schema"),
            json!({
                "type": "object",
                "required": ["id", "value", "team"],
                "properties": {
                    "id": {"type": "string"},
                    "value": {"type": "integer"},
                    "team": {"type": "string"}
                },
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata"),
    )
    .expect("validator")
}

fn context(
    tenant: &TenantScope,
    user: &str,
    session: &str,
    request: &str,
) -> AuthenticatedReplicationContext {
    AuthenticatedReplicationContext::new(
        tenant.clone(),
        CollectionId::parse("todos").expect("collection"),
        AppUserId::parse(user).expect("user"),
        SessionId::parse(session).expect("session"),
        "member",
        AccessAuthorizationEpochs {
            environment: 1,
            user: 1,
        },
        request,
    )
    .expect("context")
}

fn tenant() -> TenantScope {
    TenantScope::new(
        ProjectId::parse("prj_abcdefgh").expect("project"),
        EnvironmentId::parse("env_abcdefgh").expect("environment"),
    )
}
