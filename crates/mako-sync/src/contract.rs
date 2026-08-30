use std::{error::Error, fmt};

use mako_api::{ApiErrorEnvelope, CollectionId, TenantScope};
use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};

pub const MAX_PULL_BATCH_SIZE: usize = 1_000;
/// A filter names one field by its dotted path and one string value; both are
/// bounded because they are read on every change the scan examines.
pub const MAX_FILTER_FIELD_BYTES: usize = 128;
pub const MAX_FILTER_VALUE_BYTES: usize = 256;
pub const MAX_PUSH_BATCH_SIZE: usize = 1_000;
pub const MAX_MUTATION_ID_BYTES: usize = 256;

macro_rules! opaque_token {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub(crate) fn new(value: String) -> Result<Self, ReplicationContractError> {
                if !value.starts_with($prefix) || !(16..=4096).contains(&value.len()) {
                    return Err(ReplicationContractError::InvalidOpaqueToken);
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([OPAQUE])"))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

opaque_token!(OpaqueCheckpoint, "mcp1.");
opaque_token!(OpaqueStreamCursor, "msc1.");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedReplicationContext {
    tenant: TenantScope,
    collection_id: CollectionId,
    user_id: AppUserId,
    session_id: SessionId,
    role: String,
    authorization_epochs: AccessAuthorizationEpochs,
    request_id: String,
}

impl AuthenticatedReplicationContext {
    pub fn new(
        tenant: TenantScope,
        collection_id: CollectionId,
        user_id: AppUserId,
        session_id: SessionId,
        role: impl Into<String>,
        authorization_epochs: AccessAuthorizationEpochs,
        request_id: impl Into<String>,
    ) -> Result<Self, ReplicationContractError> {
        let role = role.into();
        let request_id = request_id.into();
        if !valid_identifier(&role, 128) || !valid_identifier(&request_id, 256) {
            return Err(ReplicationContractError::InvalidIdentityContext);
        }
        Ok(Self {
            tenant,
            collection_id,
            user_id,
            session_id,
            role,
            authorization_epochs,
            request_id,
        })
    }

    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    #[must_use]
    pub fn user_id(&self) -> &AppUserId {
        &self.user_id
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub const fn authorization_epochs(&self) -> AccessAuthorizationEpochs {
        self.authorization_epochs
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// Narrows a replication scope to the documents whose field holds one value.
///
/// A user who belongs to several households may read every one of their
/// documents, so without this a per-household local database receives all of
/// them and discards what it did not want -- paying for the transfer, and
/// re-examining every other household's changes on every pull. The filter is
/// applied **after** the policy, so it can only ever narrow what the caller
/// was already allowed to see; it is not an authorization mechanism and
/// nothing about it is trusted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReplicationFilter {
    pub field: String,
    pub value: String,
}

impl ReplicationFilter {
    pub fn validate(&self) -> Result<(), ReplicationContractError> {
        let field_valid = !self.field.is_empty()
            && self.field.len() <= MAX_FILTER_FIELD_BYTES
            && self.field.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && !segment.as_bytes()[0].is_ascii_digit()
            });
        if !field_valid
            || self.value.len() > MAX_FILTER_VALUE_BYTES
            || self.value.chars().any(char::is_control)
        {
            return Err(ReplicationContractError::InvalidPullRequest);
        }
        Ok(())
    }

    /// Whether a replicated document body matches. A document that lacks the
    /// field does not match, so a filtered scope never receives one whose
    /// membership it cannot decide.
    #[must_use]
    pub fn matches(&self, body: &Map<String, Value>) -> bool {
        let mut segments = self.field.split('.');
        let first = segments.next().expect("a split always yields one segment");
        let Some(mut value) = body.get(first) else {
            return false;
        };
        for segment in segments {
            match value.get(segment) {
                Some(next) => value = next,
                None => return false,
            }
        }
        value.as_str() == Some(self.value.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PullRequest {
    pub checkpoint: Option<OpaqueCheckpoint>,
    pub schema_version: u64,
    pub batch_size: usize,
    #[serde(default)]
    pub filter: Option<ReplicationFilter>,
}

impl PullRequest {
    pub fn validate(&self) -> Result<(), ReplicationContractError> {
        if self.schema_version == 0 || !(1..=MAX_PULL_BATCH_SIZE).contains(&self.batch_size) {
            return Err(ReplicationContractError::InvalidPullRequest);
        }
        if let Some(filter) = &self.filter {
            filter.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PullResponse {
    pub documents: Vec<Value>,
    pub checkpoint: OpaqueCheckpoint,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PushRow {
    pub mutation_id: String,
    pub assumed_master_state: Option<Value>,
    pub new_document_state: Value,
}

impl PushRow {
    fn validate(&self) -> bool {
        self.mutation_id.len() >= 16
            && valid_identifier(&self.mutation_id, MAX_MUTATION_ID_BYTES)
            && self.new_document_state.is_object()
            && self
                .assumed_master_state
                .as_ref()
                .is_none_or(Value::is_object)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PushRequest {
    pub schema_version: u64,
    pub rows: Vec<PushRow>,
}

impl PushRequest {
    pub fn validate(&self) -> Result<(), ReplicationContractError> {
        if self.schema_version == 0
            || !(1..=MAX_PUSH_BATCH_SIZE).contains(&self.rows.len())
            || self.rows.iter().any(|row| !row.validate())
        {
            return Err(ReplicationContractError::InvalidPushRequest);
        }
        let unique = self
            .rows
            .iter()
            .map(|row| row.mutation_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if unique.len() != self.rows.len() {
            return Err(ReplicationContractError::InvalidPushRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushOutcomeStatus {
    Accepted,
    Conflict,
    Denied,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PushOutcome {
    pub mutation_id: String,
    pub status: PushOutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiErrorEnvelope>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PushResponse {
    pub outcomes: Vec<PushOutcome>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveStreamRequest {
    pub schema_version: u64,
    pub checkpoint: Option<OpaqueCheckpoint>,
    pub cursor: Option<OpaqueStreamCursor>,
    /// The same narrowing the pull uses, and it has to be the same one: a
    /// stream wider than the pull delivers documents the local database
    /// discards, and one narrower silently withholds changes the pull would
    /// have sent. The cursor and checkpoint are bound to it, so a scope
    /// cannot resume under a filter it did not scan under.
    #[serde(default)]
    pub filter: Option<ReplicationFilter>,
}

impl LiveStreamRequest {
    pub fn validate(&self) -> Result<(), ReplicationContractError> {
        if self.schema_version == 0 || (self.checkpoint.is_some() && self.cursor.is_some()) {
            return Err(ReplicationContractError::InvalidStreamRequest);
        }
        if let Some(filter) = &self.filter {
            filter
                .validate()
                .map_err(|_| ReplicationContractError::InvalidStreamRequest)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LiveStreamEvent {
    Documents {
        documents: Vec<Value>,
        checkpoint: OpaqueCheckpoint,
        cursor: OpaqueStreamCursor,
    },
    Checkpoint {
        checkpoint: OpaqueCheckpoint,
        cursor: OpaqueStreamCursor,
    },
    Heartbeat {
        cursor: OpaqueStreamCursor,
    },
    Resync {
        reason: ResyncReason,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    Reconnected,
    StreamGap,
    CheckpointExpired,
    AuthorizationEpochChanged,
    ServiceFailover,
}

fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationContractError {
    InvalidOpaqueToken,
    InvalidIdentityContext,
    InvalidPullRequest,
    InvalidPushRequest,
    InvalidStreamRequest,
}

impl fmt::Display for ReplicationContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidOpaqueToken => "opaque replication token is malformed",
            Self::InvalidIdentityContext => "authenticated replication context is invalid",
            Self::InvalidPullRequest => "replication pull request is invalid",
            Self::InvalidPushRequest => "replication push request is invalid",
            Self::InvalidStreamRequest => "replication stream request is invalid",
        })
    }
}

impl Error for ReplicationContractError {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn requests_are_bounded_and_push_mutation_ids_are_unique() {
        assert!(
            PullRequest {
                checkpoint: None,
                schema_version: 1,
                batch_size: MAX_PULL_BATCH_SIZE,
                filter: None,
            }
            .validate()
            .is_ok()
        );
        assert!(
            PullRequest {
                checkpoint: None,
                schema_version: 1,
                batch_size: MAX_PULL_BATCH_SIZE + 1,
                filter: None,
            }
            .validate()
            .is_err()
        );
        let row = PushRow {
            mutation_id: "mutation_abcdefghijkl".to_owned(),
            assumed_master_state: None,
            new_document_state: json!({"id": "one"}),
        };
        assert!(
            PushRequest {
                schema_version: 1,
                rows: vec![row.clone(), row],
            }
            .validate()
            .is_err()
        );
        assert!(
            PushRequest {
                schema_version: 1,
                rows: (0..=MAX_PUSH_BATCH_SIZE)
                    .map(|index| PushRow {
                        mutation_id: format!("oversized_mutation_{index:04}"),
                        assumed_master_state: None,
                        new_document_state: json!({"id": index}),
                    })
                    .collect(),
            }
            .validate()
            .is_err()
        );
        assert!(
            LiveStreamRequest {
                schema_version: 1,
                checkpoint: Some(
                    OpaqueCheckpoint::new("mcp1.abcdefghijkl".to_owned()).expect("checkpoint")
                ),
                cursor: Some(
                    OpaqueStreamCursor::new("msc1.abcdefghijkl".to_owned()).expect("cursor")
                ),
                filter: None,
            }
            .validate()
            .is_err()
        );
    }
}
