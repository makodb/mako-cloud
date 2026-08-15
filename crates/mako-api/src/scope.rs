use std::{error::Error, fmt};

use serde::{Deserialize, Deserializer, Serialize, de};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdentifierKind {
    Project,
    Environment,
    Collection,
}

/// A rejected or ambiguous tenant boundary value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeError {
    MissingProject,
    MissingEnvironment,
    InvalidProject,
    InvalidEnvironment,
    InvalidCollection,
    TenantMismatch,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingProject => "project scope is required",
            Self::MissingEnvironment => "environment scope is required",
            Self::InvalidProject => "project identifier is invalid",
            Self::InvalidEnvironment => "environment identifier is invalid",
            Self::InvalidCollection => "collection identifier is invalid",
            Self::TenantMismatch => "request and trusted tenant scopes do not match",
        })
    }
}

impl Error for ScopeError {}

fn validate_identifier(kind: IdentifierKind, value: &str) -> Result<(), ScopeError> {
    let invalid = match kind {
        IdentifierKind::Project => !prefixed_id_is_valid(value, "prj_"),
        IdentifierKind::Environment => !prefixed_id_is_valid(value, "env_"),
        IdentifierKind::Collection => !collection_id_is_valid(value),
    };

    if invalid {
        Err(match kind {
            IdentifierKind::Project => ScopeError::InvalidProject,
            IdentifierKind::Environment => ScopeError::InvalidEnvironment,
            IdentifierKind::Collection => ScopeError::InvalidCollection,
        })
    } else {
        Ok(())
    }
}

fn prefixed_id_is_valid(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };

    (8..=64).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn collection_id_is_valid(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    value.len() <= 63
        && first.is_ascii_lowercase()
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

macro_rules! scoped_identifier {
    ($name:ident, $kind:expr) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, ScopeError> {
                let value = value.into();
                validate_identifier($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(de::Error::custom)
            }
        }
    };
}

scoped_identifier!(ProjectId, IdentifierKind::Project);
scoped_identifier!(EnvironmentId, IdentifierKind::Environment);
scoped_identifier!(CollectionId, IdentifierKind::Collection);

/// The minimum trusted tenant context accepted by internal service APIs.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantScope {
    project_id: ProjectId,
    environment_id: EnvironmentId,
}

impl TenantScope {
    pub fn new(project_id: ProjectId, environment_id: EnvironmentId) -> Self {
        Self {
            project_id,
            environment_id,
        }
    }

    pub fn require(
        project_id: Option<&str>,
        environment_id: Option<&str>,
    ) -> Result<Self, ScopeError> {
        let project_id = project_id.ok_or(ScopeError::MissingProject)?;
        let environment_id = environment_id.ok_or(ScopeError::MissingEnvironment)?;
        Ok(Self::new(
            ProjectId::parse(project_id)?,
            EnvironmentId::parse(environment_id)?,
        ))
    }

    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    pub fn ensure_matches(&self, trusted: &Self) -> Result<(), ScopeError> {
        if self == trusted {
            Ok(())
        } else {
            Err(ScopeError::TenantMismatch)
        }
    }
}

/// A collection address that cannot exist without a complete tenant scope.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionScope {
    tenant: TenantScope,
    collection_id: CollectionId,
}

impl CollectionScope {
    pub fn new(tenant: TenantScope, collection_id: CollectionId) -> Self {
        Self {
            tenant,
            collection_id,
        }
    }

    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(project: &str, environment: &str) -> TenantScope {
        TenantScope::require(Some(project), Some(environment)).expect("test scope must be valid")
    }

    #[test]
    fn requires_both_tenant_dimensions() {
        assert_eq!(
            TenantScope::require(None, Some("env_abcdefgh")),
            Err(ScopeError::MissingProject)
        );
        assert_eq!(
            TenantScope::require(Some("prj_abcdefgh"), None),
            Err(ScopeError::MissingEnvironment)
        );
    }

    #[test]
    fn rejects_malformed_and_whitespace_identifiers() {
        assert_eq!(
            ProjectId::parse(" prj_abcdefgh"),
            Err(ScopeError::InvalidProject)
        );
        assert_eq!(
            EnvironmentId::parse("env_short"),
            Err(ScopeError::InvalidEnvironment)
        );
        assert_eq!(
            CollectionId::parse("Users"),
            Err(ScopeError::InvalidCollection)
        );
    }

    #[test]
    fn rejects_cross_tenant_boundary_context() {
        let requested = scope("prj_abcdefgh", "env_abcdefgh");
        let trusted = scope("prj_ijklmnop", "env_abcdefgh");
        assert_eq!(
            requested.ensure_matches(&trusted),
            Err(ScopeError::TenantMismatch)
        );
    }

    #[test]
    fn validates_identifiers_during_deserialization() {
        let invalid = r#"{"projectId":"prj_bad","environmentId":"env_abcdefgh"}"#;
        assert!(serde_json::from_str::<TenantScope>(invalid).is_err());
    }
}
