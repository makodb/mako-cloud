use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU64};

use mako_api::CollectionId;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, StorageError, TenantKeyspace,
    WriteBatch,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{CanonicalDocument, DocumentEngineScopeError, ScopedCollectionEngine};

const INDEX_CATALOG_ENCODING_VERSION: u8 = 1;
const INDEX_VALUE_MISSING: u8 = 0x10;
const INDEX_VALUE_NULL: u8 = 0x20;
const INDEX_VALUE_FALSE: u8 = 0x30;
const INDEX_VALUE_TRUE: u8 = 0x31;
const INDEX_VALUE_NUMBER: u8 = 0x40;
const INDEX_VALUE_STRING: u8 = 0x50;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct IndexName(String);

impl IndexName {
    pub fn parse(value: impl Into<String>) -> Result<Self, IndexError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value.chars().all(|character| {
                character == '-' || character == '_' || character.is_ascii_alphanumeric()
            });
        if !valid {
            return Err(IndexError::InvalidDefinition {
                field: "name",
                reason: "must contain 1-128 ASCII letters, digits, hyphens, or underscores",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndexVersion(NonZeroU64);

impl IndexVersion {
    pub fn new(value: u64) -> Result<Self, IndexError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(IndexError::InvalidDefinition {
                field: "version",
                reason: "must be positive",
            })
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexKind {
    NonUnique,
    Unique,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexDirection {
    Ascending,
    Descending,
}

/// A supported index scalar. Missing is deliberately distinct from JSON null.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexValue {
    Missing,
    Null,
    Bool(bool),
    Number(IndexNumber),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexNumber(String);

impl IndexNumber {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl IndexValue {
    pub fn from_json(value: &Value) -> Result<Self, IndexError> {
        match value {
            Value::Null => Ok(Self::Null),
            Value::Bool(value) => Ok(Self::Bool(*value)),
            Value::Number(value) => {
                let encoded = value.to_string();
                normalize_number(&encoded)?;
                Ok(Self::Number(IndexNumber(encoded)))
            }
            Value::String(value) => Ok(Self::String(value.clone())),
            Value::Array(_) | Value::Object(_) => Err(IndexError::UnsupportedIndexValue),
        }
    }

    pub fn number(value: impl Into<String>) -> Result<Self, IndexError> {
        let value = value.into();
        normalize_number(&value)?;
        Ok(Self::Number(IndexNumber(value)))
    }

    #[must_use]
    pub fn encode(&self, direction: IndexDirection) -> Vec<u8> {
        let mut encoded = match self {
            Self::Missing => vec![INDEX_VALUE_MISSING, 0],
            Self::Null => vec![INDEX_VALUE_NULL, 0],
            Self::Bool(false) => vec![INDEX_VALUE_FALSE, 0],
            Self::Bool(true) => vec![INDEX_VALUE_TRUE, 0],
            Self::Number(value) => encode_number(value.as_str()),
            Self::String(value) => {
                let mut encoded = Vec::with_capacity(value.len() + 2);
                encoded.push(INDEX_VALUE_STRING);
                encoded.extend_from_slice(value.as_bytes());
                encoded.push(0);
                encoded
            }
        };
        if direction == IndexDirection::Descending {
            for byte in &mut encoded {
                *byte = !*byte;
            }
        }
        encoded
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexField {
    path: String,
    direction: IndexDirection,
}

impl IndexField {
    pub fn new(path: impl Into<String>, direction: IndexDirection) -> Result<Self, IndexError> {
        let path = path.into();
        validate_field_path(&path)?;
        Ok(Self { path, direction })
    }

    pub fn ascending(path: impl Into<String>) -> Result<Self, IndexError> {
        Self::new(path, IndexDirection::Ascending)
    }

    pub fn descending(path: impl Into<String>) -> Result<Self, IndexError> {
        Self::new(path, IndexDirection::Descending)
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn direction(&self) -> IndexDirection {
        self.direction
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    Building,
    Active,
    Failed,
    Deleting,
}

/// The longest encoded value one indexed field may have: an index key holds
/// each field's value as one segment, and a key segment is at most 16 KiB.
pub const MAX_INDEXED_VALUE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexFailureCode {
    DuplicateValues,
    BackfillFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexFailure {
    code: IndexFailureCode,
    affected_values: u64,
    safe_message: String,
}

impl IndexFailure {
    pub fn new(
        code: IndexFailureCode,
        affected_values: u64,
        safe_message: impl Into<String>,
    ) -> Result<Self, IndexError> {
        let safe_message = safe_message.into();
        if safe_message.is_empty()
            || safe_message.len() > 256
            || safe_message.chars().any(char::is_control)
        {
            return Err(IndexError::InvalidDefinition {
                field: "failure.safe_message",
                reason: "must contain 1-256 non-control characters",
            });
        }
        Ok(Self {
            code,
            affected_values,
            safe_message,
        })
    }

    #[must_use]
    pub const fn code(&self) -> IndexFailureCode {
        self.code
    }

    #[must_use]
    pub const fn affected_values(&self) -> u64 {
        self.affected_values
    }

    #[must_use]
    pub fn safe_message(&self) -> &str {
        &self.safe_message
    }
}

/// One immutable index shape/version plus its mutable online-build lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexDefinition {
    collection_id: CollectionId,
    name: IndexName,
    version: IndexVersion,
    kind: IndexKind,
    fields: Vec<IndexField>,
    state: IndexState,
    activation_fenced: bool,
    failure: Option<IndexFailure>,
}

impl IndexDefinition {
    pub fn new_building(
        collection_id: CollectionId,
        name: IndexName,
        version: IndexVersion,
        kind: IndexKind,
        fields: impl IntoIterator<Item = IndexField>,
    ) -> Result<Self, IndexError> {
        Self::new(
            collection_id,
            name,
            version,
            kind,
            fields,
            IndexState::Building,
            false,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        collection_id: CollectionId,
        name: IndexName,
        version: IndexVersion,
        kind: IndexKind,
        fields: impl IntoIterator<Item = IndexField>,
        state: IndexState,
        activation_fenced: bool,
        failure: Option<IndexFailure>,
    ) -> Result<Self, IndexError> {
        let fields: Vec<_> = fields.into_iter().collect();
        if fields.is_empty() {
            return Err(IndexError::InvalidDefinition {
                field: "fields",
                reason: "at least one field is required",
            });
        }
        let mut unique = BTreeSet::new();
        if fields.iter().any(|field| !unique.insert(field.path())) {
            return Err(IndexError::InvalidDefinition {
                field: "fields",
                reason: "field paths must be unique",
            });
        }
        if activation_fenced && state != IndexState::Building {
            return Err(IndexError::InvalidDefinition {
                field: "activation_fenced",
                reason: "is only valid while an index is building",
            });
        }
        if (state == IndexState::Failed) != failure.is_some() {
            return Err(IndexError::InvalidDefinition {
                field: "failure",
                reason: "must be present exactly when the index state is failed",
            });
        }
        Ok(Self {
            collection_id,
            name,
            version,
            kind,
            fields,
            state,
            activation_fenced,
            failure,
        })
    }

    #[must_use]
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    #[must_use]
    pub fn name(&self) -> &IndexName {
        &self.name
    }

    #[must_use]
    pub const fn version(&self) -> IndexVersion {
        self.version
    }

    #[must_use]
    pub const fn kind(&self) -> IndexKind {
        self.kind
    }

    #[must_use]
    pub fn fields(&self) -> &[IndexField] {
        &self.fields
    }

    #[must_use]
    pub const fn state(&self) -> IndexState {
        self.state
    }

    #[must_use]
    pub const fn activation_fenced(&self) -> bool {
        self.activation_fenced
    }

    #[must_use]
    pub fn failure(&self) -> Option<&IndexFailure> {
        self.failure.as_ref()
    }

    #[must_use]
    pub fn physical_name(&self) -> String {
        format!("{}@{}", self.name.as_str(), self.version.get())
    }

    pub fn values_for_body(
        &self,
        body: &Map<String, Value>,
    ) -> Result<Vec<IndexValue>, IndexError> {
        self.fields
            .iter()
            .map(|field| match value_at_path(body, field.path()) {
                Some(value) => IndexValue::from_json(value),
                None => Ok(IndexValue::Missing),
            })
            .collect()
    }

    pub fn encoded_components(
        &self,
        body: &Map<String, Value>,
    ) -> Result<Vec<Vec<u8>>, IndexError> {
        self.values_for_body(body)?
            .into_iter()
            .zip(&self.fields)
            .map(|(value, field)| {
                let encoded = value.encode(field.direction());
                // Each component is one key segment; a longer one used to
                // surface as a storage error, a 503 on every write.
                if encoded.len() > MAX_INDEXED_VALUE_BYTES {
                    return Err(IndexError::IndexedValueTooLarge);
                }
                Ok(encoded)
            })
            .collect()
    }

    pub(crate) fn entry_key(
        &self,
        keyspace: &TenantKeyspace,
        document: &CanonicalDocument,
    ) -> Result<Vec<u8>, IndexError> {
        let components = self.encoded_components(document.body())?;
        keyspace
            .index_entry_key(
                self.collection_id.as_str(),
                self.physical_name(),
                &components,
                document.primary_key().as_str(),
            )
            .map_err(Into::into)
    }

    pub(crate) fn owner_key(
        &self,
        keyspace: &TenantKeyspace,
        body: &Map<String, Value>,
    ) -> Result<Vec<u8>, IndexError> {
        let components = self.encoded_components(body)?;
        keyspace
            .unique_index_owner_key(
                self.collection_id.as_str(),
                self.physical_name(),
                &components,
            )
            .map_err(Into::into)
    }

    #[must_use]
    pub(crate) fn is_maintained_by_writes(&self) -> bool {
        matches!(self.state, IndexState::Building | IndexState::Active)
    }

    #[must_use]
    pub(crate) fn enforces_uniqueness(&self) -> bool {
        self.kind == IndexKind::Unique
            && (self.state == IndexState::Active || self.activation_fenced)
    }

    pub(crate) fn set_activation_fenced(&mut self) -> Result<(), IndexError> {
        if self.state != IndexState::Building {
            return Err(IndexError::InvalidStateTransition {
                from: self.state,
                to: IndexState::Building,
            });
        }
        self.activation_fenced = true;
        Ok(())
    }

    pub(crate) fn activate(&mut self) -> Result<(), IndexError> {
        if self.state != IndexState::Building {
            return Err(IndexError::InvalidStateTransition {
                from: self.state,
                to: IndexState::Active,
            });
        }
        self.state = IndexState::Active;
        self.activation_fenced = false;
        self.failure = None;
        Ok(())
    }

    pub(crate) fn fail(&mut self, failure: IndexFailure) -> Result<(), IndexError> {
        if self.state != IndexState::Building {
            return Err(IndexError::InvalidStateTransition {
                from: self.state,
                to: IndexState::Failed,
            });
        }
        self.state = IndexState::Failed;
        self.activation_fenced = false;
        self.failure = Some(failure);
        Ok(())
    }

    pub(crate) fn start_deleting(&mut self) -> Result<(), IndexError> {
        if self.state == IndexState::Deleting {
            return Ok(());
        }
        self.state = IndexState::Deleting;
        self.activation_fenced = false;
        self.failure = None;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct IndexCatalog {
    definitions: Vec<IndexDefinition>,
}

impl IndexCatalog {
    #[must_use]
    pub(crate) fn definitions(&self) -> &[IndexDefinition] {
        &self.definitions
    }

    pub(crate) fn definition_mut(
        &mut self,
        name: &IndexName,
        version: IndexVersion,
    ) -> Option<&mut IndexDefinition> {
        self.definitions
            .iter_mut()
            .find(|definition| definition.name == *name && definition.version == version)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, IndexError> {
        let definitions: Vec<_> = self
            .definitions
            .iter()
            .map(IndexDefinitionWire::from)
            .collect();
        serde_json::to_vec(&IndexCatalogWire {
            encoding_version: INDEX_CATALOG_ENCODING_VERSION,
            definitions,
        })
        .map_err(IndexError::Json)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, IndexError> {
        let wire: IndexCatalogWire = serde_json::from_slice(bytes).map_err(IndexError::Json)?;
        if wire.encoding_version != INDEX_CATALOG_ENCODING_VERSION {
            return Err(IndexError::UnsupportedCatalogEncoding(
                wire.encoding_version,
            ));
        }
        let mut definitions = Vec::with_capacity(wire.definitions.len());
        let mut identities = BTreeSet::new();
        for definition in wire.definitions {
            let definition = definition.try_into_definition()?;
            if !identities.insert((definition.name.clone(), definition.version)) {
                return Err(IndexError::DuplicateDefinition);
            }
            definitions.push(definition);
        }
        definitions
            .sort_by(|left, right| (&left.name, left.version).cmp(&(&right.name, right.version)));
        Ok(Self { definitions })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IndexCatalogWire {
    encoding_version: u8,
    definitions: Vec<IndexDefinitionWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IndexDefinitionWire {
    collection_id: CollectionId,
    name: IndexName,
    version: u64,
    kind: IndexKind,
    fields: Vec<IndexField>,
    state: IndexState,
    activation_fenced: bool,
    failure: Option<IndexFailure>,
}

impl From<&IndexDefinition> for IndexDefinitionWire {
    fn from(definition: &IndexDefinition) -> Self {
        Self {
            collection_id: definition.collection_id.clone(),
            name: definition.name.clone(),
            version: definition.version.get(),
            kind: definition.kind,
            fields: definition.fields.clone(),
            state: definition.state,
            activation_fenced: definition.activation_fenced,
            failure: definition.failure.clone(),
        }
    }
}

impl IndexDefinitionWire {
    fn try_into_definition(self) -> Result<IndexDefinition, IndexError> {
        IndexDefinition::new(
            self.collection_id,
            self.name,
            IndexVersion::new(self.version)?,
            self.kind,
            self.fields,
            self.state,
            self.activation_fenced,
            self.failure,
        )
    }
}

impl ScopedCollectionEngine {
    pub async fn create_index(
        &self,
        definition: IndexDefinition,
        durability: Durability,
    ) -> Result<(), IndexError> {
        self.ensure_index_durability(durability)?;
        if definition.collection_id() != self.scope().collection_id() {
            return Err(IndexError::CollectionMismatch);
        }
        if definition.state() != IndexState::Building {
            return Err(IndexError::InvalidDefinition {
                field: "state",
                reason: "new indexes must start in building state",
            });
        }
        let key = self.index_catalog_key()?;
        loop {
            let stored = self.adapter.get(&key).await?;
            let mut catalog = decode_catalog(stored.as_deref())?;
            if catalog.definitions.iter().any(|existing| {
                existing.name == definition.name && existing.version == definition.version
            }) {
                return Err(IndexError::DefinitionAlreadyExists);
            }
            catalog.definitions.push(definition.clone());
            catalog.definitions.sort_by(|left, right| {
                (&left.name, left.version).cmp(&(&right.name, right.version))
            });
            let encoded = catalog.encode()?;
            let condition = catalog_condition(&key, stored);
            let mut batch = WriteBatch::new();
            batch.put(&key, encoded);
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(());
            }
        }
    }

    pub async fn index_definitions(&self) -> Result<Vec<IndexDefinition>, IndexError> {
        let key = self.index_catalog_key()?;
        Ok(decode_catalog(self.adapter.get(&key).await?.as_deref())?.definitions)
    }

    pub async fn index_definition(
        &self,
        name: &IndexName,
        version: IndexVersion,
    ) -> Result<Option<IndexDefinition>, IndexError> {
        Ok(self
            .index_definitions()
            .await?
            .into_iter()
            .find(|definition| definition.name == *name && definition.version == version))
    }

    pub async fn mark_index_deleting(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
    ) -> Result<(), IndexError> {
        self.update_index_catalog(name, version, durability, |definition| {
            definition.start_deleting()
        })
        .await
    }

    /// Drops a deleted index's definition from the catalog, once
    /// `remove_index` has taken its entries away. Until then the definition
    /// stays as `Deleting`, which is what keeps queries and writes off it; a
    /// write racing the removal is conditioned on the catalog and retries
    /// against the final one. Forgetting a definition that is not deleting is
    /// refused, and forgetting one that is already gone succeeds, so a
    /// retried removal converges.
    pub async fn forget_index(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
    ) -> Result<(), IndexError> {
        self.ensure_index_durability(durability)?;
        let key = self.index_catalog_key()?;
        loop {
            let stored = self.adapter.get(&key).await?;
            let mut catalog = decode_catalog(stored.as_deref())?;
            let Some(position) = catalog
                .definitions
                .iter()
                .position(|definition| definition.name == *name && definition.version == version)
            else {
                return Ok(());
            };
            let state = catalog.definitions[position].state;
            if state != IndexState::Deleting {
                return Err(IndexError::InvalidStateTransition {
                    from: state,
                    to: IndexState::Deleting,
                });
            }
            catalog.definitions.remove(position);
            let mut batch = WriteBatch::new();
            batch.put(&key, catalog.encode()?);
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![catalog_condition(&key, stored)],
                    batch,
                    durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(());
            }
        }
    }

    pub(crate) fn index_catalog_key(&self) -> Result<Vec<u8>, IndexError> {
        self.keyspace
            .index_catalog_key(self.scope().collection_id().as_str())
            .map_err(Into::into)
    }

    pub(crate) async fn load_index_catalog(
        &self,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>, IndexCatalog), IndexError> {
        let key = self.index_catalog_key()?;
        let stored = self.adapter.get(&key).await?;
        let catalog = decode_catalog(stored.as_deref())?;
        Ok((key, stored, catalog))
    }

    pub(crate) async fn update_index_catalog(
        &self,
        name: &IndexName,
        version: IndexVersion,
        durability: Durability,
        update: impl Fn(&mut IndexDefinition) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.ensure_index_durability(durability)?;
        let key = self.index_catalog_key()?;
        loop {
            let stored = self.adapter.get(&key).await?;
            let mut catalog = decode_catalog(stored.as_deref())?;
            let definition = catalog
                .definition_mut(name, version)
                .ok_or(IndexError::DefinitionNotFound)?;
            update(definition)?;
            let encoded = catalog.encode()?;
            let condition = catalog_condition(&key, stored);
            let mut batch = WriteBatch::new();
            batch.put(&key, encoded);
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![condition],
                    batch,
                    durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(());
            }
        }
    }

    fn ensure_index_durability(&self, durability: Durability) -> Result<(), IndexError> {
        if self.adapter.capabilities().strongest_durability < durability {
            Err(IndexError::UnsupportedDurability(durability))
        } else {
            Ok(())
        }
    }
}

fn decode_catalog(stored: Option<&[u8]>) -> Result<IndexCatalog, IndexError> {
    stored
        .map(IndexCatalog::decode)
        .transpose()
        .map(Option::unwrap_or_default)
}

fn catalog_condition(key: &[u8], stored: Option<Vec<u8>>) -> KeyCondition {
    match stored {
        Some(value) => KeyCondition::ValueEquals {
            key: key.to_vec(),
            value,
        },
        None => KeyCondition::Missing { key: key.to_vec() },
    }
}

fn validate_field_path(value: &str) -> Result<(), IndexError> {
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
        Err(IndexError::InvalidDefinition {
            field: "fields.path",
            reason: "must be a dotted path of identifier segments",
        })
    }
}

fn value_at_path<'a>(body: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut value = body.get(first)?;
    for segment in segments {
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

fn encode_number(value: &str) -> Vec<u8> {
    let normalized = normalize_number(value).expect("validated index number");
    let mut encoded = vec![INDEX_VALUE_NUMBER];
    match normalized {
        NormalizedNumber::Zero => encoded.push(0x01),
        NormalizedNumber::Magnitude {
            negative,
            exponent,
            digits,
        } => {
            encoded.push(if negative { 0x00 } else { 0x02 });
            let mut magnitude = ((exponent as u32) ^ 0x8000_0000).to_be_bytes().to_vec();
            magnitude.extend_from_slice(digits.as_bytes());
            magnitude.push(0);
            if negative {
                for byte in &mut magnitude {
                    *byte = !*byte;
                }
            }
            encoded.extend(magnitude);
        }
    }
    encoded.push(0);
    encoded
}

#[derive(Debug, Eq, PartialEq)]
enum NormalizedNumber {
    Zero,
    Magnitude {
        negative: bool,
        exponent: i32,
        digits: String,
    },
}

fn normalize_number(value: &str) -> Result<NormalizedNumber, IndexError> {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let (mantissa, explicit_exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let exponent = exponent
                .parse::<i64>()
                .map_err(|_| IndexError::InvalidNumber)?;
            (mantissa, exponent)
        }
        None => (unsigned, 0),
    };
    let (whole, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |parts| parts);
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(IndexError::InvalidNumber);
    }

    let mut digits = format!("{whole}{fraction}");
    let leading = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading);
    if digits.is_empty() {
        return Ok(NormalizedNumber::Zero);
    }
    let trailing = digits
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'0')
        .count();
    digits.truncate(digits.len() - trailing);
    let scale = explicit_exponent
        .checked_sub(i64::try_from(fraction.len()).map_err(|_| IndexError::InvalidNumber)?)
        .and_then(|scale| scale.checked_add(i64::try_from(trailing).ok()?))
        .ok_or(IndexError::InvalidNumber)?;
    let exponent = i64::try_from(digits.len())
        .map_err(|_| IndexError::InvalidNumber)?
        .checked_add(scale)
        .and_then(|value| value.checked_sub(1))
        .and_then(|value| i32::try_from(value).ok())
        .ok_or(IndexError::InvalidNumber)?;
    Ok(NormalizedNumber::Magnitude {
        negative,
        exponent,
        digits,
    })
}

#[derive(Debug)]
pub enum IndexError {
    /// One indexed value is longer than an index key can hold.
    IndexedValueTooLarge,
    InvalidDefinition {
        field: &'static str,
        reason: &'static str,
    },
    InvalidStateTransition {
        from: IndexState,
        to: IndexState,
    },
    CollectionMismatch,
    DuplicateDefinition,
    DefinitionAlreadyExists,
    DefinitionNotFound,
    UnsupportedIndexValue,
    InvalidNumber,
    UnsupportedCatalogEncoding(u8),
    UnsupportedDurability(Durability),
    Scope(DocumentEngineScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for IndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexedValueTooLarge => write!(
                formatter,
                "an indexed value is longer than {} bytes",
                MAX_INDEXED_VALUE_BYTES
            ),
            Self::InvalidDefinition { field, reason } => {
                write!(formatter, "invalid index {field}: {reason}")
            }
            Self::InvalidStateTransition { from, to } => {
                write!(formatter, "cannot transition index from {from:?} to {to:?}")
            }
            Self::CollectionMismatch => {
                formatter.write_str("index collection does not match the scoped collection")
            }
            Self::DuplicateDefinition => {
                formatter.write_str("index catalog contains a duplicate name and version")
            }
            Self::DefinitionAlreadyExists => {
                formatter.write_str("index name and version already exist")
            }
            Self::DefinitionNotFound => formatter.write_str("index definition was not found"),
            Self::UnsupportedIndexValue => {
                formatter.write_str("index fields support only missing or scalar JSON values")
            }
            Self::InvalidNumber => formatter.write_str("invalid JSON number for index encoding"),
            Self::UnsupportedCatalogEncoding(version) => {
                write!(
                    formatter,
                    "unsupported index catalog encoding version {version}"
                )
            }
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid index metadata encoding: {error}"),
        }
    }
}

impl Error for IndexError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DocumentEngineScopeError> for IndexError {
    fn from(error: DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for IndexError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for IndexError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
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

    #[test]
    fn compound_versioned_definitions_round_trip_through_catalog() {
        let definition = definition(IndexKind::Unique);
        let catalog = IndexCatalog {
            definitions: vec![definition.clone()],
        };
        let decoded = IndexCatalog::decode(&catalog.encode().expect("encode")).expect("decode");

        assert_eq!(decoded.definitions(), &[definition]);
        assert_eq!(decoded.definitions()[0].fields().len(), 2);
        assert_eq!(decoded.definitions()[0].state(), IndexState::Building);
    }

    #[test]
    fn scoped_catalog_persists_and_transitions_to_deleting() {
        block_on(async {
            let scoped = scoped();
            let definition = definition(IndexKind::NonUnique);
            scoped
                .create_index(definition.clone(), Durability::Memory)
                .await
                .expect("create");
            assert_eq!(
                scoped.index_definitions().await.expect("list"),
                vec![definition]
            );

            scoped
                .mark_index_deleting(
                    &IndexName::parse("by-team-created").expect("name"),
                    IndexVersion::new(3).expect("version"),
                    Durability::Memory,
                )
                .await
                .expect("delete transition");
            assert_eq!(
                scoped.index_definitions().await.expect("list")[0].state(),
                IndexState::Deleting
            );
        });
    }

    #[test]
    fn invalid_or_duplicate_fields_are_rejected() {
        assert!(IndexField::ascending("bad path").is_err());
        let result = IndexDefinition::new_building(
            CollectionId::parse("todos").expect("collection"),
            IndexName::parse("duplicate").expect("name"),
            IndexVersion::new(1).expect("version"),
            IndexKind::NonUnique,
            [
                IndexField::ascending("team").expect("field"),
                IndexField::descending("team").expect("field"),
            ],
        );
        assert!(matches!(result, Err(IndexError::InvalidDefinition { .. })));
    }

    #[test]
    fn scalar_encoding_orders_types_numbers_strings_and_descending_values() {
        let ascending = [
            IndexValue::Missing,
            IndexValue::Null,
            IndexValue::Bool(false),
            IndexValue::Bool(true),
            IndexValue::number("-12.5").expect("number"),
            IndexValue::number("-1").expect("number"),
            IndexValue::number("0").expect("number"),
            IndexValue::number("1.0").expect("number"),
            IndexValue::number("1.01").expect("number"),
            IndexValue::number("12.5").expect("number"),
            IndexValue::String("a".to_owned()),
            IndexValue::String("aa".to_owned()),
        ]
        .map(|value| value.encode(IndexDirection::Ascending));
        assert!(ascending.windows(2).all(|pair| pair[0] < pair[1]));

        let descending = [
            IndexValue::String("aa".to_owned()),
            IndexValue::String("a".to_owned()),
            IndexValue::number("12.5").expect("number"),
            IndexValue::number("1.01").expect("number"),
            IndexValue::number("1").expect("number"),
        ]
        .map(|value| value.encode(IndexDirection::Descending));
        assert!(descending.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            IndexValue::number("1.0")
                .expect("number")
                .encode(IndexDirection::Ascending),
            IndexValue::number("1")
                .expect("number")
                .encode(IndexDirection::Ascending)
        );
    }

    #[test]
    fn compound_keys_include_missing_values_and_primary_key_tie_breakers() {
        let definition = definition(IndexKind::Unique);
        let keyspace =
            TenantKeyspace::new(b"project".to_vec(), b"environment".to_vec()).expect("keyspace");
        let body = json!({"id": "doc-a", "team": "blue"})
            .as_object()
            .cloned()
            .expect("object");
        let components = definition.encoded_components(&body).expect("components");
        assert_eq!(components.len(), 2);
        assert_eq!(
            components[1],
            IndexValue::Missing.encode(IndexDirection::Descending)
        );
        let first = keyspace
            .index_entry_key("todos", definition.physical_name(), &components, "doc-a")
            .expect("entry");
        let second = keyspace
            .index_entry_key("todos", definition.physical_name(), &components, "doc-b")
            .expect("entry");
        let owner = keyspace
            .unique_index_owner_key("todos", definition.physical_name(), &components)
            .expect("owner");
        assert!(first < second);
        assert_ne!(owner, first);
        assert!(
            keyspace
                .index_range("todos", definition.physical_name())
                .expect("range")
                .contains(&first)
        );
        assert!(
            !keyspace
                .index_range("todos", definition.physical_name())
                .expect("range")
                .contains(&owner)
        );
    }

    fn definition(kind: IndexKind) -> IndexDefinition {
        IndexDefinition::new_building(
            CollectionId::parse("todos").expect("collection"),
            IndexName::parse("by-team-created").expect("name"),
            IndexVersion::new(3).expect("version"),
            kind,
            [
                IndexField::ascending("team").expect("field"),
                IndexField::descending("created_at").expect("field"),
            ],
        )
        .expect("definition")
    }

    fn scoped() -> ScopedCollectionEngine {
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
                    CollectionId::parse("todos").expect("collection"),
                ),
            )
            .expect("scope")
    }
}
