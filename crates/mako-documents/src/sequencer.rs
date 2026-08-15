use std::{error::Error, fmt, num::NonZeroU64, sync::Arc};

use mako_api::{ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCodecError, KeyCondition, KvAdapter,
    SequencerKeyKind, StorageError, TenantKeyspace, WriteBatch,
};

use crate::DocumentEngine;

const FIRST_POSITION: u64 = 1;
pub(crate) const STATUS_MARKER: &[u8] = b"v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceDisposition {
    Committed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalizeOutcome {
    Recorded,
    AlreadyRecorded,
}

/// A durably reserved, half-open range of sequence positions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequenceLease {
    start: u64,
    end_exclusive: u64,
    next: u64,
}

impl SequenceLease {
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.start
    }

    #[must_use]
    pub const fn end_exclusive(&self) -> u64 {
        self.end_exclusive
    }

    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.end_exclusive - self.next
    }

    pub fn issue(&mut self) -> Option<u64> {
        if self.next == self.end_exclusive {
            return None;
        }
        let position = self.next;
        self.next += 1;
        Some(position)
    }
}

/// Per-project-environment allocator and terminal gap-status recorder.
#[derive(Clone)]
pub struct EnvironmentSequencer {
    adapter: Arc<dyn KvAdapter>,
    keyspace: TenantKeyspace,
    durability: Durability,
}

impl fmt::Debug for EnvironmentSequencer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvironmentSequencer")
            .field("keyspace", &self.keyspace)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl DocumentEngine {
    pub fn scope_sequencer(
        &self,
        trusted_tenant: &TenantScope,
        requested_tenant: &TenantScope,
        durability: Durability,
    ) -> Result<EnvironmentSequencer, SequencerError> {
        requested_tenant.ensure_matches(trusted_tenant)?;
        if self.adapter.capabilities().strongest_durability < durability {
            return Err(SequencerError::UnsupportedDurability(durability));
        }
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str().as_bytes(),
            trusted_tenant.environment_id().as_str().as_bytes(),
        )?;
        Ok(EnvironmentSequencer {
            adapter: Arc::clone(&self.adapter),
            keyspace,
            durability,
        })
    }
}

