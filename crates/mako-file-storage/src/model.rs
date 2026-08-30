use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use mako_object_store::valid_bucket_name;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The platform ceiling on one object; a bucket may lower it, never raise it.
pub const MAX_APPLICATION_OBJECT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_OBJECT_PATH_BYTES: usize = 512;
/// How many attributes an object may carry, and how long each part may be.
/// Bounded because a rule reads them on every request and they travel in a
/// header.
pub const MAX_OBJECT_ATTRIBUTES: usize = 8;
pub const MAX_OBJECT_ATTRIBUTE_BYTES: usize = 128;
const MAX_ALLOWED_CONTENT_TYPES: usize = 64;
const MAX_RULES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketAccess {
    /// Every request is evaluated against the bucket's rules.
    Policy,
    /// Reads need no session; every write is still evaluated.
    Public,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectOperation {
    Create,
    Read,
    Update,
    Delete,
}

impl ObjectOperation {
    #[must_use]
    pub const fn as_policy_operation(self) -> mako_policy::DocumentOperation {
        match self {
            Self::Create => mako_policy::DocumentOperation::Create,
            Self::Read => mako_policy::DocumentOperation::Read,
            Self::Update => mako_policy::DocumentOperation::Update,
            Self::Delete => mako_policy::DocumentOperation::Delete,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    Allow,
    Deny,
}

/// One rule in the same language document policies use, over the object
/// document (`new.path`, `new.owner_id`, `old.size_bytes`, ...).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BucketRule {
    pub id: String,
    pub effect: RuleEffect,
    pub operations: Vec<ObjectOperation>,
    pub expression: String,
}

/// What a developer configures; installed into the data plane verbatim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BucketConfig {
    pub id: String,
    pub access: BucketAccess,
    pub max_object_bytes: u64,
    /// `type/subtype`, `type/*`, or `*/*`; empty means any.
    #[serde(default)]
    pub allowed_content_types: Vec<String>,
    #[serde(default)]
    pub rules: Vec<BucketRule>,
}

impl BucketConfig {
    pub fn validate(&self) -> Result<(), FileStorageError> {
        if !valid_bucket_name(&self.id) {
            return Err(FileStorageError::InvalidBucket(
                "bucket id must be 2-63 lowercase letters, digits, or single hyphens",
            ));
        }
        if self.max_object_bytes == 0 || self.max_object_bytes > MAX_APPLICATION_OBJECT_BYTES {
            return Err(FileStorageError::InvalidBucket(
                "maximum object size must be between 1 byte and the platform ceiling",
            ));
        }
        if self.allowed_content_types.len() > MAX_ALLOWED_CONTENT_TYPES {
            return Err(FileStorageError::InvalidBucket(
                "too many allowed content types",
            ));
        }
        for pattern in &self.allowed_content_types {
            if !valid_content_type_pattern(pattern) {
                return Err(FileStorageError::InvalidBucket(
                    "allowed content types are type/subtype, type/*, or */*",
                ));
            }
        }
        if self.rules.len() > MAX_RULES {
            return Err(FileStorageError::InvalidBucket("too many rules"));
        }
        let mut ids = BTreeSet::new();
        for rule in &self.rules {
            if rule.id.is_empty() || rule.id.len() > 64 || !ids.insert(rule.id.as_str()) {
                return Err(FileStorageError::InvalidBucket(
                    "rule ids must be unique and 1-64 characters",
                ));
            }
            if rule.operations.is_empty() {
                return Err(FileStorageError::InvalidBucket(
                    "a rule needs at least one operation",
                ));
            }
            if rule.expression.trim().is_empty() || rule.expression.len() > 16 * 1024 {
                return Err(FileStorageError::InvalidBucket(
                    "a rule expression must be 1-16384 characters",
                ));
            }
        }
        Ok(())
    }
}

fn valid_content_type_pattern(pattern: &str) -> bool {
    let Some((kind, subtype)) = pattern.split_once('/') else {
        return false;
    };
    let token = |value: &str| {
        !value.is_empty()
            && value.len() <= 127
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
    };
    (kind == "*" && subtype == "*") || (token(kind) && (subtype == "*" || token(subtype)))
}

/// Whether a request's content type is admitted by the bucket's patterns.
#[must_use]
pub fn content_type_allowed(allowed: &[String], content_type: &str) -> bool {
    let Some((kind, subtype)) = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .split_once('/')
    else {
        return false;
    };
    let kind = kind.trim().to_ascii_lowercase();
    let subtype = subtype.trim().to_ascii_lowercase();
    if kind.is_empty() || subtype.is_empty() {
        return false;
    }
    allowed.is_empty()
        || allowed.iter().any(|pattern| {
            let pattern = pattern.to_ascii_lowercase();
            let (want_kind, want_subtype) = pattern.split_once('/').unwrap_or(("", ""));
            (want_kind == "*" || want_kind == kind)
                && (want_subtype == "*" || want_subtype == subtype)
        })
}

/// A bucket as stored, with the version that rises on every configuration change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BucketRecord {
    pub config: BucketConfig,
    pub version: u64,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
}

