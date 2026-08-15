use std::{num::NonZeroUsize, sync::Arc};

use futures::executor::block_on;
use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
use mako_documents::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
    DocumentEngine, DocumentId, DocumentValidator, MutationCommitOutcome, MutationId,
    MutationInput, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion, SequenceDisposition,
};
use mako_storage::{
    Durability, KvAdapter, RocksDbAdapter, RocksDbConfig, ScanDirection, ScanRequest,
    TenantKeyspace,
};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn concurrent_revision_change_and_acknowledgement_invariants_survive_restart() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("documents.rocksdb");
    let tenant = tenant();

    let (winner, winner_input, assumed_revision, loser_position) = block_on(async {
        let adapter =
            Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("open local RocksDB"));
        let engine = DocumentEngine::new(adapter);
        let scoped = engine
            .scope_collection(&tenant, collection(&tenant))
            .expect("scoped collection");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Sync)
            .expect("sequencer");
        let validator = DocumentValidator::compile(&metadata()).expect("validator");
        sequencer
            .lease(std::num::NonZeroU64::new(3).expect("non-zero"))
            .await
            .expect("lease");

        let created = MutationInput {
            mutation_id: MutationId::parse("create-todo").expect("mutation id"),
            commit_position: CommitPosition::new(1).expect("position"),
            document: validator
                .validate_create(json!({"id": "todo-1", "title": "created"}))
                .expect("document"),
            durability: Durability::Sync,
        };
        let MutationCommitOutcome::Applied(created) =
            scoped.create_document(created).await.expect("create")
        else {
            panic!("create must apply");
        };
        let assumed_revision = created.revision;

        let left_input = MutationInput {
            mutation_id: MutationId::parse("update-left").expect("mutation id"),
            commit_position: CommitPosition::new(2).expect("position"),
            document: validator
                .validate_create(json!({"id": "todo-1", "title": "left"}))
                .expect("document"),
            durability: Durability::Sync,
        };
        let right_input = MutationInput {
            mutation_id: MutationId::parse("update-right").expect("mutation id"),
            commit_position: CommitPosition::new(3).expect("position"),
            document: validator
                .validate_create(json!({"id": "todo-1", "title": "right"}))
                .expect("document"),
            durability: Durability::Sync,
        };

        let (left, right) = futures::join!(
            scoped.update_document(assumed_revision.clone(), left_input.clone()),
            scoped.update_document(assumed_revision.clone(), right_input.clone())
        );
        let left = left.expect("left outcome");
        let right = right.expect("right outcome");
        let (winner, winner_input, loser_position) = match (left, right) {
            (
                MutationCommitOutcome::Applied(receipt),
                MutationCommitOutcome::RevisionConflict { .. },
            ) => (receipt, left_input, 3),
            (
                MutationCommitOutcome::RevisionConflict { .. },
                MutationCommitOutcome::Applied(receipt),
            ) => (receipt, right_input, 2),
            outcomes => panic!("exactly one update must commit: {outcomes:?}"),
        };

        sequencer
            .mark_aborted(loser_position)
            .await
            .expect("abort losing position");
        assert_eq!(sequencer.recover_high_water().await.expect("high water"), 3);
        let current = scoped
            .get_document(&DocumentId::parse("todo-1").expect("document id"))
            .await
            .expect("read")
            .expect("current document");
        assert_eq!(current.revision(), &winner.revision);
        assert_eq!(current.commit_position(), winner.commit_position);

        (winner, winner_input, assumed_revision, loser_position)
    });

    block_on(async {
        let adapter = Arc::new(
            RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("reopen local RocksDB"),
        );
        let engine = DocumentEngine::new(adapter.clone());
        let scoped = engine
            .scope_collection(&tenant, collection(&tenant))
            .expect("scoped collection");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Sync)
            .expect("sequencer");

        let current = scoped
            .get_document(&DocumentId::parse("todo-1").expect("document id"))
            .await
            .expect("read after restart")
            .expect("current document");
        assert_eq!(current.revision(), &winner.revision);
        assert_eq!(current.commit_position(), winner.commit_position);
        assert_eq!(sequencer.recover_high_water().await.expect("recover"), 3);
        assert_eq!(
            sequencer
                .disposition(loser_position)
                .await
                .expect("loser disposition"),
            Some(SequenceDisposition::Aborted)
        );

        let replay = scoped
            .update_document(assumed_revision, winner_input)
            .await
            .expect("replay after restart");
        assert_eq!(replay, MutationCommitOutcome::Replayed(winner.clone()));

        let keyspace = TenantKeyspace::new(
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
        )
        .expect("keyspace");
        let changes = adapter
            .scan(ScanRequest::new(
                keyspace.changes_range("todos").expect("change range"),
                ScanDirection::Forward,
                NonZeroUsize::new(10).expect("non-zero"),
            ))
            .await
            .expect("changes");
        let revisions = adapter
            .scan(ScanRequest::new(
                keyspace.revisions_range("todos").expect("revision range"),
                ScanDirection::Forward,
                NonZeroUsize::new(10).expect("non-zero"),
            ))
            .await
            .expect("revisions");

        assert_eq!(changes.len(), 2);
        assert_eq!(revisions.len(), 2);
        let positions: Vec<_> = changes
            .iter()
            .map(|entry| {
                keyspace
                    .decode_change_key("todos", &entry.key)
                    .expect("change key")
                    .position
            })
            .collect();
        assert_eq!(positions, [1, winner.commit_position.get()]);
    });
}

fn tenant() -> TenantScope {
    TenantScope::new(
        ProjectId::parse("prj_abcdefgh").expect("project"),
        EnvironmentId::parse("env_abcdefgh").expect("environment"),
    )
}

fn collection(tenant: &TenantScope) -> CollectionScope {
    CollectionScope::new(
        tenant.clone(),
        CollectionId::parse("todos").expect("collection"),
    )
}

fn metadata() -> CollectionMetadata {
    CollectionMetadata::new(
        CollectionId::parse("todos").expect("collection"),
        CollectionMetadataVersion::new(1).expect("metadata version"),
        SchemaVersion::new(1).expect("schema version"),
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "title": {"type": "string"}
            },
            "required": ["id", "title"],
            "additionalProperties": false
        }),
        PrimaryKeyDefinition::field("id").expect("primary key"),
        SchemaCompatibility::Compatible,
        CollectionLifecycle::Active,
    )
    .expect("metadata")
}
