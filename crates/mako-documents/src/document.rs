use std::{error::Error, fmt, num::NonZeroU64};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const DOCUMENT_ENCODING_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DocumentId(String);

impl DocumentId {
    pub fn parse(value: impl Into<String>) -> Result<Self, DocumentCodecError> {
        let value = value.into();
        validate_token("primary_key", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for DocumentId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RevisionToken(String);

impl RevisionToken {
    pub fn parse(value: impl Into<String>) -> Result<Self, DocumentCodecError> {
        let value = value.into();
        validate_token("revision", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RevisionToken {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchemaVersion(NonZeroU64);

impl SchemaVersion {
    pub fn new(value: u64) -> Result<Self, DocumentCodecError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DocumentCodecError::InvalidField {
                field: "schema_version",
                reason: "must be positive",
            })
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CommitPosition(NonZeroU64);

impl CommitPosition {
    pub fn new(value: u64) -> Result<Self, DocumentCodecError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DocumentCodecError::InvalidField {
                field: "commit_position",
                reason: "must be positive",
            })
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// The storage representation of one current or historical document revision.
///
/// Fields are private so callers cannot bypass normalization or metadata
/// validation. `encode` always emits a deterministic JSON representation.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalDocument {
    primary_key: DocumentId,
    schema_version: SchemaVersion,
    revision: RevisionToken,
    commit_position: CommitPosition,
    deleted: bool,
    body: Map<String, Value>,
}

impl CanonicalDocument {
    pub fn new(
        primary_key: DocumentId,
        schema_version: SchemaVersion,
        revision: RevisionToken,
        commit_position: CommitPosition,
        deleted: bool,
        body: Value,
    ) -> Result<Self, DocumentCodecError> {
        let body = normalize_object(body)?;
        Ok(Self {
            primary_key,
            schema_version,
            revision,
            commit_position,
            deleted,
            body,
        })
    }

    #[must_use]
    pub fn primary_key(&self) -> &DocumentId {
        &self.primary_key
    }

    #[must_use]
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    #[must_use]
    pub fn revision(&self) -> &RevisionToken {
        &self.revision
    }

    #[must_use]
    pub const fn commit_position(&self) -> CommitPosition {
        self.commit_position
    }

    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted
    }

    #[must_use]
    pub fn body(&self) -> &Map<String, Value> {
        &self.body
    }

    pub fn encode(&self) -> Result<Vec<u8>, DocumentCodecError> {
        let wire = DocumentWire {
            encoding_version: DOCUMENT_ENCODING_VERSION,
            primary_key: self.primary_key.as_str(),
            schema_version: self.schema_version.get(),
            revision: self.revision.as_str(),
            commit_position: self.commit_position.get(),
            deleted: self.deleted,
            body: &self.body,
        };
        serde_json::to_vec(&wire).map_err(DocumentCodecError::Json)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DocumentCodecError> {
        let wire: OwnedDocumentWire =
            serde_json::from_slice(bytes).map_err(DocumentCodecError::Json)?;
        if wire.encoding_version != DOCUMENT_ENCODING_VERSION {
            return Err(DocumentCodecError::UnsupportedEncodingVersion(
                wire.encoding_version,
            ));
        }
        Self::new(
            DocumentId::parse(wire.primary_key)?,
            SchemaVersion::new(wire.schema_version)?,
            RevisionToken::parse(wire.revision)?,
            CommitPosition::new(wire.commit_position)?,
            wire.deleted,
            Value::Object(wire.body),
        )
    }
}

#[derive(Serialize)]
struct DocumentWire<'a> {
    encoding_version: u8,
    primary_key: &'a str,
    schema_version: u64,
    revision: &'a str,
    commit_position: u64,
    #[serde(rename = "_deleted")]
    deleted: bool,
    body: &'a Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedDocumentWire {
    encoding_version: u8,
    primary_key: String,
    schema_version: u64,
    revision: String,
    commit_position: u64,
    #[serde(rename = "_deleted")]
    deleted: bool,
    body: Map<String, Value>,
}

#[derive(Debug)]
pub enum DocumentCodecError {
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    BodyMustBeObject,
    UnsupportedEncodingVersion(u8),
    Json(serde_json::Error),
}

impl fmt::Display for DocumentCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::BodyMustBeObject => formatter.write_str("document body must be a JSON object"),
            Self::UnsupportedEncodingVersion(version) => {
                write!(formatter, "unsupported document encoding version {version}")
            }
            Self::Json(error) => write!(formatter, "invalid document encoding: {error}"),
        }
    }
}

impl Error for DocumentCodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) fn validate_token(field: &'static str, value: &str) -> Result<(), DocumentCodecError> {
    if value.is_empty() {
        return Err(DocumentCodecError::InvalidField {
            field,
            reason: "cannot be empty",
        });
    }
    if value.trim() != value {
        return Err(DocumentCodecError::InvalidField {
            field,
            reason: "cannot have surrounding whitespace",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(DocumentCodecError::InvalidField {
            field,
            reason: "cannot contain control characters",
        });
    }
    Ok(())
}

pub(crate) fn normalize_object(value: Value) -> Result<Map<String, Value>, DocumentCodecError> {
    match normalize_value(value) {
        Value::Object(object) => Ok(object),
        _ => Err(DocumentCodecError::BodyMustBeObject),
    }
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_value).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, normalize_value(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn document(body: Value) -> CanonicalDocument {
        CanonicalDocument::new(
            DocumentId::parse("doc-1").expect("document id"),
            SchemaVersion::new(3).expect("schema version"),
            RevisionToken::parse("rev_opaque").expect("revision"),
            CommitPosition::new(42).expect("commit position"),
            false,
            body,
        )
        .expect("canonical document")
    }

    #[test]
    fn encoding_is_deterministic_and_recursively_orders_object_keys() {
        let first = document(json!({
            "z": { "second": 2, "first": 1 },
            "a": [{ "y": true, "x": false }]
        }));
        let second = document(json!({
            "a": [{ "x": false, "y": true }],
            "z": { "first": 1, "second": 2 }
        }));

        assert_eq!(first, second);
        assert_eq!(
            first.encode().expect("encode"),
            second.encode().expect("encode")
        );
        assert_eq!(
            String::from_utf8(first.encode().expect("encode")).expect("utf8"),
            r#"{"encoding_version":1,"primary_key":"doc-1","schema_version":3,"revision":"rev_opaque","commit_position":42,"_deleted":false,"body":{"a":[{"x":false,"y":true}],"z":{"first":1,"second":2}}}"#
        );
    }

    #[test]
    fn encoding_round_trips_all_revision_metadata() {
        let expected = CanonicalDocument::new(
            DocumentId::parse("doc-2").expect("document id"),
            SchemaVersion::new(7).expect("schema version"),
            RevisionToken::parse("revision-7").expect("revision"),
            CommitPosition::new(99).expect("commit position"),
            true,
            json!({"id": "doc-2"}),
        )
        .expect("document");

        let encoded = expected.encode().expect("encode");
        let actual = CanonicalDocument::decode(&encoded).expect("decode");

        assert_eq!(actual, expected);
        assert!(actual.is_deleted());
        assert_eq!(actual.primary_key().as_str(), "doc-2");
        assert_eq!(actual.schema_version().get(), 7);
        assert_eq!(actual.commit_position().get(), 99);
    }

    #[test]
    fn invalid_metadata_and_non_object_bodies_are_rejected() {
        assert!(DocumentId::parse("").is_err());
        assert!(RevisionToken::parse(" revision").is_err());
        assert!(SchemaVersion::new(0).is_err());
        assert!(CommitPosition::new(0).is_err());

        let result = CanonicalDocument::new(
            DocumentId::parse("doc").expect("id"),
            SchemaVersion::new(1).expect("version"),
            RevisionToken::parse("rev").expect("revision"),
            CommitPosition::new(1).expect("position"),
            false,
            json!(["not", "an", "object"]),
        );
        assert!(matches!(result, Err(DocumentCodecError::BodyMustBeObject)));
    }

    #[test]
    fn decoding_rejects_unknown_fields_and_encoding_versions() {
        let unknown = br#"{"encoding_version":1,"primary_key":"doc","schema_version":1,"revision":"rev","commit_position":1,"_deleted":false,"body":{},"extra":true}"#;
        assert!(matches!(
            CanonicalDocument::decode(unknown),
            Err(DocumentCodecError::Json(_))
        ));

        let future = br#"{"encoding_version":2,"primary_key":"doc","schema_version":1,"revision":"rev","commit_position":1,"_deleted":false,"body":{}}"#;
        assert!(matches!(
            CanonicalDocument::decode(future),
            Err(DocumentCodecError::UnsupportedEncodingVersion(2))
        ));
    }
}
