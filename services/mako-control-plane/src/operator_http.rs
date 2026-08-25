use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    sync::Arc,
    time::{Instant, SystemTime},
};

use futures::executor::block_on;
use mako_api::{ErrorCode, ProjectId, RetryAdvice, TenantScope};
use mako_control_plane::{
    AbuseResponseId, AbuseTarget, GuardedMutation, IncidentSearch, IncidentState, InventoryKind,
    NewAbuseResponse, NewQuotaOverride, NewSupportSession, OperatorControlCenterError,
    OperatorEntitlementAdminInput, OperatorEntitlementChangeKind, OperatorError,
    OperatorPasswordAuthenticationError, OperatorPermission, OrganizationId, QuotaOverrideId,
    QuotaResource, RecoveryJobState, RecoveryRequest, SupportPermission, SupportSessionId,
    TenantSearch,
};
use mako_provisioning::{OperatorRepairAction, ProvisioningWorkflowId};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ControlPlaneGraph,
    http_support::{project_id, public_json, public_value, query_value, reject_unknown_query},
    management_http::{
        conflict, forbidden, invalid, no_payload, no_query, not_found, parse_json, require_json,
        unavailable,
    },
    operator_auth_http::authenticate_operator,
};

pub(crate) fn add_operator_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/operator/overview",
            handle_overview as Handler,
        ),
        (HttpMethod::Get, "/v1/operator/tenants", handle_tenants),
        (
            HttpMethod::Get,
            "/v1/operator/tenants/{projectId}",
            handle_tenant_360,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/security",
            handle_operator_security,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/security/identities/{developerIdentityId}/sessions/actions/revoke",
            handle_revoke_operator_sessions,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/security/entitlements/actions/plan",
            handle_plan_operator_entitlement,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/security/entitlements/actions/apply",
            handle_apply_operator_entitlement,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/inventory/{inventoryKind}",
            handle_inventory,
        ),
        (HttpMethod::Get, "/v1/operator/alerts", handle_alerts),
        (
            HttpMethod::Get,
            "/v1/operator/alerts/{alertFingerprint}",
            handle_alert,
        ),
        (HttpMethod::Get, "/v1/operator/incidents", handle_incidents),
        (
            HttpMethod::Get,
            "/v1/operator/incidents/{incidentId}",
            handle_incident,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/incidents",
            handle_create_incident,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/incidents/{incidentId}/actions/update",
            handle_update_incident,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/provisioning-workflows",
            handle_provisioning_inventory,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/projects/{projectId}/quota-overrides",
            handle_quota_override_inventory,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/quota-overrides/{quotaOverrideId}/actions/replace",
            handle_replace_quota_override,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/quota-overrides/{quotaOverrideId}/actions/revoke",
            handle_revoke_quota_override,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/projects/{projectId}/abuse-responses",
            handle_abuse_response_inventory,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/abuse-responses/{abuseResponseId}/actions/restore",
            handle_restore_abuse_response,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/projects/{projectId}/support-sessions",
            handle_support_session_inventory,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/support-sessions/current",
            handle_current_support_sessions,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/recovery-jobs",
            handle_recovery_jobs,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/recovery-jobs",
            handle_create_recovery_job,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/recovery-jobs/{recoveryJobId}",
            handle_recovery_job,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/recovery-jobs/{recoveryJobId}/actions/advance",
            handle_advance_recovery_job,
        ),
        (HttpMethod::Get, "/v1/operator/activity", handle_activity),
        (
            HttpMethod::Post,
            "/v1/operator/activity-exports",
            handle_activity_export,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/activity-exports/{activityExportId}",
            handle_get_activity_export,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/activity-exports/{activityExportId}/actions/process",
            handle_process_activity_export,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projections/{projectionName}/actions/rebuild",
            handle_projection_rebuild,
        ),
        (
            HttpMethod::Get,
            "/v1/operator/projects/{projectId}",
            handle_project as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/provisioning/{provisioningWorkflowId}/actions/repair",
            handle_repair,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/quota-overrides",
            handle_quota_override,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/organizations/{organizationId}/plan",
            handle_change_plan,
        ),
        (
            HttpMethod::Put,
            "/v1/operator/organizations/{organizationId}/plan-exceptions",
            handle_set_plan_exceptions,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/abuse-responses",
            handle_abuse_response,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/support-sessions",
            handle_support_session,
        ),
        (
            HttpMethod::Post,
            "/v1/operator/projects/{projectId}/support-sessions/{supportSessionId}/actions/revoke",
            handle_revoke_support_session,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| {
            let started = Instant::now();
            let result = handler(&graph, &request);
            graph.developer_metrics().observe_operator_control_center(
                &result,
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            );
            result
        })?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_overview(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    no_query(request)?;
    let overview = block_on(graph.operator_control_center().overview(&actor, now))
        .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &overview)
}

