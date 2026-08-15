use futures::executor::block_on;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, FailurePoint, KeyCondition, KvAdapter,
    MemoryAdapter, StorageError, StorageErrorKind, WriteBatch,
};

const PRECOMMIT_POINTS: [FailurePoint; 6] = [
    FailurePoint::BeforeWrite,
    FailurePoint::BeforeConditionCheck,
    FailurePoint::AfterConditionCheck,
    FailurePoint::BeforeBatchStage,
    FailurePoint::AfterBatchStage,
    FailurePoint::BeforeCommit,
];

#[test]
fn every_precommit_io_failure_leaves_no_candidate_state() {
    block_on(async {
        for point in PRECOMMIT_POINTS {
            let adapter = seeded_adapter().await;
            adapter
                .fail_next(point, injected_io(point))
                .expect("inject failure");

            let error = adapter
                .compare_and_write(candidate_mutation())
                .await
                .expect_err("injected operation must fail");

            assert_eq!(error.kind, StorageErrorKind::Io, "wrong error at {point:?}");
            assert_precommit_state(&adapter).await;
        }
    });
}

#[test]
fn postcommit_preack_failure_exposes_the_whole_commit_not_a_partial_batch() {
    block_on(async {
        let adapter = seeded_adapter().await;
        adapter
            .fail_next(
                FailurePoint::AfterCommit,
                injected_io(FailurePoint::AfterCommit),
            )
            .expect("inject failure");

        let error = adapter
            .compare_and_write(candidate_mutation())
            .await
            .expect_err("postcommit acknowledgement must be unknown");

        assert_eq!(error.kind, StorageErrorKind::Io);
        assert_committed_state(&adapter).await;

        let retry = adapter
            .compare_and_write(candidate_mutation())
            .await
            .expect("retry returns a conflict");
        assert!(matches!(retry, CompareAndWriteResult::Conflict { .. }));
        assert_committed_state(&adapter).await;
    });
}

#[test]
fn acknowledged_commit_exposes_every_derived_key() {
    block_on(async {
        let adapter = seeded_adapter().await;

        let result = adapter
            .compare_and_write(candidate_mutation())
            .await
            .expect("commit");

        assert_eq!(result, CompareAndWriteResult::Applied);
        assert_committed_state(&adapter).await;
    });
}

async fn seeded_adapter() -> MemoryAdapter {
    let adapter = MemoryAdapter::new();
    let mut seed = WriteBatch::new();
    seed.put(b"document", b"revision-1");
    adapter
        .write(seed, Durability::Memory)
        .await
        .expect("seed adapter");
    adapter
}

fn candidate_mutation() -> AtomicWrite {
    let mut batch = WriteBatch::new();
    batch.put(b"document", b"revision-2");
    batch.put(b"index", b"revision-2");
    batch.put(b"change", b"revision-2");
    batch.put(b"idempotency", b"revision-2");
    AtomicWrite {
        conditions: vec![KeyCondition::ValueEquals {
            key: b"document".to_vec(),
            value: b"revision-1".to_vec(),
        }],
        batch,
        durability: Durability::Memory,
    }
}

fn injected_io(point: FailurePoint) -> StorageError {
    StorageError::new(
        StorageErrorKind::Io,
        "fault_injection",
        true,
        format!("injected failure at {point:?}"),
    )
}

async fn assert_precommit_state(adapter: &MemoryAdapter) {
    assert_eq!(
        adapter.get(b"document").await.expect("document"),
        Some(b"revision-1".to_vec())
    );
    for key in [b"index".as_slice(), b"change", b"idempotency"] {
        assert_eq!(adapter.get(key).await.expect("derived key"), None);
    }
}

async fn assert_committed_state(adapter: &MemoryAdapter) {
    for key in [b"document".as_slice(), b"index", b"change", b"idempotency"] {
        assert_eq!(
            adapter.get(key).await.expect("committed key"),
            Some(b"revision-2".to_vec())
        );
    }
}
