//! An environment's cross-origin allowlist: which browser origins may call
//! its application API.
//!
//! A browser refuses a cross-origin response unless the server names the
//! origin it came from. The list of origins that may be named is a property
//! of the **environment**, not of a hostname: it applies on the platform's
//! own hostname, where every project's API lives, and on every custom
//! domain the environment is served on. An application therefore needs no
//! domain of its own before its browser client can call it.
//!
//! The control plane stores the list and the data plane enforces it, so the
//! list travels the same way an environment's quota policy does: written
//! here, installed there, and refused at both ends unless every origin is
//! exact. Nothing here decides *authorization* -- a cross-origin refusal is
//! a browser's, not the platform's -- so the list is a safety property of
//! the application's own pages, never a credential.

use std::{error::Error, fmt, sync::Arc};

use mako_api::TenantScope;
use mako_internal_rpc::{MAXIMUM_ALLOWED_ORIGINS, MAXIMUM_ORIGIN_BYTES, is_allowed_origin};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperPrincipal, OrganizationId, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
};

/// The most origins one environment may allow.
pub const MAXIMUM_ORIGINS: usize = MAXIMUM_ALLOWED_ORIGINS;

/// The stored list for one environment. The absence of a record means no
/// origin is allowed, which is what an environment that has never set one
/// carries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AllowedOriginsRecord {
    tenant: TenantScope,
    origins: Vec<String>,
    version: u64,
    updated_at_unix_seconds: u64,
}

impl AllowedOriginsRecord {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }
}

/// The origins an environment may list: at most [`MAXIMUM_ORIGINS`], each an
/// exact origin (`https://host` or `https://host:port`, lowercase host, no
/// path or trailing slash) with `http` accepted only for a loopback host,
/// unique after normalization. Returns the normalized list in the order
/// given, so what is stored is exactly what is compared against.
pub fn validate_allowed_origins(values: &[String]) -> Result<Vec<String>, AllowedOriginsError> {
    let invalid = AllowedOriginsError::InvalidOrigin;
    if values.len() > MAXIMUM_ORIGINS {
        return Err(invalid("an environment may allow at most 16 origins"));
    }
    let mut origins = Vec::with_capacity(values.len());
    for value in values {
        let origin = value.trim().to_ascii_lowercase();
        if origin.len() > MAXIMUM_ORIGIN_BYTES {
            return Err(invalid("origin must be at most 262 characters"));
        }
        if origin.ends_with('/') {
            return Err(invalid("origin must not end with a slash"));
        }
        if !is_allowed_origin(&origin) {
            return Err(invalid(
                "origin must be https://host or https://host:port with a lowercase DNS name and no path; http is allowed only for a loopback host",
            ));
        }
        if origins.contains(&origin) {
            return Err(invalid("origins must be unique"));
        }
        origins.push(origin);
    }
    Ok(origins)
}

/// Reads and replaces one environment's allowlist, under the same
/// membership rules as its other environment-scoped settings: any member
/// may read; a role that can change projects may write.
#[derive(Clone)]
pub struct AllowedOriginsService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for AllowedOriginsService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AllowedOriginsService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl AllowedOriginsService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Result<Self, AllowedOriginsError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(AllowedOriginsError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
        })
    }

    /// The list a developer sees; an environment that has never set one
    /// reads back empty.
    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<Vec<String>, AllowedOriginsError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::AllowedOriginsRead,
                false,
                now_unix_seconds,
            )
            .await?;
        let origins = self
            .stored(tenant)
            .await?
            .map_or_else(Vec::new, |record| record.origins);
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::AllowedOriginsRead,
            false,
            now_unix_seconds,
        );
        Ok(origins)
    }

    /// Replaces the list whole and returns what is now stored. The caller
    /// installs it into the data plane afterwards: this is the developer's
    /// intent, and a retry of the same `PUT` re-installs it.
    pub async fn replace(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        origins: &[String],
        now_unix_seconds: u64,
    ) -> Result<Vec<String>, AllowedOriginsError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::AllowedOriginsUpdate,
                true,
                now_unix_seconds,
            )
            .await?;
        let origins = validate_allowed_origins(origins)?;
        let key =
            ControlKeyspace::allowed_origins_key(tenant.project_id(), tenant.environment_id())?;
        let previous = self.stored(tenant).await?;
        let condition = match &previous {
            Some(previous) => KeyCondition::ValueEquals {
                key: key.clone(),
                value: serde_json::to_vec(previous)?,
            },
            None => KeyCondition::Missing { key: key.clone() },
        };
        let next = AllowedOriginsRecord {
            tenant: tenant.clone(),
            origins: origins.clone(),
            version: previous
                .as_ref()
                .map_or(0, AllowedOriginsRecord::version)
                .checked_add(1)
                .ok_or(AllowedOriginsError::CorruptRecord)?,
            updated_at_unix_seconds: now_unix_seconds,
        };
        let mut batch = WriteBatch::with_capacity(1);
        batch.put(&key, serde_json::to_vec(&next)?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![condition],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => {}
            CompareAndWriteResult::Conflict { .. } => return Err(AllowedOriginsError::Conflict),
        }
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::AllowedOriginsUpdate,
            false,
            now_unix_seconds,
        );
        Ok(origins)
    }

    /// The installed list, with no authorization: the caller is the
    /// platform's own resolution path answering the edge gateway, not a
    /// developer.
    pub async fn installed(
        &self,
        tenant: &TenantScope,
    ) -> Result<Vec<String>, AllowedOriginsError> {
        Ok(self
            .stored(tenant)
            .await?
            .map_or_else(Vec::new, |record| record.origins))
    }

    async fn stored(
        &self,
        tenant: &TenantScope,
    ) -> Result<Option<AllowedOriginsRecord>, AllowedOriginsError> {
        let key =
            ControlKeyspace::allowed_origins_key(tenant.project_id(), tenant.environment_id())?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: AllowedOriginsRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant {
            return Err(AllowedOriginsError::CorruptRecord);
        }
        Ok(Some(record))
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        action: ControlAuditAction,
        write: bool,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, AllowedOriginsError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(AllowedOriginsError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(AllowedOriginsError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        let allowed =
            membership.is_some_and(|membership| !write || membership.role().can_mutate_projects());
        if !allowed {
            self.audit(
                actor,
                project.organization_id(),
                tenant,
                action,
                true,
                now_unix_seconds,
            );
            return Err(AllowedOriginsError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationId,
        tenant: &TenantScope,
        action: ControlAuditAction,
        denied: bool,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}/allowed-origins",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str()
            ),
            outcome: if denied {
                ControlAuditOutcome::Denied
            } else {
                ControlAuditOutcome::Allowed
            },
            at_unix_seconds,
        });
    }
}