fn handle_tenants(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(
        request,
        &[
            "query",
            "lifecycle",
            "health",
            "region",
            "plan",
            "quotaState",
            "cursor",
            "limit",
        ],
    )?;
    let search = TenantSearch {
        query: query_value(request, "query")?.map(str::to_owned),
        lifecycle: query_value(request, "lifecycle")?
            .map(|value| serde_json::from_value(serde_json::Value::String(value.to_owned())))
            .transpose()
            .map_err(|_| invalid(request, "tenant lifecycle filter is invalid"))?,
        health: query_value(request, "health")?
            .map(|value| serde_json::from_value(serde_json::Value::String(value.to_owned())))
            .transpose()
            .map_err(|_| invalid(request, "tenant health filter is invalid"))?,
        region: query_value(request, "region")?.map(str::to_owned),
        plan: query_value(request, "plan")?.map(str::to_owned),
        quota_state: query_value(request, "quotaState")?.map(str::to_owned),
    };
    let cursor = query_value(request, "cursor")?;
    let limit = query_usize(request, "limit", 25, 100)?;
    let page = block_on(
        graph
            .operator_control_center()
            .tenants(&actor, &search, cursor, limit, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &page)
}

fn handle_tenant_360(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    no_query(request)?;
    let project = project_id(request)?;
    let view = block_on(
        graph
            .operator_control_center()
            .tenant_360(&actor, &project, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &view)
}

fn handle_inventory(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["projectId", "from", "until"])?;
    let kind: InventoryKind = serde_json::from_value(serde_json::Value::String(
        request
            .path_parameter("inventoryKind")
            .unwrap_or_default()
            .to_owned(),
    ))
    .map_err(|_| invalid(request, "operator inventory kind is invalid"))?;
    let project = query_value(request, "projectId")?
        .map(|value| mako_api::ProjectId::parse(value.to_owned()))
        .transpose()
        .map_err(|_| invalid(request, "project filter is invalid"))?;
    let from = query_u64(request, "from", now.saturating_sub(24 * 60 * 60))?;
    let until = query_u64(request, "until", now)?;
    let sections = block_on(graph.operator_control_center().inventory(
        &actor,
        kind,
        project.as_ref(),
        from,
        until,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(
        request,
        200,
        &serde_json::json!({
            "items": sections,
            "observedAtUnixSeconds": now,
        }),
    )
}

fn handle_alerts(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["projectId", "cursor", "limit"])?;
    let project = query_value(request, "projectId")?
        .map(ProjectId::parse)
        .transpose()
        .map_err(|_| invalid(request, "operator alert project scope is invalid"))?;
    let page = block_on(graph.operator_control_center().current_alerts(
        &actor,
        project.as_ref(),
        query_value(request, "cursor")?,
        query_usize(request, "limit", 25, 100)?,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &page)
}

fn handle_alert(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["projectId"])?;
    let project = query_value(request, "projectId")?
        .map(ProjectId::parse)
        .transpose()
        .map_err(|_| invalid(request, "operator alert project scope is invalid"))?;
    let fingerprint = request
        .path_parameter("alertFingerprint")
        .unwrap_or_default();
    let alert = block_on(graph.operator_control_center().current_alert(
        &actor,
        fingerprint,
        project.as_ref(),
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &alert)
}

fn handle_incidents(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(
        request,
        &[
            "cursor",
            "limit",
            "state",
            "severity",
            "projectId",
            "from",
            "until",
        ],
    )?;
    let state = query_value(request, "state")?
        .map(|value| {
            serde_json::from_value::<IncidentState>(serde_json::Value::String(value.to_owned()))
        })
        .transpose()
        .map_err(|_| invalid(request, "operator incident state filter is invalid"))?;
    let project = query_value(request, "projectId")?
        .map(ProjectId::parse)
        .transpose()
        .map_err(|_| invalid(request, "operator incident project filter is invalid"))?;
    let search = IncidentSearch {
        state,
        severity: query_value(request, "severity")?.map(str::to_owned),
        project_id: project,
        from_unix_seconds: query_value(request, "from")?
            .map(str::parse)
            .transpose()
            .map_err(|_| invalid(request, "operator incident from filter is invalid"))?,
        until_unix_seconds: query_value(request, "until")?
            .map(str::parse)
            .transpose()
            .map_err(|_| invalid(request, "operator incident until filter is invalid"))?,
    };
    let page = block_on(graph.operator_control_center().list_incidents(
        &actor,
        &search,
        query_value(request, "cursor")?,
        query_usize(request, "limit", 25, 100)?,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &page)
}

fn handle_incident(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    no_query(request)?;
    let id = request.path_parameter("incidentId").unwrap_or_default();
    let record = block_on(graph.operator_control_center().incident(&actor, id, now))
        .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_create_incident(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: CreateIncidentWire = parse_json(request)?;
    let guard = body.guard.into_guard(request)?;
    let record = block_on(graph.operator_control_center().create_incident(
        &actor,
        body.id,
        body.fingerprint,
        body.title,
        body.severity,
        body.project_id,
        &guard,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 201, &record)
}

fn handle_update_incident(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: UpdateIncidentWire = parse_json(request)?;
    let id = request.path_parameter("incidentId").unwrap_or_default();
    let guard = body.guard.into_guard(request)?;
    let record = block_on(graph.operator_control_center().update_incident(
        &actor,
        id,
        &body.action,
        body.assignee,
        body.note,
        &guard,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_create_recovery_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: RecoveryRequestWire = parse_json(request)?;
    let recovery = RecoveryRequest {
        id: body.id,
        project_id: body.project_id,
        backup_id: body.backup_id,
        target: body.target,
        backup_verified: body.backup_verified,
        impact_preview: body.impact_preview,
        guard: body.guard.into_guard(request)?,
    };
    let record = block_on(
        graph
            .operator_control_center()
            .create_recovery_job(&actor, recovery, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 202, &record)
}

fn handle_recovery_jobs(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["projectId", "cursor", "limit"])?;
    let project = query_value(request, "projectId")?
        .map(ProjectId::parse)
        .transpose()
        .map_err(|_| invalid(request, "operator recovery project filter is invalid"))?;
    let page = block_on(graph.operator_control_center().list_recovery_jobs(
        &actor,
        project.as_ref(),
        query_value(request, "cursor")?,
        query_usize(request, "limit", 25, 100)?,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &page)
}

fn handle_recovery_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    no_query(request)?;
    let id = request.path_parameter("recoveryJobId").unwrap_or_default();
    let record = block_on(
        graph
            .operator_control_center()
            .recovery_job(&actor, id, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_advance_recovery_job(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: AdvanceRecoveryWire = parse_json(request)?;
    if body.verification_succeeded
        || matches!(
            body.state,
            RecoveryJobState::PromotionReady | RecoveryJobState::Promoted
        )
    {
        return Err(conflict(
            request,
            "recovery verification and promotion require the approved executor",
        ));
    }
    let id = request.path_parameter("recoveryJobId").unwrap_or_default();
    let guard = body.guard.into_guard(request)?;
    let record = block_on(graph.operator_control_center().advance_recovery_job(
        &actor,
        id,
        body.state,
        body.verification_succeeded,
        body.error_class,
        &guard,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_activity(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["cursor", "limit", "query", "from", "until"])?;
    let page = block_on(
        graph.operator_control_center().activity(
            &actor,
            query_value(request, "cursor")?,
            query_usize(request, "limit", 25, 100)?,
            query_value(request, "query")?,
            query_value(request, "from")?
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| invalid(request, "activity from bound is invalid"))?,
            query_value(request, "until")?
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| invalid(request, "activity until bound is invalid"))?,
            now,
        ),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &page)
}

fn handle_activity_export(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: ActivityExportWire = parse_json(request)?;
    let guard = body.guard.into_guard(request)?;
    let record = block_on(graph.operator_control_center().create_activity_export(
        &actor,
        body.id,
        &body.filter,
        &guard,
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 202, &record)
}

fn handle_get_activity_export(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    no_query(request)?;
    let id = request
        .path_parameter("activityExportId")
        .unwrap_or_default();
    let record = block_on(
        graph
            .operator_control_center()
            .activity_export(&actor, id, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_process_activity_export(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_payload(request)?;
    no_query(request)?;
    let id = request
        .path_parameter("activityExportId")
        .unwrap_or_default();
    let record = block_on(
        graph
            .operator_control_center()
            .process_activity_export(&actor, id, now),
    )
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_projection_rebuild(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    require_json(request)?;
    reject_unknown_query(request, &[])?;
    let body: ProjectionWire = parse_json(request)?;
    let record = block_on(graph.operator_control_center().rebuild_projection(
        &actor,
        request.path_parameter("projectionName").unwrap_or_default(),
        body.cursor.as_deref(),
        now,
    ))
    .map_err(|error| control_center_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_provisioning_inventory(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["projectId", "limit"])?;
    let project = query_value(request, "projectId")?
        .map(|value| mako_api::ProjectId::parse(value.to_owned()))
        .transpose()
        .map_err(|_| invalid(request, "project filter is invalid"))?;
    let limit = NonZeroUsize::new(query_usize(request, "limit", 25, 100)?)
        .expect("validated limit is positive");
    let items = block_on(graph.operator_service().list_provisioning_workflows(
        &actor,
        project.as_ref(),
        limit,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &serde_json::json!({ "items": items }))
}

fn handle_quota_override_inventory(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["limit"])?;
    let project = project_id(request)?;
    let limit = NonZeroUsize::new(query_usize(request, "limit", 25, 100)?)
        .expect("validated limit is positive");
    let items = block_on(
        graph
            .operator_service()
            .list_quota_overrides(&actor, &project, limit, now),
    )
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &serde_json::json!({ "items": items }))
}

fn handle_replace_quota_override(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: ReplaceQuotaOverrideWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = QuotaOverrideId::parse(
        request
            .path_parameter("quotaOverrideId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "quota override path is invalid"))?;
    let expires = body
        .expires_at
        .as_deref()
        .map(|value| timestamp(request, value))
        .transpose()?;
    let record = block_on(graph.operator_service().replace_quota_override(
        &actor,
        &project,
        &id,
        body.reviewed_version,
        body.limit,
        expires,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_revoke_quota_override(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: VersionedReasonWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = QuotaOverrideId::parse(
        request
            .path_parameter("quotaOverrideId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "quota override path is invalid"))?;
    let record = block_on(graph.operator_service().revoke_quota_override(
        &actor,
        &project,
        &id,
        body.reviewed_version,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_abuse_response_inventory(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["limit"])?;
    let project = project_id(request)?;
    let limit = NonZeroUsize::new(query_usize(request, "limit", 25, 100)?)
        .expect("validated limit is positive");
    let items = block_on(
        graph
            .operator_service()
            .list_abuse_responses(&actor, &project, limit, now),
    )
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &serde_json::json!({ "items": items }))
}

fn handle_restore_abuse_response(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: VersionedReasonWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = AbuseResponseId::parse(
        request
            .path_parameter("abuseResponseId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "abuse response path is invalid"))?;
    let record = block_on(graph.operator_service().restore_abuse_response(
        &actor,
        &project,
        &id,
        body.reviewed_version,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_support_session_inventory(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["limit"])?;
    let project = project_id(request)?;
    let limit = NonZeroUsize::new(query_usize(request, "limit", 25, 100)?)
        .expect("validated limit is positive");
    let items = block_on(
        graph
            .operator_service()
            .list_support_sessions(&actor, &project, limit, now),
    )
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &serde_json::json!({ "items": items }))
}

fn handle_current_support_sessions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["limit"])?;
    let limit = NonZeroUsize::new(query_usize(request, "limit", 25, 100)?)
        .expect("validated limit is positive");
    let items = block_on(
        graph
            .operator_service()
            .list_active_support_sessions_for_operator(&actor, limit, now),
    )
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &serde_json::json!({ "items": items }))
}

fn handle_operator_security(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    reject_unknown_query(request, &["limit"])?;
    if !actor.allows(OperatorPermission::SecurityRead)
        && !actor.allows(OperatorPermission::TenantRead)
    {
        return Err(forbidden(
            request,
            "operator security inventory is forbidden",
        ));
    }
    let limit = NonZeroUsize::new(query_usize(request, "limit", 50, 100)?)
        .expect("validated limit is positive");
    let store = graph.operator_password_authentication().store();
    let entitlements = block_on(store.list_entitlements(limit))
        .map_err(|_| unavailable(request, "operator entitlement inventory is unavailable"))?;
    let sessions = block_on(store.list_sessions(limit))
        .map_err(|_| unavailable(request, "operator session inventory is unavailable"))?;
    let attempts = block_on(store.list_attempts(limit))
        .map_err(|_| unavailable(request, "operator attempt inventory is unavailable"))?;
    let entitlement_items = entitlements
        .iter()
        .map(|record| {
            serde_json::json!({
                "developerIdentityId": record.developer_identity_id(),
                "operatorId": record.operator_id(),
                "operatorEpoch": record.operator_epoch(),
                "permissions": record.permissions(),
            })
        })
        .collect::<Vec<_>>();
    let session_items = sessions
        .iter()
        .map(|record| {
            serde_json::json!({
                "developerIdentityId": record.developer_identity_id(),
                "operatorId": record.operator_id(),
                "operatorEpoch": record.operator_epoch(),
                "permissions": record.permissions(),
                "passwordVerifiedAtUnixSeconds": record.password_verified_at_unix_seconds(),
                "expiresAtUnixSeconds": record.expires_at_unix_seconds(),
                "revokedAtUnixSeconds": record.revoked_at_unix_seconds(),
            })
        })
        .collect::<Vec<_>>();
    let attempt_items = attempts
        .iter()
        .map(|record| {
            serde_json::json!({
                "class": record.class(),
                "count": record.count(),
                "nextAllowedAtUnixSeconds": record.next_allowed_at_unix_seconds(),
                "expiresAtUnixSeconds": record.expires_at_unix_seconds(),
            })
        })
        .collect::<Vec<_>>();
    public_json(
        request,
        200,
        &serde_json::json!({
            "entitlements": entitlement_items,
            "sessions": session_items,
            "attempts": attempt_items,
            "observedAtUnixSeconds": now,
        }),
    )
}

fn handle_revoke_operator_sessions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    if !actor.allows(OperatorPermission::SecurityManage) {
        return Err(forbidden(
            request,
            "operator session revocation is forbidden",
        ));
    }
    let body: ReasonWire = parse_json(request)?;
    if body.reason.len() < 8 || body.reason.len() > 1_024 {
        return Err(invalid(
            request,
            "operator session revocation reason is invalid",
        ));
    }
    let identity = mako_control_plane::DeveloperIdentityId::parse(
        request
            .path_parameter("developerIdentityId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "developer identity path is invalid"))?;
    let revoked = block_on(
        graph
            .operator_password_authentication()
            .revoke_identity_sessions(&identity, now),
    )
    .map_err(|_| unavailable(request, "operator session revocation is unavailable"))?;
    public_json(
        request,
        200,
        &serde_json::json!({ "revokedSessions": revoked }),
    )
}

fn handle_plan_operator_entitlement(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, _) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    if !actor.allows(OperatorPermission::SecurityManage) {
        return Err(forbidden(
            request,
            "operator entitlement administration is forbidden",
        ));
    }
    let body: EntitlementWire = parse_json(request)?;
    let input = body.input(request)?;
    let plan = block_on(
        graph
            .operator_password_authentication()
            .entitlement_admin_service()
            .plan(&input),
    )
    .map_err(|error| operator_entitlement_error(request, error))?;
    public_json(
        request,
        200,
        &serde_json::json!({
            "developerIdentityId": plan.developer_identity_id,
            "permissionDigest": plan.permission_digest,
            "operationDigest": plan.operation_digest,
            "operatorStatusBefore": plan.operator_status_before,
            "operatorStatusAfter": plan.operator_status_after,
            "typedConfirmation": plan.typed_confirmation,
            "replayed": plan.replayed,
        }),
    )
}

fn handle_apply_operator_entitlement(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    if !actor.allows(OperatorPermission::SecurityManage) {
        return Err(forbidden(
            request,
            "operator entitlement administration is forbidden",
        ));
    }
    let body: ApplyEntitlementWire = parse_json(request)?;
    let input = body.input.input(request)?;
    let result = block_on(
        graph
            .operator_password_authentication()
            .entitlement_admin_service()
            .apply(&input, &body.typed_confirmation, request.request_id(), now),
    )
    .map_err(|error| operator_entitlement_error(request, error))?;
    public_json(
        request,
        200,
        &serde_json::json!({
            "developerIdentityId": result.developer_identity_id,
            "permissionDigest": result.permission_digest,
            "operationDigest": result.operation_digest,
            "operatorStatusBefore": result.operator_status_before,
            "operatorStatusAfter": result.operator_status_after,
            "operatorEpoch": result.operator_epoch,
            "replayed": result.replayed,
        }),
    )
}

fn handle_project(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, false)?;
    no_payload(request)?;
    let project = project_id(request)?;
    let view = block_on(graph.operator_service().inspect_project(
        &actor,
        &project,
        NonZeroUsize::new(1_000).expect("operator environment limit is positive"),
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_value(
        request,
        200,
        serde_json::json!({
            "project": view.project,
            "environments": view.environments,
        }),
    )
}

fn handle_repair(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: RepairWire = parse_json(request)?;
    let project = project_id(request)?;
    let workflow = ProvisioningWorkflowId::parse(
        request
            .path_parameter("provisioningWorkflowId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "provisioning workflow path is invalid"))?;
    let reviewed_at = body
        .reviewed_at
        .as_deref()
        .map(|value| timestamp(request, value))
        .transpose()?;
    let record = block_on(graph.operator_service().repair_provisioning_guarded(
        &actor,
        &project,
        &workflow,
        &body.reason,
        body.action,
        reviewed_at,
        body.operation_key,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &record)
}

fn handle_quota_override(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: QuotaOverrideWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = QuotaOverrideId::parse(body.id)
        .map_err(|_| invalid(request, "quota override id is invalid"))?;
    let expires = body
        .expires_at
        .as_deref()
        .map(|value| timestamp(request, value))
        .transpose()?;
    let record = block_on(graph.operator_service().create_quota_override(
        &actor,
        NewQuotaOverride {
            id,
            project_id: project,
            resource: body.resource,
            limit: body.limit,
            reason: body.reason,
            expires_at_unix_seconds: expires,
            now_unix_seconds: now,
        },
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 201, &record)
}

/// Reinstall what every environment of an organization is held to.
///
/// Shared by the plan change and the exception change, because both alter the
/// same resolved limits, and failing mid-way leaves the record ahead of
/// enforcement -- which a retry converges.
fn reinstall_organization_limits(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor_id: &str,
    organization: &OrganizationId,
    policy: &serde_json::Value,
) -> Result<(), HttpApiError> {
    let limit = NonZeroUsize::new(100).expect("listing limit");
    let projects = block_on(graph.project_store().list_projects(organization, limit))
        .map_err(|_| unavailable(request, "the organization's projects are unavailable"))?;
    for project in &projects {
        let environments =
            block_on(graph.project_store().list_environments(project.id(), limit))
                .map_err(|_| unavailable(request, "the project's environments are unavailable"))?;
        for environment in &environments {
            let tenant = TenantScope::new(project.id().clone(), environment.id().clone());
            block_on(crate::identity_admin_http::install_quota_policy(
                graph, request, actor_id, &tenant, policy,
            ))?;
        }
    }
    Ok(())
}

fn handle_change_plan(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: PlanChangeWire = parse_json(request)?;
    let organization =
        OrganizationId::parse(request.path_parameter("organizationId").unwrap_or_default())
            .map_err(|_| invalid(request, "organization path is invalid"))?;
    let record = block_on(graph.operator_service().change_organization_plan(
        &actor,
        &organization,
        &body.plan_id,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;

    // The record has changed; what each environment is actually held to has
    // not, until the resolved limits reach the data plane that enforces them.
    // Failing here leaves the record ahead of enforcement, which a retry
    // converges -- the same direction every other propagation in this service
    // takes.
    let exceptions = block_on(graph.operator_service().plan_exceptions(&organization, now))
        .map_err(|error| operator_error(request, error))?;
    let policy =
        crate::management_http::resolve_plan_policy(request, record.plan_id(), &exceptions, now)?;
    reinstall_organization_limits(graph, request, actor.id().as_str(), &organization, &policy)?;

    public_json(
        request,
        200,
        &serde_json::json!({
            "organizationId": record.id().as_str(),
            "planId": record.plan_id(),
            "updatedAt": crate::management_http::format_timestamp(request, record.updated_at_unix_seconds())?,
        }),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PlanChangeWire {
    plan_id: String,
    reason: String,
}

fn handle_set_plan_exceptions(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: PlanExceptionsWire = parse_json(request)?;
    let organization =
        OrganizationId::parse(request.path_parameter("organizationId").unwrap_or_default())
            .map_err(|_| invalid(request, "organization path is invalid"))?;
    let stored = block_on(graph.operator_service().set_plan_exceptions(
        &actor,
        &organization,
        body.exceptions,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;

    // The exceptions change what the plan resolves to, so what every
    // environment is held to is reinstalled before the change is reported.
    let record = block_on(graph.organization_store().get_organization(&organization))
        .map_err(|_| unavailable(request, "organization is unavailable"))?
        .ok_or_else(|| not_found(request, "organization was not found"))?;
    let policy =
        crate::management_http::resolve_plan_policy(request, record.plan_id(), &stored, now)?;
    reinstall_organization_limits(graph, request, actor.id().as_str(), &organization, &policy)?;

    public_json(
        request,
        200,
        &serde_json::json!({
            "organizationId": organization.as_str(),
            "exceptions": stored,
        }),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PlanExceptionsWire {
    exceptions: Vec<mako_billing::PlanException>,
    reason: String,
}

fn handle_abuse_response(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: AbuseResponseWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = AbuseResponseId::parse(body.id)
        .map_err(|_| invalid(request, "abuse response id is invalid"))?;
    let record = block_on(graph.operator_service().respond_to_abuse(
        &actor,
        NewAbuseResponse {
            id,
            project_id: project,
            target: body.target,
            reason: body.reason,
            now_unix_seconds: now,
        },
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 201, &record)
}

fn handle_support_session(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: SupportSessionWire = parse_json(request)?;
    let project = project_id(request)?;
    let id = SupportSessionId::parse(body.id)
        .map_err(|_| invalid(request, "support session id is invalid"))?;
    let expires = timestamp(request, &body.expires_at)?;
    let permissions = body.permissions.into_iter().collect::<BTreeSet<_>>();
    let record = block_on(graph.operator_service().create_support_session(
        &actor,
        NewSupportSession {
            id,
            project_id: project,
            environment_id: body.environment_id,
            permissions,
            reason: body.reason,
            expires_at_unix_seconds: expires,
            now_unix_seconds: now,
        },
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 201, &record)
}

fn handle_revoke_support_session(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let (actor, now) = authorize_operator(graph, request, true)?;
    no_query(request)?;
    require_json(request)?;
    let body: ReasonWire = parse_json(request)?;
    let project = project_id(request)?;
    let session = SupportSessionId::parse(
        request
            .path_parameter("supportSessionId")
            .unwrap_or_default()
            .to_owned(),
    )
    .map_err(|_| invalid(request, "support session path is invalid"))?;
    let record = block_on(graph.operator_service().revoke_support_session(
        &actor,
        &project,
        &session,
        &body.reason,
        now,
    ))
    .map_err(|error| operator_error(request, error))?;
    public_json(request, 200, &record)
}

fn authorize_operator(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    mutation: bool,
) -> Result<(mako_control_plane::OperatorPrincipal, u64), HttpApiError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| unavailable(request, "system clock is unavailable"))?;
    let actor = authenticate_operator(graph, request, now, mutation)?;
    Ok((actor, now))
}

fn timestamp(request: &HttpRequest, value: &str) -> Result<u64, HttpApiError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .and_then(|time| u64::try_from(time.unix_timestamp()).ok())
        .ok_or_else(|| invalid(request, "date-time is invalid"))
}

fn operator_error(request: &HttpRequest, error: OperatorError) -> HttpApiError {
    match error {
        OperatorError::InvalidIdentifier
        | OperatorError::InvalidInput
        | OperatorError::ScopeMismatch => invalid(request, "operator request is invalid"),
        OperatorError::NotFound => not_found(request, "operator resource was not found"),
        OperatorError::Forbidden => forbidden(request, "operator action is forbidden"),
        OperatorError::InvalidState | OperatorError::Conflict => {
            conflict(request, "operator operation conflicts with current state")
        }
        _ => unavailable(request, "operator service is unavailable"),
    }
}

fn control_center_error(request: &HttpRequest, error: OperatorControlCenterError) -> HttpApiError {
    match error {
        OperatorControlCenterError::InvalidInput
        | OperatorControlCenterError::InvalidCursor
        | OperatorControlCenterError::BoundExceeded
        | OperatorControlCenterError::UnsafeResponse => {
            invalid(request, "operator control-center request is invalid")
        }
        OperatorControlCenterError::Forbidden => {
            forbidden(request, "operator control-center action is forbidden")
        }
        OperatorControlCenterError::NotFound => {
            not_found(request, "operator control-center resource was not found")
        }
        OperatorControlCenterError::InvalidState
        | OperatorControlCenterError::StaleVersion
        | OperatorControlCenterError::Conflict
        | OperatorControlCenterError::FeatureDisabled => conflict(
            request,
            "operator control-center action conflicts with current state",
        ),
        OperatorControlCenterError::StepUpRequired => HttpApiError::new(
            401,
            ErrorCode::OperatorStepUpRequired,
            "Verify your operator password again, then review and retry the same action.",
            request.request_id(),
            RetryAdvice::Never,
        ),
        _ => unavailable(request, "operator control-center service is unavailable"),
    }
}

fn query_usize(
    request: &HttpRequest,
    name: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, HttpApiError> {
    let value = query_value(request, name)?
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| invalid(request, "numeric query parameter is invalid"))?
        .unwrap_or(default);
    if value == 0 || value > maximum {
        return Err(invalid(
            request,
            "numeric query parameter is outside its bound",
        ));
    }
    Ok(value)
}

fn query_u64(request: &HttpRequest, name: &str, default: u64) -> Result<u64, HttpApiError> {
    query_value(request, name)?
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| invalid(request, "time query parameter is invalid"))
        .map(|value| value.unwrap_or(default))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateIncidentWire {
    id: String,
    fingerprint: String,
    title: String,
    severity: String,
    project_id: Option<mako_api::ProjectId>,
    guard: GuardWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateIncidentWire {
    action: String,
    assignee: Option<String>,
    note: Option<String>,
    guard: GuardWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryRequestWire {
    id: String,
    project_id: mako_api::ProjectId,
    backup_id: String,
    target: String,
    backup_verified: bool,
    impact_preview: String,
    guard: GuardWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AdvanceRecoveryWire {
    state: RecoveryJobState,
    verification_succeeded: bool,
    error_class: Option<String>,
    guard: GuardWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityExportWire {
    id: String,
    filter: serde_json::Value,
    guard: GuardWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GuardWire {
    operation_key: String,
    reviewed_version: u64,
    reason: String,
    case_reference: Option<String>,
    confirmation: String,
    action_binding: String,
    password_verified_at: String,
}

impl GuardWire {
    fn into_guard(self, request: &HttpRequest) -> Result<GuardedMutation, HttpApiError> {
        Ok(GuardedMutation {
            operation_key: self.operation_key,
            reviewed_version: self.reviewed_version,
            reason: self.reason,
            case_reference: self.case_reference,
            confirmation: self.confirmation,
            action_binding: self.action_binding,
            password_verified_at_unix_seconds: timestamp(request, &self.password_verified_at)?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionWire {
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairWire {
    reason: String,
    action: OperatorRepairAction,
    #[serde(default)]
    reviewed_at: Option<String>,
    #[serde(default)]
    operation_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct QuotaOverrideWire {
    id: String,
    resource: QuotaResource,
    limit: u64,
    reason: String,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbuseResponseWire {
    id: String,
    target: AbuseTarget,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SupportSessionWire {
    id: String,
    environment_id: Option<mako_api::EnvironmentId>,
    permissions: Vec<SupportPermission>,
    reason: String,
    expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasonWire {
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReplaceQuotaOverrideWire {
    reviewed_version: u64,
    limit: u64,
    expires_at: Option<String>,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VersionedReasonWire {
    reviewed_version: u64,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EntitlementWire {
    kind: OperatorEntitlementChangeKind,
    target_email: String,
    permissions: Vec<OperatorPermission>,
    private_reason: String,
    environment_binding: String,
    idempotency_key: String,
}

impl EntitlementWire {
    fn input(self, request: &HttpRequest) -> Result<OperatorEntitlementAdminInput, HttpApiError> {
        let permissions = self.permissions.into_iter().collect::<BTreeSet<_>>();
        if permissions.len() > 17 {
            return Err(invalid(
                request,
                "operator entitlement permission set is invalid",
            ));
        }
        Ok(OperatorEntitlementAdminInput {
            kind: self.kind,
            target_email: self.target_email,
            permissions,
            private_reason: self.private_reason,
            environment_binding: self.environment_binding,
            idempotency_key: self.idempotency_key,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ApplyEntitlementWire {
    input: EntitlementWire,
    typed_confirmation: String,
}

fn operator_entitlement_error(
    request: &HttpRequest,
    error: OperatorPasswordAuthenticationError,
) -> HttpApiError {
    match error {
        OperatorPasswordAuthenticationError::NotFound => {
            not_found(request, "operator entitlement target was not found")
        }
        OperatorPasswordAuthenticationError::Ineligible
        | OperatorPasswordAuthenticationError::InvalidConfiguration => {
            invalid(request, "operator entitlement request is invalid")
        }
        OperatorPasswordAuthenticationError::Conflict
        | OperatorPasswordAuthenticationError::ConfirmationMismatch
        | OperatorPasswordAuthenticationError::LastRecoverableAdministrator => conflict(
            request,
            "operator entitlement state conflicts with this request",
        ),
        OperatorPasswordAuthenticationError::StepUpRequired => {
            control_center_error(request, OperatorControlCenterError::StepUpRequired)
        }
        _ => unavailable(
            request,
            "operator entitlement administration is unavailable",
        ),
    }
}