impl EnvironmentSequencer {
    pub async fn lease(&self, size: NonZeroU64) -> Result<SequenceLease, SequencerError> {
        let key = self.keyspace.sequencer_key(SequencerKeyKind::Lease);
        loop {
            let current = self.adapter.get(&key).await?;
            let start = current
                .as_deref()
                .map(decode_position)
                .transpose()?
                .unwrap_or(FIRST_POSITION);
            let end_exclusive = start
                .checked_add(size.get())
                .ok_or(SequencerError::PositionOverflow)?;

            let condition = match &current {
                Some(value) => KeyCondition::ValueEquals {
                    key: key.clone(),
                    value: value.clone(),
                },
                None => KeyCondition::Missing { key: key.clone() },
            };
            let mut batch = WriteBatch::new();
            batch.put(&key, end_exclusive.to_be_bytes());
            let result = self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability: self.durability,
                })
                .await?;
            if result == CompareAndWriteResult::Applied {
                return Ok(SequenceLease {
                    start,
                    end_exclusive,
                    next: start,
                });
            }
        }
    }

    pub async fn mark_committed(&self, position: u64) -> Result<FinalizeOutcome, SequencerError> {
        self.finalize(position, SequenceDisposition::Committed)
            .await
    }

    pub async fn mark_aborted(&self, position: u64) -> Result<FinalizeOutcome, SequencerError> {
        self.finalize(position, SequenceDisposition::Aborted).await
    }

    pub async fn abort_remaining(&self, lease: &mut SequenceLease) -> Result<(), SequencerError> {
        while let Some(position) = lease.issue() {
            self.mark_aborted(position).await?;
        }
        Ok(())
    }

    pub async fn disposition(
        &self,
        position: u64,
    ) -> Result<Option<SequenceDisposition>, SequencerError> {
        validate_position(position)?;
        let committed_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Committed, position);
        let aborted_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Aborted, position);
        let snapshot = self.adapter.snapshot().await?;
        let committed = snapshot.get(&committed_key).await?.is_some();
        let aborted = snapshot.get(&aborted_key).await?.is_some();
        match (committed, aborted) {
            (true, false) => Ok(Some(SequenceDisposition::Committed)),
            (false, true) => Ok(Some(SequenceDisposition::Aborted)),
            (false, false) => Ok(None),
            (true, true) => Err(SequencerError::CorruptDisposition(position)),
        }
    }

    pub async fn committed_high_water(&self) -> Result<u64, SequencerError> {
        let key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(decode_high_water)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Recovers and advances visibility across the longest contiguous prefix
    /// whose positions are all durably committed or explicitly aborted.
    pub async fn recover_high_water(&self) -> Result<u64, SequencerError> {
        let high_water_key = self.keyspace.sequencer_key(SequencerKeyKind::HighWater);
        loop {
            let stored = self.adapter.get(&high_water_key).await?;
            let current = stored
                .as_deref()
                .map(decode_high_water)
                .transpose()?
                .unwrap_or_default();
            let lease_next = self.next_unallocated_position().await?;
            let highest_allocated = lease_next.saturating_sub(1);
            if current > highest_allocated {
                return Err(SequencerError::HighWaterBeyondAllocation {
                    high_water: current,
                    highest_allocated,
                });
            }

            let mut advanced = current;
            while advanced < highest_allocated {
                let candidate = advanced + 1;
                if self.disposition(candidate).await?.is_none() {
                    break;
                }
                advanced = candidate;
            }
            if advanced == current {
                return Ok(current);
            }

            let condition = match &stored {
                Some(value) => KeyCondition::ValueEquals {
                    key: high_water_key.clone(),
                    value: value.clone(),
                },
                None => KeyCondition::Missing {
                    key: high_water_key.clone(),
                },
            };
            let mut batch = WriteBatch::new();
            batch.put(&high_water_key, advanced.to_be_bytes());
            let result = self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability: self.durability,
                })
                .await?;
            if result == CompareAndWriteResult::Applied {
                return Ok(advanced);
            }
        }
    }

    async fn finalize(
        &self,
        position: u64,
        disposition: SequenceDisposition,
    ) -> Result<FinalizeOutcome, SequencerError> {
        self.require_allocated(position).await?;
        let committed_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Committed, position);
        let aborted_key = self
            .keyspace
            .sequencer_position_key(SequencerKeyKind::Aborted, position);
        let target_key = match disposition {
            SequenceDisposition::Committed => &committed_key,
            SequenceDisposition::Aborted => &aborted_key,
        };
        let mut batch = WriteBatch::new();
        batch.put(target_key, STATUS_MARKER);
        let result = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![
                    KeyCondition::Missing { key: committed_key },
                    KeyCondition::Missing { key: aborted_key },
                ],
                batch,
                durability: self.durability,
            })
            .await?;
        if result == CompareAndWriteResult::Applied {
            return Ok(FinalizeOutcome::Recorded);
        }

        match self.disposition(position).await? {
            Some(actual) if actual == disposition => Ok(FinalizeOutcome::AlreadyRecorded),
            Some(actual) => Err(SequencerError::AlreadyFinalized { position, actual }),
            None => Err(SequencerError::FinalizeRace(position)),
        }
    }

    async fn require_allocated(&self, position: u64) -> Result<(), SequencerError> {
        validate_position(position)?;
        let next = self.next_unallocated_position().await?;
        if position >= next {
            Err(SequencerError::PositionNotAllocated(position))
        } else {
            Ok(())
        }
    }

    async fn next_unallocated_position(&self) -> Result<u64, SequencerError> {
        let lease_key = self.keyspace.sequencer_key(SequencerKeyKind::Lease);
        self.adapter
            .get(&lease_key)
            .await?
            .as_deref()
            .map(decode_position)
            .transpose()
            .map(|position| position.unwrap_or(FIRST_POSITION))
    }
}

#[derive(Debug)]
pub enum SequencerError {
    Scope(ScopeError),
    KeyCodec(KeyCodecError),
    Storage(StorageError),
    UnsupportedDurability(Durability),
    InvalidPosition,
    InvalidStoredPosition,
    PositionOverflow,
    PositionNotAllocated(u64),
    AlreadyFinalized {
        position: u64,
        actual: SequenceDisposition,
    },
    FinalizeRace(u64),
    CorruptDisposition(u64),
    HighWaterBeyondAllocation {
        high_water: u64,
        highest_allocated: u64,
    },
}

impl fmt::Display for SequencerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::InvalidPosition => formatter.write_str("sequence positions must be positive"),
            Self::InvalidStoredPosition => {
                formatter.write_str("stored sequence allocator state is malformed")
            }
            Self::PositionOverflow => formatter.write_str("sequence position overflow"),
            Self::PositionNotAllocated(position) => {
                write!(formatter, "sequence position {position} was not allocated")
            }
            Self::AlreadyFinalized { position, actual } => {
                write!(
                    formatter,
                    "sequence position {position} is already {actual:?}"
                )
            }
            Self::FinalizeRace(position) => {
                write!(
                    formatter,
                    "sequence position {position} changed during finalization"
                )
            }
            Self::CorruptDisposition(position) => write!(
                formatter,
                "sequence position {position} is marked both committed and aborted"
            ),
            Self::HighWaterBeyondAllocation {
                high_water,
                highest_allocated,
            } => write!(
                formatter,
                "committed high water {high_water} exceeds highest allocated position {highest_allocated}"
            ),
        }
    }
}

impl Error for SequencerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScopeError> for SequencerError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<KeyCodecError> for SequencerError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for SequencerError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

fn validate_position(position: u64) -> Result<(), SequencerError> {
    if position == 0 {
        Err(SequencerError::InvalidPosition)
    } else {
        Ok(())
    }
}

pub(crate) fn decode_position(bytes: &[u8]) -> Result<u64, SequencerError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| SequencerError::InvalidStoredPosition)?;
    let position = u64::from_be_bytes(encoded);
    validate_position(position)?;
    Ok(position)
}