#[derive(Debug)]
pub enum AllowedOriginsError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    /// An origin was refused; the message says why, for the developer.
    InvalidOrigin(&'static str),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for AllowedOriginsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDurability => {
                formatter.write_str("allowed origin durability is unsupported")
            }
            Self::NotFound => formatter.write_str("environment was not found"),
            Self::Forbidden => formatter.write_str("allowed origin operation is forbidden"),
            Self::Conflict => formatter.write_str("allowed origins changed concurrently"),
            Self::CorruptRecord => formatter.write_str("allowed origins record is corrupt"),
            Self::InvalidOrigin(message) => formatter.write_str(message),
            Self::Project(error) => write!(formatter, "project store failed: {error}"),
            Self::Organization(error) => write!(formatter, "organization store failed: {error}"),
            Self::Keyspace(error) => write!(formatter, "allowed origins key failed: {error}"),
            Self::Storage(error) => write!(formatter, "allowed origins storage failed: {error}"),
            Self::Json(error) => write!(formatter, "allowed origins record failed: {error}"),
        }
    }
}

impl Error for AllowedOriginsError {}

impl From<ProjectStoreError> for AllowedOriginsError {
    fn from(error: ProjectStoreError) -> Self {
        Self::Project(error)
    }
}

impl From<OrganizationStoreError> for AllowedOriginsError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Organization(error)
    }
}

impl From<ControlKeyspaceError> for AllowedOriginsError {
    fn from(error: ControlKeyspaceError) -> Self {
        Self::Keyspace(error)
    }
}

