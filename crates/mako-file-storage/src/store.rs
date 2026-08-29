use std::{num::NonZeroUsize, sync::Arc};

use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, TenantKeyspace, WriteBatch,
};

use crate::{BucketRecord, BucketTotals, FileStorageError, ObjectRecord};

/// Bucket configuration, object metadata, and totals in the tenant's keyspace.
#[derive(Clone)]
pub struct BucketStore {
    adapter: Arc<dyn KvAdapter>,
    keyspace: TenantKeyspace,
    durability: Durability,
}

impl std::fmt::Debug for BucketStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BucketStore")
    }
}

/// The totals record as read, so a write can be conditioned on it.
#[derive(Clone, Debug, Default)]
pub(crate) struct ObservedTotals {
    pub(crate) totals: BucketTotals,
    raw: Option<Vec<u8>>,
}

impl BucketStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        tenant: &TenantScope,
        durability: Durability,
    ) -> Result<Self, FileStorageError> {
        let keyspace = TenantKeyspace::new(
            tenant.project_id().as_str().as_bytes().to_vec(),
            tenant.environment_id().as_str().as_bytes().to_vec(),
        )
        .map_err(|_| FileStorageError::Storage)?;
        Ok(Self {
            adapter,
            keyspace,
            durability,
        })
    }

    pub async fn get_bucket(
        &self,
        bucket_id: &str,
    ) -> Result<Option<BucketRecord>, FileStorageError> {
        let key = self
            .keyspace
            .bucket_key(bucket_id)
            .map_err(|_| FileStorageError::Storage)?;
        self.read(&key).await
    }

    pub async fn list_buckets(
        &self,
        limit: NonZeroUsize,
    ) -> Result<Vec<BucketRecord>, FileStorageError> {
        let range = self
            .keyspace
            .buckets_range()
            .map_err(|_| FileStorageError::Storage)?;
        let entries = self
            .adapter
            .scan(ScanRequest {
                range,
                direction: ScanDirection::Forward,
                limit,
            })
            .await
            .map_err(|_| FileStorageError::Storage)?;
        entries
            .into_iter()
            .map(|entry| {
                serde_json::from_slice(&entry.value).map_err(|_| FileStorageError::Corrupt)
            })
            .collect()
    }

    /// Creates or replaces a bucket, conditioned on what the caller read.
    pub async fn put_bucket(
        &self,
        previous: Option<&BucketRecord>,
        next: &BucketRecord,
    ) -> Result<(), FileStorageError> {
        let key = self
            .keyspace
            .bucket_key(&next.config.id)
            .map_err(|_| FileStorageError::Storage)?;
        let condition = match previous {
            Some(previous) => KeyCondition::ValueEquals {
                key: key.clone(),
                value: encode(previous)?,
            },
            None => KeyCondition::Missing { key: key.clone() },
        };
        let mut batch = WriteBatch::new();
        batch.put(&key, encode(next)?);
        self.apply(vec![condition], batch).await
    }

    /// Removes a bucket and its totals; the caller has already emptied it.
    pub async fn delete_bucket(&self, record: &BucketRecord) -> Result<(), FileStorageError> {
        let key = self
            .keyspace
            .bucket_key(&record.config.id)
            .map_err(|_| FileStorageError::Storage)?;
        let totals_key = self
            .keyspace
            .bucket_totals_key(&record.config.id)
            .map_err(|_| FileStorageError::Storage)?;
        let mut batch = WriteBatch::new();
        batch.delete(&key);
        batch.delete(&totals_key);
        self.apply(
            vec![KeyCondition::ValueEquals {
                key,
                value: encode(record)?,
            }],
            batch,
        )
        .await
    }

    pub async fn totals(&self, bucket_id: &str) -> Result<BucketTotals, FileStorageError> {
        Ok(self.observe_totals(bucket_id).await?.totals)
    }

    pub(crate) async fn observe_totals(
        &self,
        bucket_id: &str,
    ) -> Result<ObservedTotals, FileStorageError> {
        let key = self
            .keyspace
            .bucket_totals_key(bucket_id)
            .map_err(|_| FileStorageError::Storage)?;
        let raw = self
            .adapter
            .get(&key)
            .await
            .map_err(|_| FileStorageError::Storage)?;
        let totals = match &raw {
            Some(bytes) => serde_json::from_slice(bytes).map_err(|_| FileStorageError::Corrupt)?,
            None => BucketTotals::default(),
        };
        Ok(ObservedTotals { totals, raw })
    }

    /// Every bucket's totals, summed: the environment's stored object bytes.
    pub async fn environment_totals(&self) -> Result<BucketTotals, FileStorageError> {
        let range = self
            .keyspace
            .bucket_totals_range()
            .map_err(|_| FileStorageError::Storage)?;
        let entries = self
            .adapter
            .scan(ScanRequest {
                range,
                direction: ScanDirection::Forward,
                limit: NonZeroUsize::new(10_000).expect("limit"),
            })
            .await
            .map_err(|_| FileStorageError::Storage)?;
        let mut sum = BucketTotals::default();
        for entry in entries {
            let totals: BucketTotals =
                serde_json::from_slice(&entry.value).map_err(|_| FileStorageError::Corrupt)?;
            sum.object_count = sum.object_count.saturating_add(totals.object_count);
            sum.total_bytes = sum.total_bytes.saturating_add(totals.total_bytes);
        }
        Ok(sum)
    }

    pub async fn get_object(
        &self,
        bucket_id: &str,
        path: &str,
    ) -> Result<Option<ObjectRecord>, FileStorageError> {
        let key = self
            .keyspace
            .object_key(bucket_id, path)
            .map_err(|_| FileStorageError::Storage)?;
        self.read(&key).await
    }

    /// Objects of a bucket in path order, optionally under a prefix and after a path.
    pub async fn list_objects(
        &self,
        bucket_id: &str,
        prefix: Option<&str>,
        after: Option<&str>,
        limit: NonZeroUsize,
    ) -> Result<Vec<ObjectRecord>, FileStorageError> {
        let mut range = self
            .keyspace
            .objects_range(bucket_id)
            .map_err(|_| FileStorageError::Storage)?;
        if let Some(after) = after {
            let mut start = self
                .keyspace
                .object_key(bucket_id, after)
                .map_err(|_| FileStorageError::Storage)?;
            start.push(0);
            if range.contains(&start) {
                range.start_inclusive = start;
            }
        }
        let mut items = Vec::new();
        // A prefix narrows the result, not the scan: keys are whole-value escaped,
        // so the scan walks the bucket in order and keeps what matches.
        let page = NonZeroUsize::new(limit.get().saturating_mul(4).max(64)).expect("page");
        loop {
            let entries = self
                .adapter
                .scan(ScanRequest {
                    range: range.clone(),
                    direction: ScanDirection::Forward,
                    limit: page,
                })
                .await
                .map_err(|_| FileStorageError::Storage)?;
            let count = entries.len();
            let mut last_key = None;
            for entry in entries {
                let record: ObjectRecord =
                    serde_json::from_slice(&entry.value).map_err(|_| FileStorageError::Corrupt)?;
                last_key = Some(entry.key);
                if prefix.is_none_or(|prefix| record.path.starts_with(prefix)) {
                    items.push(record);
                    if items.len() >= limit.get() {
                        return Ok(items);
                    }
                }
            }
            match last_key {
                Some(mut key) if count == page.get() => {
                    key.push(0);
                    range.start_inclusive = key;
                }
                _ => return Ok(items),
            }
        }
    }

    /// Writes an object record and the bucket's new totals together, conditioned
    /// on the record and totals the caller observed.
    pub(crate) async fn commit_object(
        &self,
        previous: Option<&ObjectRecord>,
        next: &ObjectRecord,
        observed: &ObservedTotals,
        totals: BucketTotals,
    ) -> Result<(), FileStorageError> {
        let key = self
            .keyspace
            .object_key(&next.bucket_id, &next.path)
            .map_err(|_| FileStorageError::Storage)?;
        let condition = match previous {
            Some(previous) => KeyCondition::ValueEquals {
                key: key.clone(),
                value: encode(previous)?,
            },
            None => KeyCondition::Missing { key: key.clone() },
        };
        let mut batch = WriteBatch::new();
        batch.put(&key, encode(next)?);
        let mut conditions = vec![condition];
        self.stage_totals(
            &next.bucket_id,
            observed,
            totals,
            &mut conditions,
            &mut batch,
        )?;
        self.apply(conditions, batch).await
    }

    pub(crate) async fn remove_object(
        &self,
        record: &ObjectRecord,
        observed: &ObservedTotals,
        totals: BucketTotals,
    ) -> Result<(), FileStorageError> {
        let key = self
            .keyspace
            .object_key(&record.bucket_id, &record.path)
            .map_err(|_| FileStorageError::Storage)?;
        let mut batch = WriteBatch::new();
        batch.delete(&key);
        let mut conditions = vec![KeyCondition::ValueEquals {
            key,
            value: encode(record)?,
        }];
        self.stage_totals(
            &record.bucket_id,
            observed,
            totals,
            &mut conditions,
            &mut batch,
        )?;
        self.apply(conditions, batch).await
    }

    fn stage_totals(
        &self,
        bucket_id: &str,
        observed: &ObservedTotals,
        totals: BucketTotals,
        conditions: &mut Vec<KeyCondition>,
        batch: &mut WriteBatch,
    ) -> Result<(), FileStorageError> {
        let key = self
            .keyspace
            .bucket_totals_key(bucket_id)
            .map_err(|_| FileStorageError::Storage)?;
        conditions.push(match &observed.raw {
            Some(raw) => KeyCondition::ValueEquals {
                key: key.clone(),
                value: raw.clone(),
            },
            None => KeyCondition::Missing { key: key.clone() },
        });
        batch.put(&key, encode(&totals)?);
        Ok(())
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, FileStorageError> {
        self.adapter
            .get(key)
            .await
            .map_err(|_| FileStorageError::Storage)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| FileStorageError::Corrupt))
            .transpose()
    }

    async fn apply(
        &self,
        conditions: Vec<KeyCondition>,
        batch: WriteBatch,
    ) -> Result<(), FileStorageError> {
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions,
                batch,
                durability: self.durability,
            })
            .await
            .map_err(|_| FileStorageError::Storage)?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(FileStorageError::Conflict),
        }
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, FileStorageError> {
    serde_json::to_vec(value).map_err(|_| FileStorageError::Storage)
}
