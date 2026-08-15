use std::{collections::BTreeSet, error::Error, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_api::{EnvironmentId, ProjectId};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{ControlKeyspace, ControlKeyspaceError, DeveloperIdentityId, OrganizationId};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AutomationTokenId(String);

impl AutomationTokenId {
    pub fn parse(value: impl Into<String>) -> Result<Self, TokenServiceError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("atm_") else {
            return Err(TokenServiceError::InvalidToken);
        };
        if !(8..=64).contains(&suffix.len())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(TokenServiceError::InvalidToken);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AutomationTokenId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationPermission {
    OrganizationRead,
    ProjectRead,
    ProjectWrite,
    EnvironmentRead,
    EnvironmentWrite,
    CollectionWrite,
    PolicyWrite,
    FunctionDeploy,
    AuditRead,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AutomationScope {
    organization_id: OrganizationId,
    project_id: Option<ProjectId>,
    environment_id: Option<EnvironmentId>,
    permissions: BTreeSet<AutomationPermission>,
}

impl AutomationScope {
    pub fn new(
        organization_id: OrganizationId,
        project_id: Option<ProjectId>,
        environment_id: Option<EnvironmentId>,
        permissions: impl IntoIterator<Item = AutomationPermission>,
    ) -> Result<Self, TokenServiceError> {
        let permissions = permissions.into_iter().collect::<BTreeSet<_>>();
        if permissions.is_empty() || (environment_id.is_some() && project_id.is_none()) {
            return Err(TokenServiceError::InvalidScope);
        }
        Ok(Self {
            organization_id,
            project_id,
            environment_id,
            permissions,
        })
    }

    #[must_use]
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    #[must_use]
    pub fn allows(
        &self,
        permission: AutomationPermission,
        resource: &ManagementResourceScope,
    ) -> bool {
        self.permissions.contains(&permission)
            && self.organization_id == resource.organization_id
            && self
                .project_id
                .as_ref()
                .is_none_or(|project| resource.project_id.as_ref() == Some(project))
            && self
                .environment_id
                .as_ref()
                .is_none_or(|environment| resource.environment_id.as_ref() == Some(environment))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementResourceScope {
    pub organization_id: OrganizationId,
    pub project_id: Option<ProjectId>,
    pub environment_id: Option<EnvironmentId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTokenStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AutomationTokenRecord {
    id: AutomationTokenId,
    name: String,
    scope: AutomationScope,
    secret_digest: String,
    status: AutomationTokenStatus,
    created_by: DeveloperIdentityId,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    revoked_at_unix_seconds: Option<u64>,
}

impl AutomationTokenRecord {
    #[must_use]
    pub fn id(&self) -> &AutomationTokenId {
        &self.id
    }

    #[must_use]
    pub fn scope(&self) -> &AutomationScope {
        &self.scope
    }

    #[must_use]
    pub const fn status(&self) -> AutomationTokenStatus {
        self.status
    }

    fn revoke(&mut self, now_unix_seconds: u64) -> Result<(), TokenServiceError> {
        if self.status != AutomationTokenStatus::Active
            || now_unix_seconds < self.created_at_unix_seconds
        {
            return Err(TokenServiceError::InvalidToken);
        }
        self.status = AutomationTokenStatus::Revoked;
        self.revoked_at_unix_seconds = Some(now_unix_seconds);
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct AutomationTokenSecret(String);

impl AutomationTokenSecret {
    fn generate(id: &AutomationTokenId) -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(format!(
            "mako_at.{}.{}",
            id.as_str(),
            URL_SAFE_NO_PAD.encode(bytes)
        ))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, TokenServiceError> {
        let value = value.into();
        let mut parts = value.split('.');
        if parts.next() != Some("mako_at")
            || parts.next().is_none()
            || parts
                .next()
                .and_then(|secret| URL_SAFE_NO_PAD.decode(secret).ok())
                .is_none_or(|secret| secret.len() != 32)
            || parts.next().is_some()
        {
            return Err(TokenServiceError::InvalidToken);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.0
    }

    fn id(&self) -> Result<AutomationTokenId, TokenServiceError> {
        let id = self
            .0
            .split('.')
            .nth(1)
            .ok_or(TokenServiceError::InvalidToken)?;
        AutomationTokenId::parse(id)
    }

    fn digest(&self) -> String {
        blake3::hash(self.0.as_bytes()).to_hex().to_string()
    }
}

impl fmt::Debug for AutomationTokenSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AutomationTokenSecret([REDACTED])")
    }
}

#[derive(Debug)]
pub struct AutomationTokenIssue {
    pub record: AutomationTokenRecord,
    pub secret: AutomationTokenSecret,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomationPrincipal {
    token_id: AutomationTokenId,
    scope: AutomationScope,
}

impl AutomationPrincipal {
    #[cfg(test)]
    pub(crate) fn for_test(token_id: AutomationTokenId, scope: AutomationScope) -> Self {
        Self { token_id, scope }
    }

    #[must_use]
    pub fn token_id(&self) -> &AutomationTokenId {
        &self.token_id
    }

    #[must_use]
    pub fn allows(
        &self,
        permission: AutomationPermission,
        resource: &ManagementResourceScope,
    ) -> bool {
        self.scope.allows(permission, resource)
    }
}

#[derive(Clone)]
pub struct AutomationTokenService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl fmt::Debug for AutomationTokenService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AutomationTokenService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl AutomationTokenService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
    ) -> Result<Self, TokenServiceError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(TokenServiceError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
        })
    }

    pub async fn issue(
        &self,
        id: AutomationTokenId,
        name: impl Into<String>,
        scope: AutomationScope,
        created_by: DeveloperIdentityId,
        created_at_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<AutomationTokenIssue, TokenServiceError> {
        let name = name.into();
        if name.is_empty()
            || name.len() > 120
            || name.trim() != name
            || expires_at_unix_seconds <= created_at_unix_seconds
        {
            return Err(TokenServiceError::InvalidToken);
        }
        let secret = AutomationTokenSecret::generate(&id);
        let record = AutomationTokenRecord {
            id,
            name,
            scope,
            secret_digest: secret.digest(),
            status: AutomationTokenStatus::Active,
            created_by,
            created_at_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
        };
        let key = ControlKeyspace::automation_token_key(record.id())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(AutomationTokenIssue { record, secret })
    }

    pub async fn authenticate(
        &self,
        secret: &AutomationTokenSecret,
        now_unix_seconds: u64,
    ) -> Result<AutomationPrincipal, TokenServiceError> {
        let id = secret.id()?;
        let record = self
            .get(&id)
            .await?
            .ok_or(TokenServiceError::InvalidToken)?;
        if record.status != AutomationTokenStatus::Active
            || now_unix_seconds >= record.expires_at_unix_seconds
            || !constant_time_eq(record.secret_digest.as_bytes(), secret.digest().as_bytes())
        {
            return Err(TokenServiceError::InvalidToken);
        }
        Ok(AutomationPrincipal {
            token_id: id,
            scope: record.scope,
        })
    }

    pub async fn revoke(
        &self,
        id: &AutomationTokenId,
        now_unix_seconds: u64,
    ) -> Result<AutomationTokenRecord, TokenServiceError> {
        let previous = self.get(id).await?.ok_or(TokenServiceError::NotFound)?;
        let mut next = previous.clone();
        next.revoke(now_unix_seconds)?;
        let key = ControlKeyspace::automation_token_key(id)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(&previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(next)
    }

    pub async fn rotate(
        &self,
        current_id: &AutomationTokenId,
        replacement_id: AutomationTokenId,
        now_unix_seconds: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<AutomationTokenIssue, TokenServiceError> {
        if current_id == &replacement_id || expires_at_unix_seconds <= now_unix_seconds {
            return Err(TokenServiceError::InvalidToken);
        }
        let previous = self
            .get(current_id)
            .await?
            .ok_or(TokenServiceError::NotFound)?;
        let mut retired = previous.clone();
        retired.revoke(now_unix_seconds)?;
        let secret = AutomationTokenSecret::generate(&replacement_id);
        let replacement = AutomationTokenRecord {
            id: replacement_id,
            name: previous.name.clone(),
            scope: previous.scope.clone(),
            secret_digest: secret.digest(),
            status: AutomationTokenStatus::Active,
            created_by: previous.created_by.clone(),
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
        };
        let current_key = ControlKeyspace::automation_token_key(current_id)?;
        let replacement_key = ControlKeyspace::automation_token_key(replacement.id())?;
        let mut batch = WriteBatch::with_capacity(2);
        batch.put(&current_key, serde_json::to_vec(&retired)?);
        batch.put(&replacement_key, serde_json::to_vec(&replacement)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key: current_key,
                    value: serde_json::to_vec(&previous)?,
                },
                KeyCondition::Missing {
                    key: replacement_key,
                },
            ],
            batch,
            durability: self.durability,
        })
        .await?;
        Ok(AutomationTokenIssue {
            record: replacement,
            secret,
        })
    }

    async fn get(
        &self,
        id: &AutomationTokenId,
    ) -> Result<Option<AutomationTokenRecord>, TokenServiceError> {
        self.adapter
            .get(&ControlKeyspace::automation_token_key(id)?)
            .await?
            .map(|value| serde_json::from_slice(&value).map_err(TokenServiceError::from))
            .transpose()
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), TokenServiceError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(TokenServiceError::Conflict),
        }
    }
}

#[derive(Debug)]
pub enum TokenServiceError {
    InvalidToken,
    InvalidScope,
    NotFound,
    Conflict,
    UnsupportedDurability,
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Serialization(serde_json::Error),
}

impl fmt::Display for TokenServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken => formatter.write_str("automation token is invalid"),
            Self::InvalidScope => formatter.write_str("automation scope is invalid"),
            Self::NotFound => formatter.write_str("automation token was not found"),
            Self::Conflict => formatter.write_str("automation token changed concurrently"),
            Self::UnsupportedDurability => {
                formatter.write_str("automation token durability is unsupported")
            }
            Self::Keyspace(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Serialization(_) => formatter.write_str("automation token serialization failed"),
        }
    }
}

