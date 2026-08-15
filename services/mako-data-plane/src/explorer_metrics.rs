use std::{
    array,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use mako_api::{ExplorerAccessMode, ExplorerOperation};

const OPERATION_COUNT: usize = 9;
const MODE_COUNT: usize = 2;
const OUTCOME_COUNT: usize = 3;
const LATENCY_BUCKETS_MILLISECONDS: [u64; 6] = [5, 25, 100, 500, 2_000, u64::MAX];
const GRANT_FAILURE_COUNT: usize = 5;

/// Fixed-cardinality explorer metrics. None of these vectors can accept a
/// tenant, user, document, token, email, or caller-supplied label.
pub struct ExplorerMetrics {
    requests_by_operation: [AtomicU64; OPERATION_COUNT],
    authorizations_by_mode: [AtomicU64; MODE_COUNT],
    outcomes: [AtomicU64; OUTCOME_COUNT],
    latency_buckets: [AtomicU64; LATENCY_BUCKETS_MILLISECONDS.len()],
    grant_failures: [AtomicU64; GRANT_FAILURE_COUNT],
    query_plan_failures: AtomicU64,
    conflicts: AtomicU64,
}

impl Default for ExplorerMetrics {
    fn default() -> Self {
        Self {
            requests_by_operation: array::from_fn(|_| AtomicU64::new(0)),
            authorizations_by_mode: array::from_fn(|_| AtomicU64::new(0)),
            outcomes: array::from_fn(|_| AtomicU64::new(0)),
            latency_buckets: array::from_fn(|_| AtomicU64::new(0)),
            grant_failures: array::from_fn(|_| AtomicU64::new(0)),
            query_plan_failures: AtomicU64::new(0),
            conflicts: AtomicU64::new(0),
        }
    }
}

impl ExplorerMetrics {
    pub fn observe_request(
        &self,
        operation: ExplorerOperation,
        succeeded: bool,
        elapsed: Duration,
    ) {
        self.requests_by_operation[operation_index(operation)].fetch_add(1, Ordering::Relaxed);
        self.outcomes[if succeeded { 0 } else { 2 }].fetch_add(1, Ordering::Relaxed);
        let milliseconds = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let bucket = LATENCY_BUCKETS_MILLISECONDS
            .iter()
            .position(|limit| milliseconds <= *limit)
            .unwrap_or(LATENCY_BUCKETS_MILLISECONDS.len() - 1);
        self.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_authorized_mode(&self, mode: ExplorerAccessMode) {
        self.authorizations_by_mode[match mode {
            ExplorerAccessMode::PolicyPreview => 0,
            ExplorerAccessMode::Administrative => 1,
        }]
        .fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_grant_failure(&self, failure: GrantFailureClass) {
        self.grant_failures[failure as usize].fetch_add(1, Ordering::Relaxed);
        self.outcomes[1].fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_query_plan_failure(&self) {
        self.query_plan_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_conflict(&self) {
        self.conflicts.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> ExplorerMetricsSnapshot {
        ExplorerMetricsSnapshot {
            requests_by_operation: self.requests_by_operation.each_ref().map(load),
            authorizations_by_mode: self.authorizations_by_mode.each_ref().map(load),
            outcomes: self.outcomes.each_ref().map(load),
            latency_buckets: self.latency_buckets.each_ref().map(load),
            grant_failures: self.grant_failures.each_ref().map(load),
            query_plan_failures: self.query_plan_failures.load(Ordering::Relaxed),
            conflicts: self.conflicts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantFailureClass {
    Missing = 0,
    Malformed = 1,
    Unverified = 2,
    ScopeOrOperation = 3,
    RevokedOrStale = 4,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerMetricsSnapshot {
    pub requests_by_operation: [u64; OPERATION_COUNT],
    pub authorizations_by_mode: [u64; MODE_COUNT],
    pub outcomes: [u64; OUTCOME_COUNT],
    pub latency_buckets: [u64; LATENCY_BUCKETS_MILLISECONDS.len()],
    pub grant_failures: [u64; GRANT_FAILURE_COUNT],
    pub query_plan_failures: u64,
    pub conflicts: u64,
}

fn load(value: &AtomicU64) -> u64 {
    value.load(Ordering::Relaxed)
}

const fn operation_index(operation: ExplorerOperation) -> usize {
    match operation {
        ExplorerOperation::Get => 0,
        ExplorerOperation::Browse => 1,
        ExplorerOperation::Query => 2,
        ExplorerOperation::Plan => 3,
        ExplorerOperation::History => 4,
        ExplorerOperation::Simulate => 5,
        ExplorerOperation::Mutate => 6,
        ExplorerOperation::Import => 7,
        ExplorerOperation::Export => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_closed_enums_and_snapshots_are_bounded() {
        let metrics = ExplorerMetrics::default();
        metrics.observe_request(ExplorerOperation::Query, false, Duration::from_millis(101));
        metrics.observe_authorized_mode(ExplorerAccessMode::PolicyPreview);
        metrics.observe_grant_failure(GrantFailureClass::RevokedOrStale);
        metrics.observe_query_plan_failure();
        metrics.observe_conflict();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.requests_by_operation[2], 1);
        assert_eq!(snapshot.authorizations_by_mode, [1, 0]);
        assert_eq!(snapshot.outcomes, [0, 1, 1]);
        assert_eq!(snapshot.latency_buckets[3], 1);
        assert_eq!(snapshot.grant_failures[4], 1);
        assert_eq!(snapshot.query_plan_failures, 1);
        assert_eq!(snapshot.conflicts, 1);
    }
}
