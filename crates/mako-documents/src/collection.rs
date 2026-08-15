use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU64};

use mako_api::CollectionId;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, StorageError, WriteBatch,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    DocumentCodecError, DocumentEngineScopeError, SchemaVersion, ScopedCollectionEngine,
    document::normalize_object,
};

const COLLECTION_METADATA_ENCODING_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CollectionMetadataVersion(NonZeroU64);

impl CollectionMetadataVersion {
    pub fn new(value: u64) -> Result<Self, CollectionMetadataError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(CollectionMetadataError::InvalidField {
                field: "metadata_version",
                reason: "must be positive",
            })
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// The field holding the user-visible RxDB primary key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrimaryKeyDefinition {
    Field {
        field: String,
    },
    Composite {
        key: String,
        fields: Vec<String>,
        separator: String,
    },
}

impl PrimaryKeyDefinition {
    pub fn field(field: impl Into<String>) -> Result<Self, CollectionMetadataError> {
        let definition = Self::Field {
            field: field.into(),
        };
        definition.validate()?;
        Ok(definition)
    }

    pub fn composite(
        key: impl Into<String>,
        fields: impl IntoIterator<Item = impl Into<String>>,
        separator: impl Into<String>,
    ) -> Result<Self, CollectionMetadataError> {
        let definition = Self::Composite {
            key: key.into(),
            fields: fields.into_iter().map(Into::into).collect(),
            separator: separator.into(),
        };
        definition.validate()?;
        Ok(definition)
    }

    #[must_use]
    pub fn key_field(&self) -> &str {
        match self {
            Self::Field { field } => field,
            Self::Composite { key, .. } => key,
        }
    }

    #[must_use]
    pub fn source_fields(&self) -> &[String] {
        match self {
            Self::Field { field } => std::slice::from_ref(field),
            Self::Composite { fields, .. } => fields,
        }
    }

