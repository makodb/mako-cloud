//! Commit groups, and the barrier an online checkpoint closes.
//!
//! A sequence-allocating writer leases positions with one write and resolves
//! them with others. A checkpoint taken between those writes would capture
//! positions that are neither committed nor aborted, and a restore from it
//! would hold that tenant's high water behind the gap. A writer therefore
//! enters a commit group before it leases and leaves when its lease is
//! dropped; an online checkpoint closes the barrier, waits a bounded time for
//! the open groups to drain, and only then copies the database.
//!
//! There is no async runtime here: the barrier is a `Mutex` and a `Condvar`,
//! and a member holds an owned token rather than a borrowed guard, so the
//! token can be carried across the `await`s `block_on` drives.

use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use crate::{StorageError, StorageErrorKind, StorageResult};

/// How long a writer waits at a closed barrier before reporting the storage as
/// temporarily unavailable. A checkpoint closes the barrier for its drain bound
/// plus the time to link the database, so this only trips if a checkpoint
/// wedges, and then writers fail retryably instead of hanging.
pub const DEFAULT_COMMIT_GROUP_ENTRY_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct BarrierState {
    open_groups: u64,
    closed: bool,
}

/// Counts open commit groups and lets one checkpoint at a time close entry.
#[derive(Debug, Default)]
pub struct CommitGroupBarrier {
    state: Mutex<BarrierState>,
    changed: Condvar,
}

impl CommitGroupBarrier {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Join a commit group, waiting at most `wait` while a checkpoint has the
    /// barrier closed.
    pub fn enter(self: &Arc<Self>, wait: Duration) -> StorageResult<CommitGroupToken> {
        let deadline = Instant::now() + wait;
        let mut state = self.lock()?;
        while state.closed {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(StorageError::new(
                    StorageErrorKind::Unavailable,
                    "commit_group",
                    true,
                    "storage is completing a checkpoint",
                ));
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| poisoned())?
                .0;
        }
        state.open_groups += 1;
        Ok(CommitGroupToken {
            barrier: Some(Arc::clone(self)),
        })
    }

    /// Close the barrier and wait at most `drain` for every open group to
    /// leave. On success the barrier stays closed until the returned value is
    /// dropped. On timeout the barrier reopens before the error is returned,
    /// so writers are never held longer than the bound.
    pub fn quiesce(self: &Arc<Self>, drain: Duration) -> StorageResult<QuiescedBarrier> {
        let started = Instant::now();
        let deadline = started + drain;
        let mut state = self.lock()?;
        if state.closed {
            return Err(StorageError::new(
                StorageErrorKind::Conflict,
                "commit_group",
                true,
                "another checkpoint is in progress",
            ));
        }
        state.closed = true;
        while state.open_groups > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.closed = false;
                drop(state);
                self.changed.notify_all();
                return Err(StorageError::new(
                    StorageErrorKind::Unavailable,
                    "commit_group",
                    true,
                    "open commit groups did not drain within the checkpoint bound",
                ));
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| poisoned())?
                .0;
        }
        drop(state);
        Ok(QuiescedBarrier {
            barrier: Arc::clone(self),
            waited: started.elapsed(),
        })
    }

    /// Commit groups currently open; for tests and diagnostics.
    #[must_use]
    pub fn open_groups(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.open_groups)
    }

    fn lock(&self) -> StorageResult<MutexGuard<'_, BarrierState>> {
        self.state.lock().map_err(|_| poisoned())
    }

    fn leave(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.open_groups = state.open_groups.saturating_sub(1);
        }
        self.changed.notify_all();
    }

    fn reopen(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = false;
        }
        self.changed.notify_all();
    }
}

/// Membership in one commit group; leaving happens on drop.
#[derive(Debug)]
pub struct CommitGroupToken {
    barrier: Option<Arc<CommitGroupBarrier>>,
}

impl CommitGroupToken {
    /// A token for an engine that never takes online checkpoints.
    #[must_use]
    pub const fn detached() -> Self {
        Self { barrier: None }
    }
}

impl Drop for CommitGroupToken {
    fn drop(&mut self) {
        if let Some(barrier) = self.barrier.take() {
            barrier.leave();
        }
    }
}

/// A closed, drained barrier. Dropping it lets writers in again.
#[derive(Debug)]
pub struct QuiescedBarrier {
    barrier: Arc<CommitGroupBarrier>,
    waited: Duration,
}

impl QuiescedBarrier {
    /// How long the checkpoint waited for open groups to drain.
    #[must_use]
    pub const fn waited(&self) -> Duration {
        self.waited
    }
}

impl Drop for QuiescedBarrier {
    fn drop(&mut self) {
        self.barrier.reopen();
    }
}

fn poisoned() -> StorageError {
    StorageError::new(
        StorageErrorKind::Internal,
        "commit_group",
        false,
        "commit group barrier is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread};

    use super::*;

    #[test]
    fn quiesce_waits_for_open_groups_and_blocks_new_ones() {
        let barrier = CommitGroupBarrier::new();
        let member = barrier.enter(Duration::from_secs(1)).expect("enter");
        let (released_tx, released_rx) = mpsc::channel();
        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            drop(member);
            released_tx.send(()).expect("signal");
        });

        let quiesced = barrier.quiesce(Duration::from_secs(5)).expect("drains");
        released_rx.recv().expect("member left first");
        assert_eq!(barrier.open_groups(), 0);
        assert!(quiesced.waited() >= Duration::from_millis(40));

        let blocked = barrier.enter(Duration::from_millis(20));
        assert_eq!(
            blocked.expect_err("closed barrier refuses entry").kind,
            StorageErrorKind::Unavailable
        );
        drop(quiesced);
        let _member = barrier.enter(Duration::from_millis(20)).expect("reopened");
        holder.join().expect("holder");
    }

    #[test]
    fn a_group_that_outlasts_the_bound_abandons_the_checkpoint_and_reopens() {
        let barrier = CommitGroupBarrier::new();
        let member = barrier.enter(Duration::from_secs(1)).expect("enter");
        let error = barrier
            .quiesce(Duration::from_millis(30))
            .expect_err("long group outlasts bound");
        assert_eq!(error.kind, StorageErrorKind::Unavailable);
        assert!(error.retryable);
        let _second = barrier
            .enter(Duration::from_millis(10))
            .expect("barrier reopened after the abandoned checkpoint");
        drop(member);
    }

    #[test]
    fn only_one_checkpoint_closes_the_barrier_at_a_time() {
        let barrier = CommitGroupBarrier::new();
        let first = barrier.quiesce(Duration::from_millis(10)).expect("first");
        let second = barrier.quiesce(Duration::from_millis(10));
        assert_eq!(
            second.expect_err("second checkpoint conflicts").kind,
            StorageErrorKind::Conflict
        );
        drop(first);
        barrier
            .quiesce(Duration::from_millis(10))
            .expect("after release");
    }

    #[test]
    fn a_waiting_writer_enters_as_soon_as_the_checkpoint_finishes() {
        let barrier = CommitGroupBarrier::new();
        let quiesced = barrier.quiesce(Duration::from_millis(10)).expect("closed");
        let waiting = Arc::clone(&barrier);
        let writer = thread::spawn(move || waiting.enter(Duration::from_secs(5)).map(drop));
        thread::sleep(Duration::from_millis(30));
        drop(quiesced);
        writer
            .join()
            .expect("writer")
            .expect("entered after reopen");
    }

    #[test]
    fn detached_tokens_are_inert() {
        drop(CommitGroupToken::detached());
    }
}