impl From<StorageError> for AllowedOriginsError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for AllowedOriginsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use crate::{
        ControlAuditOutcome,
        email_template::tests::{Fixture, NOW, fixture},
    };

    struct Harness {
        fixture: Fixture,
        service: AllowedOriginsService,
    }

    fn harness() -> Harness {
        let fixture = fixture();
        let kv: Arc<dyn KvAdapter> = fixture.adapter.clone();
        let service = AllowedOriginsService::new(
            kv.clone(),
            Durability::Memory,
            ProjectStore::new(kv.clone(), Durability::Memory).expect("projects"),
            OrganizationStore::new(kv, Durability::Memory).expect("organizations"),
            fixture.audit.clone(),
        )
        .expect("service");
        Harness { fixture, service }
    }

    fn origins(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// The list is exact: a browser sends an origin with no path and no
    /// trailing slash, and only that form can ever be matched or echoed.
    #[test]
    fn allowed_origins_are_exact_unique_bounded_and_https_unless_loopback() {
        assert_eq!(
            validate_allowed_origins(&origins(&[
                "  HTTPS://App.Example.com  ",
                "https://app.example.com:8443",
                "http://localhost:5173",
                "http://127.0.0.1:5173",
            ]))
            .expect("valid"),
            [
                "https://app.example.com",
                "https://app.example.com:8443",
                "http://localhost:5173",
                "http://127.0.0.1:5173",
            ],
            "origins are normalized and kept in the order given"
        );
        assert!(validate_allowed_origins(&[]).expect("none").is_empty());
        for invalid in [
            "app.example.com",
            "https://app.example.com/",
            "https://app.example.com/path",
            "https://app.example.com?q=1",
            "https://*.example.com",
            "https://app_1.example.com",
            "ftp://app.example.com",
            "http://app.example.com",
            "https://app.example.com:0",
            "null",
            "*",
        ] {
            assert!(
                matches!(
                    validate_allowed_origins(&origins(&[invalid])),
                    Err(AllowedOriginsError::InvalidOrigin(_))
                ),
                "{invalid:?} must be refused"
            );
        }
        assert!(
            matches!(
                validate_allowed_origins(&origins(&[
                    "https://app.example.com",
                    "https://APP.example.com",
                ])),
                Err(AllowedOriginsError::InvalidOrigin(_))
            ),
            "an origin may not be listed twice under a different case"
        );
        let too_many = (0..=MAXIMUM_ORIGINS)
            .map(|index| format!("https://app{index}.example.com"))
            .collect::<Vec<_>>();
        assert!(matches!(
            validate_allowed_origins(&too_many),
            Err(AllowedOriginsError::InvalidOrigin(_))
        ));
        assert_eq!(
            validate_allowed_origins(&too_many[1..])
                .expect("the bound itself is allowed")
                .len(),
            MAXIMUM_ORIGINS
        );
    }

    /// An environment starts with no cross-origin access, a member who may
    /// change projects replaces the list whole, every member may read it,
    /// and both are audited.
    #[test]
    fn the_list_is_read_by_members_replaced_by_writers_and_audited() {
        let harness = harness();
        let tenant = harness.fixture.tenant.clone();
        block_on(async {
            assert!(
                harness
                    .service
                    .get(&harness.fixture.owner, &tenant, NOW)
                    .await
                    .expect("read")
                    .is_empty(),
                "an environment allows no origin until one is set"
            );
            assert!(
                harness
                    .service
                    .installed(&tenant)
                    .await
                    .expect("installed")
                    .is_empty()
            );
            let stored = harness
                .service
                .replace(
                    &harness.fixture.owner,
                    &tenant,
                    &origins(&["https://app.example.com", "http://127.0.0.1:5173"]),
                    NOW + 1,
                )
                .await
                .expect("replaced");
            assert_eq!(stored, ["https://app.example.com", "http://127.0.0.1:5173"]);
            assert_eq!(
                harness
                    .service
                    .get(&harness.fixture.viewer, &tenant, NOW + 2)
                    .await
                    .expect("a viewer reads"),
                stored
            );
            assert_eq!(
                harness.service.installed(&tenant).await.expect("installed"),
                stored,
                "the resolution path sees the same list without a developer"
            );
            // A replacement is whole: what is not in the new list is gone.
            assert_eq!(
                harness
                    .service
                    .replace(
                        &harness.fixture.owner,
                        &tenant,
                        &origins(&["https://app.example.com"]),
                        NOW + 3,
                    )
                    .await
                    .expect("narrowed"),
                ["https://app.example.com"]
            );
            assert!(
                harness
                    .service
                    .replace(&harness.fixture.owner, &tenant, &[], NOW + 4)
                    .await
                    .expect("cleared")
                    .is_empty(),
                "an empty list withdraws cross-origin access"
            );
            // A viewer may not write, and a stranger may not even read.
            assert!(matches!(
                harness
                    .service
                    .replace(
                        &harness.fixture.viewer,
                        &tenant,
                        &origins(&["https://app.example.com"]),
                        NOW + 5,
                    )
                    .await
                    .expect_err("a viewer may not write"),
                AllowedOriginsError::Forbidden
            ));
            assert!(matches!(
                harness
                    .service
                    .get(&harness.fixture.stranger, &tenant, NOW + 5)
                    .await
                    .expect_err("a stranger may not read"),
                AllowedOriginsError::Forbidden
            ));
            // A refused origin never reaches storage.
            assert!(matches!(
                harness
                    .service
                    .replace(
                        &harness.fixture.owner,
                        &tenant,
                        &origins(&["http://app.example.com"]),
                        NOW + 6,
                    )
                    .await
                    .expect_err("plaintext, not loopback"),
                AllowedOriginsError::InvalidOrigin(_)
            ));
            assert!(
                harness
                    .service
                    .installed(&tenant)
                    .await
                    .expect("installed")
                    .is_empty()
            );
            let audit = harness.fixture.audit.0.lock().expect("audit");
            let actions = |action: &str, outcome: ControlAuditOutcome| {
                audit
                    .iter()
                    .filter(|event| event.action.as_str() == action && event.outcome == outcome)
                    .count()
            };
            assert_eq!(
                actions("allowed_origins_read", ControlAuditOutcome::Allowed),
                2,
                "the owner's read and the viewer's; the resolution path audits nothing"
            );
            assert_eq!(
                actions("allowed_origins_update", ControlAuditOutcome::Allowed),
                3,
                "a refused origin is not an update"
            );
            assert_eq!(
                actions("allowed_origins_update", ControlAuditOutcome::Denied),
                1
            );
            assert_eq!(
                actions("allowed_origins_read", ControlAuditOutcome::Denied),
                1
            );
        });
    }
}
