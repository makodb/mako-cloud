use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
// The wire model these services exchange lives in the shared contract crate,
// and is re-exported here so every existing importer is unaffected.
use mako_api::TenantScope;
pub use mako_api::{
    EventOutcome, HealthState, ObservabilityPage, ObservabilityPayload, ObservabilityQuery,
    ObservabilityQueryError, ObservabilityRecord, ObservabilitySignal, RetentionWindow,
};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink,
    DeveloperPrincipal, OrganizationId, OrganizationStore, OrganizationStoreError, ProjectStore,
    ProjectStoreError, is_read_action, team_audit_tenant,
};

/// Backend pages read at most to fill one page of a team's changes.
const MAX_TEAM_ACTIVITY_SCANS: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservabilityBackendError {
    Unavailable,
    CursorExpired {
        retained_from_unix_milliseconds: u64,
    },
}

impl fmt::Display for ObservabilityBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "observability backend is unavailable",
            Self::CursorExpired { .. } => "observability cursor is outside retention",
        })
    }
}

impl Error for ObservabilityBackendError {}

#[async_trait]
pub trait ObservabilityBackend: Send + Sync {
    async fn query(
        &self,
        tenant: &TenantScope,
        signal: ObservabilitySignal,
        query: &ObservabilityQuery,
    ) -> Result<ObservabilityPage, ObservabilityBackendError>;
}

#[derive(Clone)]
pub struct ObservabilityService {
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    backend: Arc<dyn ObservabilityBackend>,
}

impl fmt::Debug for ObservabilityService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservabilityService")
            .finish_non_exhaustive()
    }
}

impl ObservabilityService {
    #[must_use]
    pub fn new(
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        backend: Arc<dyn ObservabilityBackend>,
    ) -> Self {
        Self {
            projects,
            organizations,
            audit,
            backend,
        }
    }