/// The running count and byte total of a bucket, kept with every object write.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BucketTotals {
    pub object_count: u64,
    pub total_bytes: u64,
}

/// What is known about an object without touching its bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObjectRecord {
    pub bucket_id: String,
    pub path: String,
    pub content_type: String,
    /// Plaintext size; what the application stored and is charged for.
    pub size_bytes: u64,
    pub owner_id: Option<String>,
    /// `sha256:` of the plaintext; what a client can verify.
    pub digest: String,
    /// `sha256:` of what the object store holds; the store verifies this one.
    pub stored_digest: String,
    pub created_at_unix_seconds: u64,
    pub updated_at_unix_seconds: u64,
    /// Application-chosen strings a bucket rule may read, set when the object
    /// was stored. A rule verifies one against the caller's trusted claims --
    /// `claims.households[new.attributes.household_id] != null` -- so a false
    /// attribute names a household the caller is not in and grants nothing.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl ObjectRecord {
    /// The synthetic document policies are evaluated against.
    #[must_use]
    pub fn policy_document(&self) -> Value {
        json!({
            "path": self.path,
            "bucket": self.bucket_id,
            "owner_id": self.owner_id.clone().unwrap_or_default(),
            "content_type": self.content_type,
            "size_bytes": self.size_bytes,
            "created_at": self.created_at_unix_seconds,
            "updated_at": self.updated_at_unix_seconds,
            "attributes": self.attributes,
        })
    }
}

/// The schema of the object document, so policy expressions resolve
/// `new.owner_id`, `old.size_bytes`, and the rest exactly as for a collection.
#[must_use]
pub fn object_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "bucket": { "type": "string" },
            "owner_id": { "type": "string" },
            "content_type": { "type": "string" },
            "size_bytes": { "type": "integer" },
            "created_at": { "type": "integer" },
            "updated_at": { "type": "integer" },
            // Open on purpose: what an application attaches is its own, so a
            // rule reads `new.attributes.<name>` as a dynamic value rather
            // than the compiler calling it an unknown field.
            "attributes": { "type": "object", "additionalProperties": true },
        },
        "required": ["path", "bucket", "owner_id", "content_type", "size_bytes", "created_at", "updated_at", "attributes"],
        "additionalProperties": false,
    })
}

