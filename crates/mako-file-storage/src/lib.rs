//! Application file storage: buckets and objects an application stores next to
//! its documents, governed by the same policy engine and metered like every
//! other resource.
//!
//! Object bytes live in the platform object store under content-addressed,
//! tenant-prefixed keys and are encrypted with a key derived for the tenant;
//! everything else -- bucket configuration, object metadata, running totals --
//! lives in the environment's keyspace so a listing never touches the store
//! and a total is never a scan.
mod crypto;
mod model;
mod service;
mod store;

pub use crypto::{ObjectCipher, ObjectKeyRoot};
pub use model::{
    BucketAccess, BucketConfig, BucketRecord, BucketRule, BucketTotals, FileStorageError,
    MAX_APPLICATION_OBJECT_BYTES, MAX_OBJECT_PATH_BYTES, ObjectOperation, ObjectRecord, RuleEffect,
    content_type_allowed, object_schema, validate_object_path,
};
pub use service::{
    FileStorageService, ObjectPage, ObjectPrecondition, ObjectPrincipal, ObjectRequest,
    RemovedBucket, StoredObject,
};
pub use store::BucketStore;