    fn validate(&self) -> Result<(), CollectionMetadataError> {
        match self {
            Self::Field { field } => validate_field_path("primary_key.field", field),
            Self::Composite {
                key,
                fields,
                separator,
            } => {
                validate_field_path("primary_key.key", key)?;
                if fields.len() < 2 {
                    return Err(CollectionMetadataError::InvalidField {
                        field: "primary_key.fields",
                        reason: "a composite primary key needs at least two source fields",
                    });
                }
                let mut unique = BTreeSet::new();
                for field in fields {
                    validate_field_path("primary_key.fields", field)?;
                    if !unique.insert(field.as_str()) {
                        return Err(CollectionMetadataError::InvalidField {
                            field: "primary_key.fields",
                            reason: "source fields must be unique",
                        });
                    }
                }
                if separator.is_empty() || separator.chars().any(char::is_control) {
                    return Err(CollectionMetadataError::InvalidField {
                        field: "primary_key.separator",
                        reason: "must be non-empty and cannot contain control characters",
                    });
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompatibility {
    PendingValidation,
    Compatible,
    MigrationRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionLifecycle {
    Creating,
    Active,
    Suspended,
    Deleting,
}

/// One immutable version of a collection's schema and lifecycle metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionMetadata {
    collection_id: CollectionId,
    metadata_version: CollectionMetadataVersion,
    schema_version: SchemaVersion,
    json_schema: Map<String, Value>,
    primary_key: PrimaryKeyDefinition,
    compatibility: SchemaCompatibility,
    lifecycle: CollectionLifecycle,
}

impl CollectionMetadata {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        collection_id: CollectionId,
        metadata_version: CollectionMetadataVersion,
        schema_version: SchemaVersion,
        json_schema: Value,
        primary_key: PrimaryKeyDefinition,
        compatibility: SchemaCompatibility,
        lifecycle: CollectionLifecycle,
    ) -> Result<Self, CollectionMetadataError> {
        primary_key.validate()?;
        let json_schema = normalize_object(json_schema).map_err(|error| match error {
            DocumentCodecError::BodyMustBeObject => CollectionMetadataError::InvalidField {
                field: "json_schema",
                reason: "must be a JSON object",
            },
            other => CollectionMetadataError::Document(other),
        })?;

        Ok(Self {
            collection_id,
            metadata_version,
            schema_version,
            json_schema,
            primary_key,
            compatibility,
            lifecycle,
        })
    }

    #[must_use]
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    #[must_use]
    pub const fn metadata_version(&self) -> CollectionMetadataVersion {
        self.metadata_version
    }

    #[must_use]
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    #[must_use]
    pub fn json_schema(&self) -> &Map<String, Value> {
        &self.json_schema
    }

    #[must_use]
    pub fn primary_key(&self) -> &PrimaryKeyDefinition {
        &self.primary_key
    }

    #[must_use]
    pub const fn compatibility(&self) -> SchemaCompatibility {
        self.compatibility
    }

    #[must_use]
    pub const fn lifecycle(&self) -> CollectionLifecycle {
        self.lifecycle
    }

    pub fn encode(&self) -> Result<Vec<u8>, CollectionMetadataError> {
        serde_json::to_vec(&CollectionMetadataWire {
            encoding_version: COLLECTION_METADATA_ENCODING_VERSION,
            collection_id: &self.collection_id,
            metadata_version: self.metadata_version.get(),
            schema_version: self.schema_version.get(),
            json_schema: &self.json_schema,
            primary_key: &self.primary_key,
            compatibility: self.compatibility,
            lifecycle: self.lifecycle,
        })
        .map_err(CollectionMetadataError::Json)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CollectionMetadataError> {
        let wire: OwnedCollectionMetadataWire =
            serde_json::from_slice(bytes).map_err(CollectionMetadataError::Json)?;
        if wire.encoding_version != COLLECTION_METADATA_ENCODING_VERSION {
            return Err(CollectionMetadataError::UnsupportedEncodingVersion(
                wire.encoding_version,
            ));
        }

        Self::new(
            wire.collection_id,
            CollectionMetadataVersion::new(wire.metadata_version)?,
            SchemaVersion::new(wire.schema_version).map_err(CollectionMetadataError::Document)?,
            Value::Object(wire.json_schema),
            wire.primary_key,
            wire.compatibility,
            wire.lifecycle,
        )
    }
}

impl ScopedCollectionEngine {
    pub async fn collection_metadata(
        &self,
    ) -> Result<Option<CollectionMetadata>, CollectionMetadataLoadError> {
        let key = self.collection_metadata_key()?;
        let metadata = self
            .adapter
            .get(&key)
            .await?
            .as_deref()
            .map(CollectionMetadata::decode)
            .transpose()?;
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.collection_id() != self.scope().collection_id())
        {
            return Err(CollectionMetadataLoadError::ScopeMismatch);
        }
        Ok(metadata)
    }
}

/// Result of installing collection metadata into the store that serves the
/// collection's document traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionMetadataInstallOutcome {
    /// Metadata was written for a collection that had none.
    Created,
    /// Metadata identical to the request was already present.
    Unchanged,
    /// Older metadata was replaced by a newer metadata version.
    Updated,
}

impl ScopedCollectionEngine {
    /// Durably install collection metadata so this data plane can resolve the
    /// collection for document reads and writes.
    ///
    /// The collection record of origin lives in the control plane's own store,
    /// which this data plane cannot read. Until this metadata exists here,
    /// document traffic for the collection is rejected as not found, so control
    /// plane collection administration must call this before reporting the
    /// collection usable.
    ///
    /// Idempotent on the collection identifier: replaying the same propagation
    /// reports `Unchanged` rather than conflicting, so a retry after a partial
    /// failure converges. A lower or equal metadata version over different
    /// content is refused, so a stale retry cannot roll the collection back.
    pub async fn install_collection_metadata(
        &self,
        metadata: &CollectionMetadata,
        durability: Durability,
    ) -> Result<CollectionMetadataInstallOutcome, CollectionMetadataInstallError> {
        if metadata.collection_id() != self.scope().collection_id() {
            return Err(CollectionMetadataInstallError::ScopeMismatch);
        }
        let key = self.collection_metadata_key()?;
        let encoded = metadata.encode()?;

        let (condition, outcome) = match self.adapter.get(&key).await? {
            None => (
                KeyCondition::Missing { key: key.clone() },
                CollectionMetadataInstallOutcome::Created,
            ),
            Some(current) if current == encoded => {
                return Ok(CollectionMetadataInstallOutcome::Unchanged);
            }
            Some(current) => {
                let installed = CollectionMetadata::decode(&current)?;
                if installed.metadata_version() >= metadata.metadata_version() {
                    return Err(CollectionMetadataInstallError::StaleMetadataVersion);
                }
                (
                    KeyCondition::ValueEquals {
                        key: key.clone(),
                        value: current,
                    },
                    CollectionMetadataInstallOutcome::Updated,
                )
            }
        };

        let mut batch = WriteBatch::new();
        batch.put(key, &encoded);
        let request = AtomicWrite {
            conditions: vec![condition],
            batch,
            durability,
        };
        request.validate()?;
        match self.adapter.compare_and_write(request).await? {
            CompareAndWriteResult::Applied => Ok(outcome),
            CompareAndWriteResult::Conflict { .. } => {
                Err(CollectionMetadataInstallError::ConcurrentModification)
            }
        }
    }
}

#[derive(Debug)]
pub enum CollectionMetadataInstallError {
    Scope(DocumentEngineScopeError),
    ScopeMismatch,
    StaleMetadataVersion,
    ConcurrentModification,
    Storage(StorageError),
    Metadata(CollectionMetadataError),
}

impl fmt::Display for CollectionMetadataInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Scope(_) => "collection metadata scope is invalid",
            Self::ScopeMismatch => "collection metadata does not match its storage scope",
            Self::StaleMetadataVersion => {
                "collection metadata version does not advance the installed version"
            }
            Self::ConcurrentModification => "collection metadata changed during installation",
            Self::Storage(_) => "collection metadata storage is unavailable",
            Self::Metadata(_) => "collection metadata is invalid",
        })
    }
}