pub(crate) fn decode_high_water(bytes: &[u8]) -> Result<u64, SequencerError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| SequencerError::InvalidStoredPosition)?;
    Ok(u64::from_be_bytes(encoded))
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{MemoryAdapter, RocksDbAdapter, RocksDbConfig};
    use tempfile::TempDir;

    use super::*;

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    #[test]
    fn concurrent_leases_are_disjoint_and_terminal_status_is_idempotent() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter);
            let tenant = tenant();
            let first = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let second = first.clone();

            let (left, right) = futures::join!(
                first.lease(NonZeroU64::new(4).expect("non-zero")),
                second.lease(NonZeroU64::new(3).expect("non-zero"))
            );
            let left = left.expect("left lease");
            let right = right.expect("right lease");
            assert!(left.end_exclusive() <= right.start() || right.end_exclusive() <= left.start());

            let position = left.start();
            assert_eq!(
                first.mark_committed(position).await.expect("commit"),
                FinalizeOutcome::Recorded
            );
            assert_eq!(
                first.mark_committed(position).await.expect("repeat commit"),
                FinalizeOutcome::AlreadyRecorded
            );
            assert!(matches!(
                first.mark_aborted(position).await,
                Err(SequencerError::AlreadyFinalized {
                    actual: SequenceDisposition::Committed,
                    ..
                })
            ));
        });
    }

    #[test]
    fn leases_and_gap_markers_survive_a_rocksdb_restart() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("sequencer.rocksdb");
        let tenant = tenant();

        block_on(async {
            let adapter =
                Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("open RocksDB"));
            let engine = DocumentEngine::new(adapter);
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Sync)
                .expect("sequencer");
            let mut lease = sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");
            assert_eq!(lease.issue(), Some(1));
            sequencer.mark_committed(1).await.expect("commit marker");
            assert_eq!(lease.issue(), Some(2));
            sequencer.mark_aborted(2).await.expect("abort marker");
        });

        block_on(async {
            let adapter =
                Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("reopen RocksDB"));
            let engine = DocumentEngine::new(adapter);
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Sync)
                .expect("sequencer");

            assert_eq!(
                sequencer.disposition(1).await.expect("position 1"),
                Some(SequenceDisposition::Committed)
            );
            assert_eq!(
                sequencer.disposition(2).await.expect("position 2"),
                Some(SequenceDisposition::Aborted)
            );
            assert_eq!(sequencer.disposition(3).await.expect("position 3"), None);
            let next = sequencer
                .lease(NonZeroU64::new(2).expect("non-zero"))
                .await
                .expect("next lease");
            assert_eq!(next.start(), 4);
            assert_eq!(next.end_exclusive(), 6);
        });
    }

    #[test]
    fn unallocated_positions_and_cross_tenant_scope_are_rejected() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter);
            let trusted = tenant();
            let other = TenantScope::new(
                ProjectId::parse("prj_ijklmnop").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            );
            assert!(matches!(
                engine.scope_sequencer(&trusted, &other, Durability::Memory),
                Err(SequencerError::Scope(ScopeError::TenantMismatch))
            ));

            let sequencer = engine
                .scope_sequencer(&trusted, &trusted, Durability::Memory)
                .expect("sequencer");
            assert!(matches!(
                sequencer.mark_committed(1).await,
                Err(SequencerError::PositionNotAllocated(1))
            ));
        });
    }

    #[test]
    fn high_water_stops_at_unresolved_gaps_and_recovers_after_restart() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("high-water.rocksdb");
        let tenant = tenant();

        block_on(async {
            let adapter =
                Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("open RocksDB"));
            let engine = DocumentEngine::new(adapter);
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Sync)
                .expect("sequencer");
            let lease = sequencer
                .lease(NonZeroU64::new(5).expect("non-zero"))
                .await
                .expect("lease");
            assert_eq!((lease.start(), lease.end_exclusive()), (1, 6));

            sequencer.mark_committed(1).await.expect("position 1");
            sequencer.mark_committed(3).await.expect("position 3");
            sequencer.mark_aborted(4).await.expect("position 4");
            assert_eq!(sequencer.recover_high_water().await.expect("advance"), 1);
            assert_eq!(sequencer.committed_high_water().await.expect("read"), 1);

            sequencer.mark_aborted(2).await.expect("position 2");
            assert_eq!(sequencer.recover_high_water().await.expect("advance"), 4);
        });

        block_on(async {
            let adapter =
                Arc::new(RocksDbAdapter::open(RocksDbConfig::new(&path)).expect("reopen RocksDB"));
            let engine = DocumentEngine::new(adapter);
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Sync)
                .expect("sequencer");

            assert_eq!(sequencer.recover_high_water().await.expect("recover"), 4);
            sequencer.mark_committed(5).await.expect("position 5");
            assert_eq!(sequencer.recover_high_water().await.expect("advance"), 5);
            assert_eq!(sequencer.committed_high_water().await.expect("read"), 5);
        });
    }
}