    pub async fn query_usage(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::Usage,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_quotas(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::Quota,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_health(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::Health,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_replication_errors(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::ReplicationError,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_authentication_events(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::AuthenticationEvent,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_function_metrics(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::FunctionMetric,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_logs(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::ProjectLog,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    pub async fn query_index_states(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::IndexState,
            query,
            now_unix_milliseconds,
        )
        .await
    }

    /// An environment's audit events. With `changes_only`, reads are left out
    /// as for a team's activity: every visit to the Activity page records
    /// reads of its own, and they used to push the changes a reader came for
    /// off the page.
    pub async fn query_audit_events(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        changes_only: bool,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        let page = self
            .query_signal(
                actor,
                tenant,
                ObservabilitySignal::Audit,
                query,
                now_unix_milliseconds,
            )
            .await?;
        if !changes_only {
            return Ok(page);
        }
        self.keep_changes(tenant, query, page, now_unix_milliseconds)
            .await
    }

    /// A team's own activity: its invitations, membership and role changes,
    /// automation tokens, and project-level actions, newest first when asked.
    /// Any member of the team may read it, as they may read an environment's
    /// audit events. With `changes_only`, reads are left out and further
    /// backend pages are read to fill the page; a page is only ever taken
    /// whole, so paging on never skips an event.
    pub async fn query_team_activity(
        &self,
        actor: &DeveloperPrincipal,
        team: &OrganizationId,
        query: &ObservabilityQuery,
        changes_only: bool,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        query.validate(now_unix_milliseconds)?;
        let target = format!("{}/activity", team.as_str());
        if self
            .organizations
            .get_membership(team, actor.identity_id())
            .await?
            .is_none()
        {
            self.audit.record(ControlAuditEvent {
                organization_id: team.clone(),
                actor_id: actor.identity_id().clone(),
                action: ControlAuditAction::ObservabilityRead,
                target,
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_milliseconds / 1_000,
            });
            return Err(ObservabilityError::Forbidden);
        }
        let tenant = team_audit_tenant(team).ok_or(ObservabilityError::InvalidBackendResponse)?;
        let mut page = self
            .backend
            .query(&tenant, ObservabilitySignal::Audit, query)
            .await?;
        validate_page(
            &page,
            &tenant,
            ObservabilitySignal::Audit,
            query,
            now_unix_milliseconds,
        )?;
        if changes_only {
            page = self
                .keep_changes(&tenant, query, page, now_unix_milliseconds)
                .await?;
        }
        self.audit.record(ControlAuditEvent {
            organization_id: team.clone(),
            actor_id: actor.identity_id().clone(),
            action: ControlAuditAction::ObservabilityRead,
            target,
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds: now_unix_milliseconds / 1_000,
        });
        Ok(page)
    }

    /// Leaves reads out of an audit page and reads further backend pages to
    /// fill it; a page is only ever taken whole, so paging on never skips an
    /// event.
    async fn keep_changes(
        &self,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        mut page: ObservabilityPage,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        let mut items = Vec::with_capacity(query.limit);
        let mut next = page.next_cursor.clone();
        items.extend(page.items.drain(..).filter(is_change));
        let mut scans = 1;
        while items.len() < query.limit && scans < MAX_TEAM_ACTIVITY_SCANS {
            let Some(cursor) = next.clone() else { break };
            let continued = ObservabilityQuery {
                cursor: Some(cursor),
                ..query.clone()
            };
            let more = self
                .backend
                .query(tenant, ObservabilitySignal::Audit, &continued)
                .await?;
            validate_page(
                &more,
                tenant,
                ObservabilitySignal::Audit,
                &continued,
                now_unix_milliseconds,
            )?;
            let changes: Vec<_> = more.items.into_iter().filter(is_change).collect();
            if items.len() + changes.len() > query.limit {
                break;
            }
            items.extend(changes);
            next = more.next_cursor;
            page.retention = more.retention;
            scans += 1;
        }
        page.items = items;
        page.next_cursor = next;
        Ok(page)
    }

    async fn query_signal(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        signal: ObservabilitySignal,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        query.validate(now_unix_milliseconds)?;
        let organization = self.authorize(actor, tenant, now_unix_milliseconds).await?;
        let page = self.backend.query(tenant, signal, query).await?;
        validate_page(&page, tenant, signal, query, now_unix_milliseconds)?;
        self.audit.record(ControlAuditEvent {
            organization_id: organization,
            actor_id: actor.identity_id().clone(),
            action: ControlAuditAction::ObservabilityRead,
            target: format!(
                "{}/{}/{signal:?}",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str()
            ),
            outcome: ControlAuditOutcome::Allowed,
            at_unix_seconds: now_unix_milliseconds / 1_000,
        });
        Ok(page)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_milliseconds: u64,
    ) -> Result<OrganizationId, ObservabilityError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(ObservabilityError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(ObservabilityError::NotFound)?;
        if self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?
            .is_none()
        {
            self.audit.record(ControlAuditEvent {
                organization_id: project.organization_id().clone(),
                actor_id: actor.identity_id().clone(),
                action: ControlAuditAction::ObservabilityRead,
                target: format!(
                    "{}/{}/authorization",
                    tenant.project_id().as_str(),
                    tenant.environment_id().as_str()
                ),
                outcome: ControlAuditOutcome::Denied,
                at_unix_seconds: now_unix_milliseconds / 1_000,
            });
            return Err(ObservabilityError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }
}

fn is_change(record: &ObservabilityRecord) -> bool {
    !matches!(&record.payload, ObservabilityPayload::Audit { action, .. } if is_read_action(action))
}

/// A page is observed after the caller captured the time it validates against:
/// the service reads the clock when the request arrives, and the backend reads
/// it again when it asks the telemetry service, which echoes that later reading
/// back as the page's observation time. Without this tolerance every page looks
/// like it was observed in the future and no observability query can succeed.
const MAX_OBSERVATION_SKEW_MILLISECONDS: u64 = 30_000;

fn validate_page(
    page: &ObservabilityPage,
    tenant: &TenantScope,
    signal: ObservabilitySignal,
    query: &ObservabilityQuery,
    now_unix_milliseconds: u64,
) -> Result<(), ObservabilityError> {
    let retention = &page.retention;
    if page.items.len() > query.limit
        || retention.retention_seconds == 0
        || retention.retained_from_unix_milliseconds > retention.observed_at_unix_milliseconds
        || retention.observed_at_unix_milliseconds
            > now_unix_milliseconds.saturating_add(MAX_OBSERVATION_SKEW_MILLISECONDS)
        || page.next_cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control)
        })
    {
        return Err(ObservabilityError::InvalidBackendResponse);
    }
    if query
        .from_unix_milliseconds
        .is_some_and(|from| from < retention.retained_from_unix_milliseconds)
    {
        return Err(ObservabilityError::RetentionExpired {
            retained_from_unix_milliseconds: retention.retained_from_unix_milliseconds,
        });
    }
    if page.items.iter().any(|record| {
        record.tenant != *tenant
            || record.payload.signal() != signal
            || !record.payload.is_safe()
            || record.timestamp_unix_milliseconds < retention.retained_from_unix_milliseconds
            || record.timestamp_unix_milliseconds > retention.observed_at_unix_milliseconds
            || query
                .from_unix_milliseconds
                .is_some_and(|from| record.timestamp_unix_milliseconds < from)
            || query
                .until_unix_milliseconds
                .is_some_and(|until| record.timestamp_unix_milliseconds > until)
    }) {
        return Err(ObservabilityError::InvalidBackendResponse);
    }
    Ok(())
}

#[derive(Debug)]
pub enum ObservabilityError {
    NotFound,
    Forbidden,
    InvalidQuery,
    RetentionExpired {
        retained_from_unix_milliseconds: u64,
    },
    InvalidBackendResponse,
    Backend(ObservabilityBackendError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
}

impl fmt::Display for ObservabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "observability scope was not found",
            Self::Forbidden => "observability query is forbidden",
            Self::InvalidQuery => "observability query is invalid",
            Self::RetentionExpired { .. } => "observability query is outside retention",
            Self::InvalidBackendResponse => "observability backend response is invalid",
            Self::Backend(_) => "observability backend query failed",
            Self::Project(_) => "observability project lookup failed",
            Self::Organization(_) => "observability organization lookup failed",
        })
    }
}

impl Error for ObservabilityError {}

impl From<ObservabilityQueryError> for ObservabilityError {
    fn from(_: ObservabilityQueryError) -> Self {
        Self::InvalidQuery
    }
}

impl From<ObservabilityBackendError> for ObservabilityError {
    fn from(error: ObservabilityBackendError) -> Self {
        match error {
            ObservabilityBackendError::CursorExpired {
                retained_from_unix_milliseconds,
            } => Self::RetentionExpired {
                retained_from_unix_milliseconds,
            },
            error => Self::Backend(error),
        }
    }
}

impl From<ProjectStoreError> for ObservabilityError {
    fn from(error: ProjectStoreError) -> Self {
        Self::Project(error)
    }
}

impl From<OrganizationStoreError> for ObservabilityError {
    fn from(error: OrganizationStoreError) -> Self {
        Self::Organization(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::{Durability, KvAdapter, MemoryAdapter};

    use super::*;
    use crate::{
        DeveloperIdentityId, EnvironmentRecord, MembershipRecord, OrganizationRecord,
        OrganizationRole, ProjectRecord,
    };
    use mako_api::QuotaResource;

    struct Backend {
        response: Mutex<ObservabilityPage>,
    }

    #[async_trait]
    impl ObservabilityBackend for Backend {
        async fn query(
            &self,
            _: &TenantScope,
            _: ObservabilitySignal,
            _: &ObservabilityQuery,
        ) -> Result<ObservabilityPage, ObservabilityBackendError> {
            Ok(self.response.lock().expect("response").clone())
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<ControlAuditEvent>>);

    impl ControlAuditSink for Audit {
        fn record(&self, event: ControlAuditEvent) {
            self.0.lock().expect("audit").push(event);
        }
    }

    #[test]
    fn queries_are_tenant_scoped_bounded_and_retention_aware() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects = ProjectStore::new(adapter, Durability::Memory).expect("projects");
            let organization = OrganizationId::parse("org_example00").expect("organization");
            let developer = DeveloperIdentityId::parse("dev_example00").expect("developer");
            organizations
                .create_organization(
                    &OrganizationRecord::new(organization.clone(), "Example", 1)
                        .expect("organization"),
                    &MembershipRecord::new(
                        organization.clone(),
                        developer.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("organization");
            let project = ProjectId::parse("prj_example00").expect("project");
            let environment = EnvironmentId::parse("env_example00").expect("environment");
            projects
                .create_project(
                    &ProjectRecord::new(project.clone(), organization, "Mako", "local", 1)
                        .expect("project"),
                )
                .await
                .expect("project");
            projects
                .create_environment(
                    &EnvironmentRecord::new(environment.clone(), project.clone(), "Dev", 1)
                        .expect("environment"),
                )
                .await
                .expect("environment");
            let tenant = TenantScope::new(project, environment);
            let backend = Arc::new(Backend {
                response: Mutex::new(ObservabilityPage {
                    items: vec![ObservabilityRecord {
                        tenant: tenant.clone(),
                        timestamp_unix_milliseconds: 9_500,
                        payload: ObservabilityPayload::Usage {
                            resource: QuotaResource::StorageBytes,
                            quantity: 42,
                            unit: "bytes".to_owned(),
                        },
                    }],
                    next_cursor: Some("cursor_2".to_owned()),
                    retention: RetentionWindow {
                        retained_from_unix_milliseconds: 5_000,
                        observed_at_unix_milliseconds: 10_000,
                        retention_seconds: 5,
                    },
                }),
            });
            let audit = Arc::new(Audit::default());
            let service =
                ObservabilityService::new(projects, organizations, audit.clone(), backend.clone());
            let actor = DeveloperPrincipal::for_test(developer, "viewer@example.test");
            let query = ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: Some(5_000),
                until_unix_milliseconds: Some(10_000),
                limit: 10,
                newest_first: false,
            };
            let page = service
                .query_usage(&actor, &tenant, &query, 10_000)
                .await
                .expect("usage");
            assert_eq!(page.items.len(), 1);
            assert_eq!(audit.0.lock().expect("audit").len(), 1);

            let expired = ObservabilityQuery {
                from_unix_milliseconds: Some(4_999),
                ..query.clone()
            };
            assert!(matches!(
                service.query_usage(&actor, &tenant, &expired, 10_000).await,
                Err(ObservabilityError::RetentionExpired { .. })
            ));

            backend.response.lock().expect("response").items[0].tenant = TenantScope::new(
                ProjectId::parse("prj_other000").expect("project"),
                EnvironmentId::parse("env_other000").expect("environment"),
            );
            assert!(matches!(
                service.query_usage(&actor, &tenant, &query, 10_000).await,
                Err(ObservabilityError::InvalidBackendResponse)
            ));
        });
    }

    /// Serves pages by cursor: "" is the first page, "p1" the next, and so on.
    struct PagedBackend {
        pages: Vec<Vec<&'static str>>,
        tenant: TenantScope,
        queried: Mutex<Vec<(TenantScope, Option<String>)>>,
    }

    #[async_trait]
    impl ObservabilityBackend for PagedBackend {
        async fn query(
            &self,
            tenant: &TenantScope,
            _: ObservabilitySignal,
            query: &ObservabilityQuery,
        ) -> Result<ObservabilityPage, ObservabilityBackendError> {
            self.queried
                .lock()
                .expect("queried")
                .push((tenant.clone(), query.cursor.clone()));
            let index = query
                .cursor
                .as_deref()
                .map_or(0, |cursor| cursor[1..].parse().expect("cursor"));
            Ok(ObservabilityPage {
                items: self.pages[index]
                    .iter()
                    .enumerate()
                    .map(|(offset, action)| ObservabilityRecord {
                        tenant: self.tenant.clone(),
                        timestamp_unix_milliseconds: 9_000 - (index * 10 + offset) as u64,
                        payload: ObservabilityPayload::Audit {
                            organization_id: "org_websiteteam".to_owned(),
                            actor_id: "dev_example00".to_owned(),
                            action: (*action).to_owned(),
                            target: format!("control_resource/res_{index}{offset}"),
                            outcome: EventOutcome::Allowed,
                            request_id: format!("req_{index}{offset}"),
                            details: None,
                        },
                    })
                    .collect(),
                next_cursor: (index + 1 < self.pages.len()).then(|| format!("p{}", index + 1)),
                retention: RetentionWindow {
                    retained_from_unix_milliseconds: 5_000,
                    observed_at_unix_milliseconds: 10_000,
                    retention_seconds: 5,
                },
            })
        }
    }

    #[test]
    fn an_environments_audit_can_leave_its_reads_out() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects = ProjectStore::new(adapter, Durability::Memory).expect("projects");
            let team = OrganizationId::parse("org_websiteteam").expect("team");
            let member = DeveloperIdentityId::parse("dev_example00").expect("member");
            organizations
                .create_organization(
                    &OrganizationRecord::new(team.clone(), "Website", 1).expect("team"),
                    &MembershipRecord::new(
                        team.clone(),
                        member.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("team");
            let project = mako_api::ProjectId::parse("prj_example00").expect("project");
            let environment = mako_api::EnvironmentId::parse("env_example00").expect("environment");
            projects
                .create_project(
                    &crate::ProjectRecord::new(project.clone(), team, "Site", "local", 1)
                        .expect("project"),
                )
                .await
                .expect("project");
            projects
                .create_environment(
                    &crate::EnvironmentRecord::new(
                        environment.clone(),
                        project.clone(),
                        "Development",
                        1,
                    )
                    .expect("environment"),
                )
                .await
                .expect("environment");
            let tenant = TenantScope::new(project, environment);
            // What the Activity page itself leaves behind: reads around the changes.
            let backend = Arc::new(PagedBackend {
                pages: vec![
                    vec![
                        "observability_read",
                        "collection_read",
                        "observability_read",
                    ],
                    vec!["policy_activate", "observability_read", "function_deploy"],
                ],
                tenant: tenant.clone(),
                queried: Mutex::new(Vec::new()),
            });
            let service = ObservabilityService::new(
                projects,
                organizations,
                Arc::new(Audit::default()),
                backend,
            );
            let actor = DeveloperPrincipal::for_test(member, "developer@example.test");
            let query = ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: Some(5_000),
                until_unix_milliseconds: Some(10_000),
                limit: 3,
                newest_first: true,
            };
            let everything = service
                .query_audit_events(&actor, &tenant, &query, false, 10_000)
                .await
                .expect("everything");
            assert_eq!(everything.items.len(), 3, "the first page is all reads");
            let changes = service
                .query_audit_events(&actor, &tenant, &query, true, 10_000)
                .await
                .expect("changes");
            let actions: Vec<_> = changes
                .items
                .iter()
                .map(|record| match &record.payload {
                    ObservabilityPayload::Audit { action, .. } => action.clone(),
                    other => panic!("unexpected payload {other:?}"),
                })
                .collect();
            assert_eq!(actions, ["policy_activate", "function_deploy"]);
            assert_eq!(changes.next_cursor, None);
        });
    }

    #[test]
    fn team_activity_is_for_members_and_can_leave_reads_out_without_skipping_changes() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let organizations =
                OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
            let projects = ProjectStore::new(adapter, Durability::Memory).expect("projects");
            let team = OrganizationId::parse("org_websiteteam").expect("team");
            let member = DeveloperIdentityId::parse("dev_example00").expect("member");
            organizations
                .create_organization(
                    &OrganizationRecord::new(team.clone(), "Website", 1).expect("team"),
                    &MembershipRecord::new(
                        team.clone(),
                        member.clone(),
                        OrganizationRole::Owner,
                        1,
                    ),
                )
                .await
                .expect("team");
            let tenant = team_audit_tenant(&team).expect("tenant");
            let backend = Arc::new(PagedBackend {
                pages: vec![
                    vec!["membership_list", "membership_update", "organization_read"],
                    vec!["membership_list", "membership_list", "invitation_create"],
                    vec![
                        "membership_delete",
                        "automation_token_revoke",
                        "invitation_accept",
                    ],
                    vec!["organization_update"],
                ],
                tenant: tenant.clone(),
                queried: Mutex::new(Vec::new()),
            });
            let audit = Arc::new(Audit::default());
            let service =
                ObservabilityService::new(projects, organizations, audit.clone(), backend.clone());
            let query = ObservabilityQuery {
                cursor: None,
                from_unix_milliseconds: Some(5_000),
                until_unix_milliseconds: Some(10_000),
                limit: 3,
                newest_first: true,
            };
            let outsider = DeveloperPrincipal::for_test(
                DeveloperIdentityId::parse("dev_outsider0").expect("outsider"),
                "outsider@example.test",
            );
            assert!(matches!(
                service
                    .query_team_activity(&outsider, &team, &query, false, 10_000)
                    .await,
                Err(ObservabilityError::Forbidden)
            ));
            assert!(
                backend.queried.lock().expect("queried").is_empty(),
                "nothing is read for a non-member"
            );

            let actor = DeveloperPrincipal::for_test(member, "viewer@example.test");
            let all = service
                .query_team_activity(&actor, &team, &query, false, 10_000)
                .await
                .expect("all");
            assert_eq!(all.items.len(), 3);
            assert_eq!(all.next_cursor.as_deref(), Some("p1"));

            // Changes only: page 0 gives one, page 1 one more; page 2's three
            // would overflow the limit, so the page stops before it and the
            // cursor resumes there.
            let changes = service
                .query_team_activity(&actor, &team, &query, true, 10_000)
                .await
                .expect("changes");
            let actions: Vec<_> = changes
                .items
                .iter()
                .map(|record| match &record.payload {
                    ObservabilityPayload::Audit { action, .. } => action.clone(),
                    other => panic!("unexpected payload {other:?}"),
                })
                .collect();
            assert_eq!(actions, ["membership_update", "invitation_create"]);
            assert_eq!(changes.next_cursor.as_deref(), Some("p2"));
            let resumed = service
                .query_team_activity(
                    &actor,
                    &team,
                    &ObservabilityQuery {
                        cursor: changes.next_cursor.clone(),
                        ..query.clone()
                    },
                    true,
                    10_000,
                )
                .await
                .expect("resumed");
            assert_eq!(
                resumed.items.len(),
                3,
                "the events after the cursor are all returned"
            );
            assert!(
                backend
                    .queried
                    .lock()
                    .expect("queried")
                    .iter()
                    .all(|(queried, _)| *queried == tenant),
                "only the team's own audit tenant is read"
            );
            assert!(
                audit
                    .0
                    .lock()
                    .expect("audit")
                    .iter()
                    .any(|event| event.outcome == ControlAuditOutcome::Denied)
            );
        });
    }
}