impl Error for CollectionMetadataInstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Metadata(error) => Some(error),
            Self::ScopeMismatch | Self::StaleMetadataVersion | Self::ConcurrentModification => None,
        }
    }
}

impl From<DocumentEngineScopeError> for CollectionMetadataInstallError {
    fn from(error: DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<StorageError> for CollectionMetadataInstallError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<CollectionMetadataError> for CollectionMetadataInstallError {
    fn from(error: CollectionMetadataError) -> Self {
        Self::Metadata(error)
    }
}

#[derive(Debug)]
pub enum CollectionMetadataLoadError {
    Scope(DocumentEngineScopeError),
    ScopeMismatch,
    Storage(StorageError),
    Metadata(CollectionMetadataError),
}

impl fmt::Display for CollectionMetadataLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Scope(_) => "collection metadata scope is invalid",
            Self::ScopeMismatch => "collection metadata does not match its storage scope",
            Self::Storage(_) => "collection metadata storage is unavailable",
            Self::Metadata(_) => "collection metadata is invalid",
        })
    }
}

impl Error for CollectionMetadataLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Metadata(error) => Some(error),
            Self::ScopeMismatch => None,
        }
    }
}

impl From<DocumentEngineScopeError> for CollectionMetadataLoadError {
    fn from(error: DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<StorageError> for CollectionMetadataLoadError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<CollectionMetadataError> for CollectionMetadataLoadError {
    fn from(error: CollectionMetadataError) -> Self {
        Self::Metadata(error)
    }
}

#[derive(Serialize)]
struct CollectionMetadataWire<'a> {
    encoding_version: u8,
    collection_id: &'a CollectionId,
    metadata_version: u64,
    schema_version: u64,
    json_schema: &'a Map<String, Value>,
    primary_key: &'a PrimaryKeyDefinition,
    compatibility: SchemaCompatibility,
    lifecycle: CollectionLifecycle,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedCollectionMetadataWire {
    encoding_version: u8,
    collection_id: CollectionId,
    metadata_version: u64,
    schema_version: u64,
    json_schema: Map<String, Value>,
    primary_key: PrimaryKeyDefinition,
    compatibility: SchemaCompatibility,
    lifecycle: CollectionLifecycle,
}

#[derive(Debug)]
pub enum CollectionMetadataError {
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    UnsupportedEncodingVersion(u8),
    Document(DocumentCodecError),
    Json(serde_json::Error),
}

impl fmt::Display for CollectionMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::UnsupportedEncodingVersion(version) => {
                write!(
                    formatter,
                    "unsupported collection metadata encoding version {version}"
                )
            }
            Self::Document(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid collection metadata encoding: {error}"),
        }
    }
}