/// The attributes an object may carry: bounded in count and in the length of
/// each name and value, names shaped like identifiers so a rule can address
/// one, and no control characters, because they travel in a header.
pub fn validate_object_attributes(
    attributes: &BTreeMap<String, String>,
) -> Result<(), FileStorageError> {
    if attributes.len() > MAX_OBJECT_ATTRIBUTES {
        return Err(FileStorageError::InvalidAttributes("too many attributes"));
    }
    for (name, value) in attributes {
        if name.is_empty()
            || name.len() > MAX_OBJECT_ATTRIBUTE_BYTES
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || name.as_bytes()[0].is_ascii_digit()
        {
            return Err(FileStorageError::InvalidAttributes("attribute name"));
        }
        if value.len() > MAX_OBJECT_ATTRIBUTE_BYTES || value.chars().any(char::is_control) {
            return Err(FileStorageError::InvalidAttributes("attribute value"));
        }
    }
    Ok(())
}

/// An object path is a relative, `/`-separated name that cannot leave its bucket.
pub fn validate_object_path(path: &str) -> Result<(), FileStorageError> {
    if path.is_empty() || path.len() > MAX_OBJECT_PATH_BYTES {
        return Err(FileStorageError::InvalidPath("must be 1-512 bytes"));
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err(FileStorageError::InvalidPath(
            "must not start or end with a slash",
        ));
    }
    if path
        .chars()
        .any(|character| character.is_control() || character == '\\')
    {
        return Err(FileStorageError::InvalidPath(
            "contains a control character or backslash",
        ));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(FileStorageError::InvalidPath("contains an empty segment"));
        }
        if segment == "." || segment == ".." {
            return Err(FileStorageError::InvalidPath(
                "must not contain . or .. segments",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileStorageError {
    InvalidBucket(&'static str),
    InvalidPath(&'static str),
    /// The attributes offered with a write are out of bounds or malformed.
    InvalidAttributes(&'static str),
    /// The bucket's rules do not compile against the object document.
    InvalidPolicy(String),
    BucketNotFound,
    BucketNotEmpty,
    ObjectNotFound,
    ObjectTooLarge {
        maximum_bytes: u64,
    },
    ContentTypeNotAllowed,
    /// The plan's ceiling on stored object bytes would be crossed.
    StorageCapExceeded {
        ceiling_bytes: u64,
    },
    /// The bucket's policy refused; the stable decision code names why.
    Denied(String),
    /// A concurrent change won; the caller may retry.
    Conflict,
    /// A write's precondition did not hold against the current object.
    PreconditionFailed,
    /// The stored object does not match its record.
    Corrupt,
    Storage,
    ObjectStore,
}

impl fmt::Display for FileStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBucket(reason) => {
                write!(formatter, "bucket configuration is invalid: {reason}")
            }
            Self::InvalidPath(reason) => write!(formatter, "object path is invalid: {reason}"),
            Self::InvalidAttributes(reason) => {
                write!(formatter, "object attributes are invalid: {reason}")
            }
            Self::InvalidPolicy(reason) => write!(formatter, "bucket policy is invalid: {reason}"),
            Self::BucketNotFound => formatter.write_str("bucket was not found"),
            Self::BucketNotEmpty => formatter.write_str("bucket still holds objects"),
            Self::ObjectNotFound => formatter.write_str("object was not found"),
            Self::ObjectTooLarge { maximum_bytes } => {
                write!(
                    formatter,
                    "object exceeds the bucket's maximum of {maximum_bytes} bytes"
                )
            }
            Self::ContentTypeNotAllowed => formatter.write_str("content type is not allowed"),
            Self::StorageCapExceeded { ceiling_bytes } => {
                write!(
                    formatter,
                    "stored object bytes would exceed the plan's {ceiling_bytes}"
                )
            }
            Self::Denied(code) => write!(formatter, "bucket policy denied the request ({code})"),
            Self::Conflict => formatter.write_str("object changed concurrently"),
            Self::PreconditionFailed => formatter.write_str("object precondition failed"),
            Self::Corrupt => formatter.write_str("stored object failed verification"),
            Self::Storage => formatter.write_str("object metadata storage is unavailable"),
            Self::ObjectStore => formatter.write_str("object storage is unavailable"),
        }
    }
}

impl Error for FileStorageError {}
