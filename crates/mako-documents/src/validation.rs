use std::{error::Error, fmt};

use serde_json::{Map, Value};

use crate::{
    CanonicalDocument, CollectionMetadata, DocumentCodecError, DocumentId, PrimaryKeyDefinition,
    SchemaVersion, document::normalize_object,
};

/// A normalized document body that has passed both JSON Schema and primary-key checks.
/// Mutation APIs accept this type so invalid bodies cannot reach a storage write.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedDocumentBody {
    primary_key: DocumentId,
    schema_version: SchemaVersion,
    body: Map<String, Value>,
}

impl ValidatedDocumentBody {
    #[must_use]
    pub fn primary_key(&self) -> &DocumentId {
        &self.primary_key
    }

    #[must_use]
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    #[must_use]
    pub fn body(&self) -> &Map<String, Value> {
        &self.body
    }

    #[must_use]
    pub fn into_body(self) -> Map<String, Value> {
        self.body
    }
}

/// A reusable, side-effect-free validator compiled from one collection version.
pub struct DocumentValidator {
    schema_version: SchemaVersion,
    primary_key: PrimaryKeyDefinition,
    validator: jsonschema::Validator,
}

impl fmt::Debug for DocumentValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentValidator")
            .field("schema_version", &self.schema_version)
            .field("primary_key", &self.primary_key)
            .finish_non_exhaustive()
    }
}

impl DocumentValidator {
    pub fn compile(metadata: &CollectionMetadata) -> Result<Self, DocumentValidationError> {
        let schema = Value::Object(metadata.json_schema().clone());
        let validator = jsonschema::validator_for(&schema).map_err(|error| {
            DocumentValidationError::InvalidSchema {
                schema_path: error.schema_path().to_string(),
            }
        })?;
        Ok(Self {
            schema_version: metadata.schema_version(),
            primary_key: metadata.primary_key().clone(),
            validator,
        })
    }

    #[must_use]
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub fn validate_create(
        &self,
        proposed: Value,
    ) -> Result<ValidatedDocumentBody, DocumentValidationError> {
        self.validate(proposed, None)
    }

    pub fn validate_update(
        &self,
        current: &CanonicalDocument,
        proposed: Value,
    ) -> Result<ValidatedDocumentBody, DocumentValidationError> {
        self.validate(proposed, Some(current.primary_key()))
    }

    fn validate(
        &self,
        proposed: Value,
        expected_primary_key: Option<&DocumentId>,
    ) -> Result<ValidatedDocumentBody, DocumentValidationError> {
        let issues: Vec<_> = self
            .validator
            .iter_errors(&proposed)
            .map(|error| SchemaValidationIssue {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
            })
            .collect();
        if !issues.is_empty() {
            return Err(DocumentValidationError::SchemaViolation { issues });
        }

        let body = normalize_object(proposed).map_err(DocumentValidationError::Document)?;
        let primary_key = extract_primary_key(&body, &self.primary_key)?;
        if let Some(expected) = expected_primary_key
            && expected != &primary_key
        {
            return Err(DocumentValidationError::PrimaryKeyChanged {
                expected: expected.clone(),
                proposed: primary_key,
            });
        }

        Ok(ValidatedDocumentBody {
            primary_key,
            schema_version: self.schema_version,
            body,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaValidationIssue {
    pub instance_path: String,
    pub schema_path: String,
}

#[derive(Debug)]
pub enum DocumentValidationError {
    InvalidSchema {
        schema_path: String,
    },
    SchemaViolation {
        issues: Vec<SchemaValidationIssue>,
    },
    MissingPrimaryKey {
        field: String,
    },
    PrimaryKeyMustBeString {
        field: String,
    },
    CompositeSourceMustBeScalar {
        field: String,
    },
    CompositePrimaryKeyMismatch {
        field: String,
    },
    PrimaryKeyChanged {
        expected: DocumentId,
        proposed: DocumentId,
    },
    Document(DocumentCodecError),
}

impl fmt::Display for DocumentValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchema { schema_path } => {
                write!(formatter, "invalid JSON Schema at {schema_path}")
            }
            Self::SchemaViolation { issues } => {
                write!(
                    formatter,
                    "document has {} schema violation(s)",
                    issues.len()
                )
            }
            Self::MissingPrimaryKey { field } => {
                write!(formatter, "document is missing primary-key field {field}")
            }
            Self::PrimaryKeyMustBeString { field } => {
                write!(formatter, "primary-key field {field} must be a string")
            }
            Self::CompositeSourceMustBeScalar { field } => {
                write!(
                    formatter,
                    "composite primary-key source {field} must be a scalar"
                )
            }
            Self::CompositePrimaryKeyMismatch { field } => {
                write!(
                    formatter,
                    "composite primary-key field {field} does not match its sources"
                )
            }
            Self::PrimaryKeyChanged { .. } => {
                formatter.write_str("an update cannot change the document primary key")
            }
            Self::Document(error) => error.fmt(formatter),
        }
    }
}

impl Error for DocumentValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Document(error) => Some(error),
            _ => None,
        }
    }
}

