use std::{collections::BTreeMap, error::Error, fmt};

use mako_api::CollectionScope;
use serde_json::{Map, Value};

use crate::DocumentOperation;

/// The longest address a policy will carry. RFC 5321 bounds a path at 256
/// octets; anything longer is not an address this platform issued a token for.
const MAX_EMAIL_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SubjectId(String);

impl SubjectId {
    pub fn parse(value: impl Into<String>) -> Result<Self, PolicyContextError> {
        let value = value.into();
        validate_token("identity.user_id", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct VerifiedRole(String);

impl VerifiedRole {
    pub fn parse(value: impl Into<String>) -> Result<Self, PolicyContextError> {
        let value = value.into();
        validate_token("identity.role", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The address a caller's account is registered under, and whether the
/// environment has confirmed that the caller controls it.
///
/// The two are separate because they answer different questions. The address
/// is always the one the account authenticated as; confirmation is what says
/// somebody proved they can receive mail there. A rule that hands a document
/// to an address -- an invitation, say -- must require both, or an attacker
/// registers the address they want to read and is handed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEmail {
    address: String,
    confirmed: bool,
}

impl VerifiedEmail {
    pub fn parse(address: impl Into<String>, confirmed: bool) -> Result<Self, PolicyContextError> {
        let address = address.into();
        if address.is_empty()
            || address.len() > MAX_EMAIL_BYTES
            || address.chars().any(char::is_control)
            || !address.contains('@')
        {
            return Err(PolicyContextError::InvalidToken("identity.email"));
        }
        Ok(Self {
            address: address.to_lowercase(),
            confirmed,
        })
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    #[must_use]
    pub const fn confirmed(&self) -> bool {
        self.confirmed
    }
}

/// Identity data supplied only after gateway/session verification. There is no
/// field for user-editable profile metadata by design.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedIdentity {
    user_id: Option<SubjectId>,
    role: VerifiedRole,
    email: Option<VerifiedEmail>,
    trusted_claims: Map<String, Value>,
}

impl VerifiedIdentity {
    pub fn anonymous(trusted_claims: Value) -> Result<Self, PolicyContextError> {
        Self::new(
            None,
            VerifiedRole::parse("anonymous")?,
            None,
            trusted_claims,
        )
    }

    pub fn user(
        user_id: SubjectId,
        role: VerifiedRole,
        email: Option<VerifiedEmail>,
        trusted_claims: Value,
    ) -> Result<Self, PolicyContextError> {
        Self::new(Some(user_id), role, email, trusted_claims)
    }

    fn new(
        user_id: Option<SubjectId>,
        role: VerifiedRole,
        email: Option<VerifiedEmail>,
        trusted_claims: Value,
    ) -> Result<Self, PolicyContextError> {
        let Value::Object(trusted_claims) = trusted_claims else {
            return Err(PolicyContextError::TrustedClaimsMustBeObject);
        };
        validate_json_depth(&Value::Object(trusted_claims.clone()), 0)?;
        Ok(Self {
            user_id,
            role,
            email,
            trusted_claims,
        })
    }

    #[must_use]
    pub fn user_id(&self) -> Option<&SubjectId> {
        self.user_id.as_ref()
    }

    #[must_use]
    pub fn role(&self) -> &VerifiedRole {
        &self.role
    }

    #[must_use]
    pub fn email(&self) -> Option<&VerifiedEmail> {
        self.email.as_ref()
    }

    #[must_use]
    pub fn trusted_claims(&self) -> &Map<String, Value> {
        &self.trusted_claims
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SafeRequestMetadata {
    attributes: BTreeMap<String, String>,
}

impl SafeRequestMetadata {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            attributes: BTreeMap::new(),
        }
    }

    pub fn new(
        attributes: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, PolicyContextError> {
        let mut safe = BTreeMap::new();
        for (name, value) in attributes {
            validate_attribute_name(&name)?;
            if value.len() > 512 || value.chars().any(char::is_control) {
                return Err(PolicyContextError::InvalidRequestAttributeValue(name));
            }
            if safe.insert(name.clone(), value).is_some() {
                return Err(PolicyContextError::DuplicateRequestAttribute(name));
            }
        }
        Ok(Self { attributes: safe })
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PolicyEvaluationContext {
    scope: CollectionScope,
    operation: DocumentOperation,
    identity: VerifiedIdentity,
    old_document: Option<Map<String, Value>>,
    new_document: Option<Map<String, Value>>,
    request: SafeRequestMetadata,
}

impl PolicyEvaluationContext {
    pub fn new(
        scope: CollectionScope,
        operation: DocumentOperation,
        identity: VerifiedIdentity,
        old_document: Option<Value>,
        new_document: Option<Value>,
        request: SafeRequestMetadata,
    ) -> Result<Self, PolicyContextError> {
        let old_document = old_document.map(require_document).transpose()?;
        let new_document = new_document.map(require_document).transpose()?;
        let valid_states = match operation {
            DocumentOperation::Create => old_document.is_none() && new_document.is_some(),
            DocumentOperation::Read | DocumentOperation::Delete => {
                old_document.is_some() && new_document.is_none()
            }
            DocumentOperation::Update => old_document.is_some() && new_document.is_some(),
        };
        if !valid_states {
            return Err(PolicyContextError::InvalidDocumentStates(operation));
        }
        Ok(Self {
            scope,
            operation,
            identity,
            old_document,
            new_document,
            request,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    #[must_use]
    pub const fn operation(&self) -> DocumentOperation {
        self.operation
    }

    #[must_use]
    pub fn identity(&self) -> &VerifiedIdentity {
        &self.identity
    }

    #[must_use]
    pub fn old_document(&self) -> Option<&Map<String, Value>> {
        self.old_document.as_ref()
    }

    #[must_use]
    pub fn new_document(&self) -> Option<&Map<String, Value>> {
        self.new_document.as_ref()
    }

    #[must_use]
    pub fn request(&self) -> &SafeRequestMetadata {
        &self.request
    }
}

fn require_document(value: Value) -> Result<Map<String, Value>, PolicyContextError> {
    let Value::Object(document) = value else {
        return Err(PolicyContextError::DocumentMustBeObject);
    };
    validate_json_depth(&Value::Object(document.clone()), 0)?;
    Ok(document)
}

fn validate_json_depth(value: &Value, depth: usize) -> Result<(), PolicyContextError> {
    if depth > 16 {
        return Err(PolicyContextError::JsonDepthExceeded);
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_json_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_json_depth(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_token(field: &'static str, value: &str) -> Result<(), PolicyContextError> {
    if value.is_empty()
        || value.len() > 128
        || !value.chars().all(|character| {
            character == '-' || character == '_' || character.is_ascii_alphanumeric()
        })
    {
        Err(PolicyContextError::InvalidToken(field))
    } else {
        Ok(())
    }
}

fn validate_attribute_name(value: &str) -> Result<(), PolicyContextError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character == '_' || character.is_ascii_lowercase())
    {
        Err(PolicyContextError::InvalidRequestAttributeName(
            value.to_owned(),
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyContextError {
    InvalidToken(&'static str),
    TrustedClaimsMustBeObject,
    DocumentMustBeObject,
    JsonDepthExceeded,
    InvalidDocumentStates(DocumentOperation),
    InvalidRequestAttributeName(String),
    InvalidRequestAttributeValue(String),
    DuplicateRequestAttribute(String),
}

impl fmt::Display for PolicyContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken(field) => write!(formatter, "invalid {field}"),
            Self::TrustedClaimsMustBeObject => {
                formatter.write_str("trusted claims must be a JSON object")
            }
            Self::DocumentMustBeObject => formatter.write_str("policy document must be an object"),
            Self::JsonDepthExceeded => formatter.write_str("policy JSON value exceeds depth limit"),
            Self::InvalidDocumentStates(operation) => {
                write!(
                    formatter,
                    "old/new document states are invalid for {operation:?}"
                )
            }
            Self::InvalidRequestAttributeName(name) => {
                write!(
                    formatter,
                    "request attribute name {name} is not allowlist-safe"
                )
            }
            Self::InvalidRequestAttributeValue(name) => {
                write!(formatter, "request attribute {name} has an invalid value")
            }
            Self::DuplicateRequestAttribute(name) => {
                write!(formatter, "request attribute {name} is duplicated")
            }
        }
    }
}

impl Error for PolicyContextError {}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, EnvironmentId, ProjectId, TenantScope};
    use serde_json::json;

    use super::*;

    #[test]
    fn context_binds_verified_identity_scope_and_operation_states() {
        let identity = VerifiedIdentity::user(
            SubjectId::parse("user-1").expect("subject"),
            VerifiedRole::parse("member").expect("role"),
            None,
            json!({"team_ids": ["blue"]}),
        )
        .expect("identity");
        let context = PolicyEvaluationContext::new(
            scope(),
            DocumentOperation::Update,
            identity,
            Some(json!({"id": "doc-1", "owner_id": "user-1"})),
            Some(json!({"id": "doc-1", "owner_id": "user-1"})),
            SafeRequestMetadata::new([("method".to_owned(), "PATCH".to_owned())]).expect("request"),
        )
        .expect("context");

        assert_eq!(
            context.identity().user_id().expect("user").as_str(),
            "user-1"
        );
        assert_eq!(context.identity().role().as_str(), "member");
        assert_eq!(context.request().get("method"), Some("PATCH"));
        assert_eq!(context.operation(), DocumentOperation::Update);
    }

    #[test]
    fn operation_incorrect_states_and_non_object_claims_fail() {
        assert!(VerifiedIdentity::anonymous(json!(["not", "claims"])).is_err());
        let identity = VerifiedIdentity::anonymous(json!({})).expect("identity");
        assert!(matches!(
            PolicyEvaluationContext::new(
                scope(),
                DocumentOperation::Create,
                identity,
                Some(json!({"id": "old"})),
                Some(json!({"id": "new"})),
                SafeRequestMetadata::empty(),
            ),
            Err(PolicyContextError::InvalidDocumentStates(
                DocumentOperation::Create
            ))
        ));
    }

    fn scope() -> CollectionScope {
        CollectionScope::new(
            TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            CollectionId::parse("todos").expect("collection"),
        )
    }
}