impl Error for TokenServiceError {}

impl From<ControlKeyspaceError> for TokenServiceError {
    fn from(error: ControlKeyspaceError) -> Self {
        Self::Keyspace(error)
    }
}

impl From<StorageError> for TokenServiceError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for TokenServiceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_storage::MemoryAdapter;

    #[test]
    fn token_scope_is_enforced_and_revocation_is_immediate() {
        futures::executor::block_on(async {
            let service =
                AutomationTokenService::new(Arc::new(MemoryAdapter::new()), Durability::Memory)
                    .expect("service");
            let organization = OrganizationId::parse("org_example00").expect("organization");
            let project = ProjectId::parse("prj_example00").expect("project");
            let scope = AutomationScope::new(
                organization.clone(),
                Some(project.clone()),
                None,
                [AutomationPermission::ProjectRead],
            )
            .expect("scope");
            let issue = service
                .issue(
                    AutomationTokenId::parse("atm_example00").expect("token id"),
                    "CI read token",
                    scope,
                    DeveloperIdentityId::parse("dev_example00").expect("developer"),
                    10,
                    100,
                )
                .await
                .expect("issue");
            assert!(
                !serde_json::to_string(&issue.record)
                    .expect("serialize")
                    .contains(issue.secret.expose_once())
            );
            let principal = service
                .authenticate(&issue.secret, 20)
                .await
                .expect("authenticate");
            assert!(principal.allows(
                AutomationPermission::ProjectRead,
                &ManagementResourceScope {
                    organization_id: organization.clone(),
                    project_id: Some(project),
                    environment_id: None,
                }
            ));
            assert!(!principal.allows(
                AutomationPermission::ProjectWrite,
                &ManagementResourceScope {
                    organization_id: organization,
                    project_id: ProjectId::parse("prj_other0000").ok(),
                    environment_id: None,
                }
            ));
            let replacement = service
                .rotate(
                    issue.record.id(),
                    AutomationTokenId::parse("atm_replaced00").expect("replacement id"),
                    30,
                    200,
                )
                .await
                .expect("rotate");
            assert!(service.authenticate(&issue.secret, 31).await.is_err());
            service
                .authenticate(&replacement.secret, 31)
                .await
                .expect("replacement authenticates");
            service
                .revoke(replacement.record.id(), 40)
                .await
                .expect("revoke replacement");
            assert!(service.authenticate(&replacement.secret, 41).await.is_err());
            assert_eq!(
                format!("{:?}", issue.secret),
                "AutomationTokenSecret([REDACTED])"
            );
        });
    }
}
