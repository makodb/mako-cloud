use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use mako_api::TenantScope;
use serde::{Deserialize, Serialize};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink,
    DeveloperPrincipal, OrganizationId, OrganizationStore, OrganizationStoreError, ProjectStore,
    ProjectStoreError, QuotaResource,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilitySignal {
    Usage,
    Quota,
    Health,
    ReplicationError,
    AuthenticationEvent,
    FunctionMetric,
    ProjectLog,
    IndexState,
    Audit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityQuery {
    pub cursor: Option<String>,
    pub from_unix_milliseconds: Option<u64>,
    pub until_unix_milliseconds: Option<u64>,
    pub limit: usize,
}

impl ObservabilityQuery {
    fn validate(&self, now_unix_milliseconds: u64) -> Result<(), ObservabilityError> {
        if self.limit == 0
            || self.limit > 1_000
            || self.cursor.as_ref().is_some_and(|cursor| {
                cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control)
            })
            || self
                .from_unix_milliseconds
                .is_some_and(|from| from > now_unix_milliseconds)
            || self
                .until_unix_milliseconds
                .is_some_and(|until| until > now_unix_milliseconds)
            || matches!(
                (self.from_unix_milliseconds, self.until_unix_milliseconds),
                (Some(from), Some(until)) if from > until
            )
        {
            return Err(ObservabilityError::InvalidQuery);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetentionWindow {
    pub retained_from_unix_milliseconds: u64,
    pub observed_at_unix_milliseconds: u64,
    pub retention_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservabilityPayload {
    Usage {
        resource: QuotaResource,
        quantity: u64,
        unit: String,
    },
    Quota {
        resource: QuotaResource,
        limit: u64,
        consumed: u64,
        retry_after_unix_milliseconds: Option<u64>,
    },
    Health {
        service: String,
        region: String,
        status: HealthState,
        diagnostic: Option<String>,
    },
    ReplicationError {
        collection_id: String,
        category: String,
        retryable: bool,
        message: String,
        correlation_id: String,
    },
    AuthenticationEvent {
        category: String,
        outcome: EventOutcome,
        application_user_id: Option<String>,
        message: String,
        correlation_id: String,
    },
    FunctionMetric {
        function_name: String,
        version: u64,
        region: String,
        invocation_count: u64,
        error_count: u64,
        latency_milliseconds: u64,
        compute_milliseconds: u64,
    },
    ProjectLog {
        source: String,
        level: String,
        message: String,
        correlation_id: String,
    },
    IndexState {
        collection_id: String,
        index_name: String,
        index_version: u64,
        state: String,
        progress_percent: u8,
        message: Option<String>,
    },
    Audit {
        organization_id: String,
        actor_id: String,
        action: String,
        target: String,
        outcome: EventOutcome,
        request_id: String,
        details: Option<String>,
    },
}

impl ObservabilityPayload {
    #[must_use]
    pub const fn signal(&self) -> ObservabilitySignal {
        match self {
            Self::Usage { .. } => ObservabilitySignal::Usage,
            Self::Quota { .. } => ObservabilitySignal::Quota,
            Self::Health { .. } => ObservabilitySignal::Health,
            Self::ReplicationError { .. } => ObservabilitySignal::ReplicationError,
            Self::AuthenticationEvent { .. } => ObservabilitySignal::AuthenticationEvent,
            Self::FunctionMetric { .. } => ObservabilitySignal::FunctionMetric,
            Self::ProjectLog { .. } => ObservabilitySignal::ProjectLog,
            Self::IndexState { .. } => ObservabilitySignal::IndexState,
            Self::Audit { .. } => ObservabilitySignal::Audit,
        }
    }

    #[must_use]
    pub fn is_safe(&self) -> bool {
        match self {
            Self::Usage { unit, .. } => safe_text(unit, 64),
            Self::Quota { .. } => true,
            Self::Health {
                service,
                region,
                diagnostic,
                ..
            } => {
                safe_text(service, 128)
                    && safe_text(region, 64)
                    && diagnostic
                        .as_ref()
                        .is_none_or(|value| safe_text(value, 1_024))
            }
            Self::ReplicationError {
                collection_id,
                category,
                message,
                correlation_id,
                ..
            } => {
                safe_text(collection_id, 128)
                    && safe_text(category, 128)
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::AuthenticationEvent {
                category,
                application_user_id,
                message,
                correlation_id,
                ..
            } => {
                safe_text(category, 128)
                    && application_user_id
                        .as_ref()
                        .is_none_or(|value| safe_text(value, 128))
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::FunctionMetric {
                function_name,
                region,
                version,
                ..
            } => *version > 0 && safe_text(function_name, 63) && safe_text(region, 64),
            Self::ProjectLog {
                source,
                level,
                message,
                correlation_id,
            } => {
                safe_text(source, 128)
                    && safe_text(level, 32)
                    && safe_text(message, 4_096)
                    && safe_text(correlation_id, 128)
            }
            Self::IndexState {
                collection_id,
                index_name,
                index_version,
                state,
                progress_percent,
                message,
            } => {
                *index_version > 0
                    && *progress_percent <= 100
                    && safe_text(collection_id, 128)
                    && safe_text(index_name, 128)
                    && safe_text(state, 64)
                    && message.as_ref().is_none_or(|value| safe_text(value, 1_024))
            }
            Self::Audit {
                organization_id,
                actor_id,
                action,
                target,
                request_id,
                details,
                ..
            } => {
                safe_text(organization_id, 128)
                    && safe_text(actor_id, 128)
                    && safe_text(action, 128)
                    && safe_text(target, 1_024)
                    && safe_text(request_id, 128)
                    && details.as_ref().is_none_or(|value| safe_text(value, 2_048))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventOutcome {
    Allowed,
    Denied,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityRecord {
    pub tenant: TenantScope,
    pub timestamp_unix_milliseconds: u64,
    pub payload: ObservabilityPayload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservabilityPage {
    pub items: Vec<ObservabilityRecord>,
    pub next_cursor: Option<String>,
    pub retention: RetentionWindow,
}

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

    pub async fn query_audit_events(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        query: &ObservabilityQuery,
        now_unix_milliseconds: u64,
    ) -> Result<ObservabilityPage, ObservabilityError> {
        self.query_signal(
            actor,
            tenant,
            ObservabilitySignal::Audit,
            query,
            now_unix_milliseconds,
        )
        .await
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
        || retention.observed_at_unix_milliseconds > now_unix_milliseconds
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

fn safe_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
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
}