impl Error for CollectionMetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Document(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_field_path(
    field_name: &'static str,
    value: &str,
) -> Result<(), CollectionMetadataError> {
    let valid = !value.is_empty()
        && value.len() <= 255
        && value.split('.').all(|segment| {
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
                && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
        });
    if valid {
        Ok(())
    } else {
        Err(CollectionMetadataError::InvalidField {
            field: field_name,
            reason: "must be a dotted path of identifier segments",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::executor::block_on;
    use mako_api::{CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::MemoryAdapter;
    use serde_json::json;

    use super::*;
    use crate::DocumentEngine;

    fn metadata(schema: Value) -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection id"),
            CollectionMetadataVersion::new(4).expect("metadata version"),
            SchemaVersion::new(2).expect("schema version"),
            schema,
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
    }

    #[test]
    fn metadata_round_trips_with_deterministic_schema_ordering() {
        let first = metadata(json!({
            "required": ["id"],
            "properties": { "title": { "type": "string" }, "id": { "type": "string" } },
            "type": "object"
        }));
        let second = metadata(json!({
            "type": "object",
            "properties": { "id": { "type": "string" }, "title": { "type": "string" } },
            "required": ["id"]
        }));

        assert_eq!(first, second);
        assert_eq!(
            first.encode().expect("encode"),
            second.encode().expect("encode")
        );
        assert_eq!(
            CollectionMetadata::decode(&first.encode().expect("encode")).expect("decode"),
            first
        );
    }

    #[test]
    fn composite_primary_keys_retain_rxdb_components() {
        let key = PrimaryKeyDefinition::composite("id", ["organization_id", "local_id"], "|")
            .expect("composite key");
        assert_eq!(key.key_field(), "id");
        assert_eq!(key.source_fields(), ["organization_id", "local_id"]);

        assert!(PrimaryKeyDefinition::composite("id", ["only_one"], "|").is_err());
        assert!(PrimaryKeyDefinition::composite("id", ["same", "same"], "|").is_err());
    }

    #[test]
    fn invalid_metadata_versions_and_schema_shapes_are_rejected() {
        assert!(CollectionMetadataVersion::new(0).is_err());
        assert!(PrimaryKeyDefinition::field("not a field").is_err());

        let result = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection id"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!(["not", "a", "schema"]),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::PendingValidation,
            CollectionLifecycle::Creating,
        );
        assert!(matches!(
            result,
            Err(CollectionMetadataError::InvalidField {
                field: "json_schema",
                ..
            })
        ));
    }

    #[test]
    fn decode_validates_identifiers_and_rejects_unknown_fields() {
        let invalid_collection = br#"{"encoding_version":1,"collection_id":"Todos","metadata_version":1,"schema_version":1,"json_schema":{},"primary_key":{"kind":"field","field":"id"},"compatibility":"compatible","lifecycle":"active"}"#;
        assert!(matches!(
            CollectionMetadata::decode(invalid_collection),
            Err(CollectionMetadataError::Json(_))
        ));

        let unknown = br#"{"encoding_version":1,"collection_id":"todos","metadata_version":1,"schema_version":1,"json_schema":{},"primary_key":{"kind":"field","field":"id"},"compatibility":"compatible","lifecycle":"active","extra":true}"#;
        assert!(matches!(
            CollectionMetadata::decode(unknown),
            Err(CollectionMetadataError::Json(_))
        ));
    }

    fn scoped(collection: &str) -> ScopedCollectionEngine {
        let adapter = Arc::new(MemoryAdapter::new());
        let engine = DocumentEngine::new(adapter);
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        engine
            .scope_collection(
                &tenant,
                CollectionScope::new(
                    tenant.clone(),
                    CollectionId::parse(collection).expect("collection"),
                ),
            )
            .expect("scope")
    }

    fn versioned(version: u64, schema: Value) -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection id"),
            CollectionMetadataVersion::new(version).expect("metadata version"),
            SchemaVersion::new(2).expect("schema version"),
            schema,
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "string" } }
        })
    }

    #[test]
    fn installed_metadata_becomes_resolvable_and_replays_as_unchanged() {
        let scoped = scoped("todos");
        let metadata = metadata(schema());

        let created = block_on(scoped.install_collection_metadata(&metadata, Durability::Memory))
            .expect("install");
        assert_eq!(created, CollectionMetadataInstallOutcome::Created);

        // Before installation the collection cannot be resolved for document
        // traffic; after it, it can. This is the propagation the control plane
        // depends on.
        let loaded = block_on(scoped.collection_metadata()).expect("load");
        assert_eq!(
            loaded.as_ref().map(CollectionMetadata::collection_id),
            Some(metadata.collection_id())
        );

        let replayed = block_on(scoped.install_collection_metadata(&metadata, Durability::Memory))
            .expect("replay");
        assert_eq!(replayed, CollectionMetadataInstallOutcome::Unchanged);
    }

    #[test]
    fn newer_metadata_version_replaces_older_and_stale_version_is_refused() {
        let scoped = scoped("todos");
        block_on(scoped.install_collection_metadata(&versioned(4, schema()), Durability::Memory))
            .expect("install");

        let advanced = json!({
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "string" }, "title": { "type": "string" } }
        });
        let updated = block_on(
            scoped.install_collection_metadata(&versioned(5, advanced), Durability::Memory),
        )
        .expect("update");
        assert_eq!(updated, CollectionMetadataInstallOutcome::Updated);

        let stale = block_on(scoped.install_collection_metadata(
            &versioned(
                3,
                json!({"type": "object", "properties": { "id": { "type": "string" } }}),
            ),
            Durability::Memory,
        ));
        assert!(matches!(
            stale,
            Err(CollectionMetadataInstallError::StaleMetadataVersion)
        ));
    }

    #[test]
    fn metadata_for_another_collection_is_refused() {
        let scoped = scoped("orders");
        let result =
            block_on(scoped.install_collection_metadata(&metadata(schema()), Durability::Memory));
        assert!(matches!(
            result,
            Err(CollectionMetadataInstallError::ScopeMismatch)
        ));
    }
}