fn extract_primary_key(
    body: &Map<String, Value>,
    definition: &PrimaryKeyDefinition,
) -> Result<DocumentId, DocumentValidationError> {
    match definition {
        PrimaryKeyDefinition::Field { field } => extract_string_key(body, field),
        PrimaryKeyDefinition::Composite {
            key,
            fields,
            separator,
        } => {
            let stored = extract_string_key(body, key)?;
            let components = fields
                .iter()
                .map(|field| {
                    field_value(body, field)
                        .ok_or_else(|| DocumentValidationError::MissingPrimaryKey {
                            field: field.clone(),
                        })
                        .and_then(|value| scalar_key_component(field, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if stored.as_str() != components.join(separator) {
                return Err(DocumentValidationError::CompositePrimaryKeyMismatch {
                    field: key.clone(),
                });
            }
            Ok(stored)
        }
    }
}

fn extract_string_key(
    body: &Map<String, Value>,
    field: &str,
) -> Result<DocumentId, DocumentValidationError> {
    let value =
        field_value(body, field).ok_or_else(|| DocumentValidationError::MissingPrimaryKey {
            field: field.to_owned(),
        })?;
    let value = value
        .as_str()
        .ok_or_else(|| DocumentValidationError::PrimaryKeyMustBeString {
            field: field.to_owned(),
        })?;
    DocumentId::parse(value).map_err(DocumentValidationError::Document)
}

fn scalar_key_component(field: &str, value: &Value) -> Result<String, DocumentValidationError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => {
            Err(DocumentValidationError::CompositeSourceMustBeScalar {
                field: field.to_owned(),
            })
        }
    }
}

fn field_value<'a>(body: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut value = body.get(first)?;
    for segment in segments {
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use mako_api::CollectionId;
    use serde_json::json;

    use super::*;
    use crate::{
        CollectionLifecycle, CollectionMetadataVersion, CommitPosition, RevisionToken,
        SchemaCompatibility,
    };

    fn metadata(primary_key: PrimaryKeyDefinition) -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(3).expect("schema version"),
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string", "minLength": 1 },
                    "org": { "type": "string" },
                    "local": { "type": "integer" }
                },
                "required": ["id", "title"],
                "additionalProperties": false
            }),
            primary_key,
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
    }

    fn current(id: &str) -> CanonicalDocument {
        CanonicalDocument::new(
            DocumentId::parse(id).expect("document id"),
            SchemaVersion::new(3).expect("schema version"),
            RevisionToken::parse("rev-current").expect("revision"),
            CommitPosition::new(7).expect("position"),
            false,
            json!({"id": id, "title": "existing"}),
        )
        .expect("document")
    }

    #[test]
    fn validates_schema_and_returns_a_normalized_body() {
        let validator = DocumentValidator::compile(&metadata(
            PrimaryKeyDefinition::field("id").expect("primary key"),
        ))
        .expect("validator");

        let validated = validator
            .validate_create(json!({"title": "write tests", "id": "todo-1"}))
            .expect("valid document");

        assert_eq!(validated.primary_key().as_str(), "todo-1");
        assert_eq!(validated.schema_version().get(), 3);
        assert_eq!(
            validated.body().keys().cloned().collect::<Vec<_>>(),
            ["id", "title"]
        );
    }

    #[test]
    fn reports_only_schema_paths_not_protected_values() {
        let validator = DocumentValidator::compile(&metadata(
            PrimaryKeyDefinition::field("id").expect("primary key"),
        ))
        .expect("validator");
        let protected_value = "do-not-put-this-in-an-error";

        let error = validator
            .validate_create(json!({"id": "todo-1", "title": protected_value, "extra": true}))
            .expect_err("additional property must fail");
        let DocumentValidationError::SchemaViolation { issues } = error else {
            panic!("expected schema violation");
        };

        assert!(!issues.is_empty());
        assert!(!format!("{issues:?}").contains(protected_value));
    }

    #[test]
    fn immutable_primary_key_is_checked_before_a_mutation_can_be_built() {
        let validator = DocumentValidator::compile(&metadata(
            PrimaryKeyDefinition::field("id").expect("primary key"),
        ))
        .expect("validator");
        let existing = current("todo-1");

        let error = validator
            .validate_update(&existing, json!({"id": "todo-2", "title": "changed"}))
            .expect_err("primary key change must fail");

        assert!(matches!(
            error,
            DocumentValidationError::PrimaryKeyChanged { .. }
        ));
    }

    #[test]
    fn validates_rxdb_composite_primary_key_material() {
        let validator = DocumentValidator::compile(&metadata(
            PrimaryKeyDefinition::composite("id", ["org", "local"], "|").expect("primary key"),
        ))
        .expect("validator");

        let valid = validator.validate_create(
            json!({"id": "acme|42", "org": "acme", "local": 42, "title": "valid"}),
        );
        assert!(valid.is_ok());

        let mismatch = validator.validate_create(
            json!({"id": "wrong", "org": "acme", "local": 42, "title": "invalid"}),
        );
        assert!(matches!(
            mismatch,
            Err(DocumentValidationError::CompositePrimaryKeyMismatch { .. })
        ));
    }

    #[test]
    fn invalid_schema_is_rejected_during_compilation() {
        let invalid = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({"type": "definitely-not-a-json-schema-type"}),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::PendingValidation,
            CollectionLifecycle::Creating,
        )
        .expect("metadata encoding accepts draft schemas");

        assert!(matches!(
            DocumentValidator::compile(&invalid),
            Err(DocumentValidationError::InvalidSchema { .. })
        ));
    }
}
