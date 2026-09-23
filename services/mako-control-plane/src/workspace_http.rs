use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use mako_api::{
    CollectionId, ConnectCollection, ConnectMetadata, ConnectionCheckRequest,
    ConnectionCheckResult, ConnectionCheckState, ConnectionCheckStep, DeveloperRestoreRequest,
    EXPLORER_MAX_SYNC_WINDOW_SECONDS, ObservabilityPage, ProviderStatus, SyncSummary, TenantScope,
    WorkspaceDestination, WorkspaceSummary, WorkspaceSummarySection,
};
use mako_control_plane::{
    DeveloperWorkspaceError, LifecycleState, ObservabilityPayload, ObservabilityQuery,
    ProjectDataPermission,
};
use mako_identity::{ProjectCredentialKind, ProjectCredentialMetadata, ProjectCredentialState};
use mako_internal_rpc::{IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::Deserialize;
use serde_json::{Value, json as json_value};

use crate::{
    ControlPlaneGraph,
    http_support::{project_id, query_value, reject_unknown_query, tenant},
    management_http::{
        conflict, forbidden, invalid, json, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_workspace_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/workspace/summary",
            workspace_summary as Handler,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/workspace/navigation",
            workspace_navigation,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/connect",
            connect_metadata,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/connect/check",
            connection_check,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/sync/summary",
            sync_summary,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/backups",
            list_backups,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/restore-requests",
            request_restore,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/restore-requests",
            list_restores,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn workspace_summary(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    no_query(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let project = graph
            .project_service()
            .get_project(&actor, tenant.project_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        let environment = graph
            .project_service()
            .get_environment(&actor, tenant.project_id(), tenant.environment_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        let mut sections = BTreeMap::new();
        sections.insert(
            "lifecycle".to_owned(),
            current_section(
                now,
                json_value!({
                    "project": lifecycle_name(project.lifecycle()),
                    "environment": lifecycle_name(environment.lifecycle()),
                    "region": project.region(),
                    "ready": project.lifecycle() == LifecycleState::Active
                        && environment.lifecycle() == LifecycleState::Active,
                }),
            ),
        );
        match graph
            .collection_service()
            .list_collections(
                &actor,
                &tenant,
                NonZeroUsize::new(1_000).expect("collection count limit"),
                now,
            )
            .await
        {
            Ok(collections) => {
                sections.insert(
                    "collections".to_owned(),
                    current_section(
                        now,
                        json_value!({
                            "count": collections.len(),
                            "limited": collections.len() == 1_000,
                            "active": collections.iter().filter(|item| item.lifecycle() == mako_documents::CollectionLifecycle::Active).count(),
                        }),
                    ),
                );
            }
            Err(_) => {
                sections.insert(
                    "collections".to_owned(),
                    unavailable_section(now, "collection_summary_unavailable"),
                );
            }
        }
        match graph
            .data_job_service()
            .list(
                &actor,
                &tenant,
                NonZeroUsize::new(100).expect("job count limit"),
            )
            .await
        {
            Ok(jobs) => {
                sections.insert(
                    "dataJobs".to_owned(),
                    current_section(
                        now,
                        json_value!({
                            "count": jobs.len(),
                            "limited": jobs.len() == 100,
                            "active": jobs.iter().filter(|job| !matches!(
                                job.state,
                                mako_api::DataJobState::Succeeded
                                    | mako_api::DataJobState::Failed
                                    | mako_api::DataJobState::Cancelled
                                    | mako_api::DataJobState::Expired
                            )).count(),
                        }),
                    ),
                );
            }
            Err(_) => {
                sections.insert(
                    "dataJobs".to_owned(),
                    unavailable_section(now, "data_job_summary_unavailable"),
                );
            }
        }
        let query = ObservabilityQuery {
            cursor: None,
            from_unix_milliseconds: Some(now.saturating_sub(60 * 60).saturating_mul(1_000)),
            until_unix_milliseconds: Some(now.saturating_mul(1_000)),
            limit: 100,
        };
        for (id, result) in [
            (
                "usage",
                graph
                    .observability_service()
                    .query_usage(&actor, &tenant, &query, now.saturating_mul(1_000))
                    .await,
            ),
            (
                "sync",
                graph
                    .observability_service()
                    .query_replication_errors(&actor, &tenant, &query, now.saturating_mul(1_000))
                    .await,
            ),
            (
                "activity",
                graph
                    .observability_service()
                    .query_audit_events(&actor, &tenant, &query, now.saturating_mul(1_000))
                    .await,
            ),
        ] {
            sections.insert(
                id.to_owned(),
                match result {
                    Ok(page) => WorkspaceSummarySection {
                        status: ProviderStatus::Current,
                        observed_at_unix_seconds: page.retention.observed_at_unix_milliseconds
                            / 1_000,
                        fresh_until_unix_seconds: now.saturating_add(60),
                        retained_since_unix_seconds: Some(
                            page.retention.retained_from_unix_milliseconds / 1_000,
                        ),
                        payload: Some(observability_summary(id, &page, now)),
                        remediation_code: None,
                    },
                    Err(_) => unavailable_section(now, &format!("{id}_summary_unavailable")),
                },
            );
        }
        match graph
            .function_service()
            .list_functions(
                &actor,
                &tenant,
                NonZeroUsize::new(100).expect("function count limit"),
                now,
            )
            .await
        {
            Ok(functions) => {
                sections.insert(
                    "functions".to_owned(),
                    current_section(
                        now,
                        json_value!({"count": functions.len(), "limited": functions.len() == 100}),
                    ),
                );
            }
            Err(_) => {
                sections.insert(
                    "functions".to_owned(),
                    unavailable_section(now, "function_summary_unavailable"),
                );
            }
        }
        let backups = graph.developer_backups(&tenant, now);
        // The inventory names what a recovery has to bring back: the verified
        // recovery points, and the objects applications stored beside their data.
        let object_storage =
            crate::storage_bucket_http::object_storage_totals(graph, request, &actor, &tenant)
                .await;
        sections.insert(
            "backups".to_owned(),
            current_section(
                now,
                json_value!({"verifiedCount": backups.len(), "objectStorage": object_storage}),
            ),
        );
        json(request, 200, &WorkspaceSummary { tenant, sections })
    })
}

// This is a bounded overview of already-authorized, scrubbed telemetry. Keep
// bodies, free-form audit details, error messages, and cursor tokens out of it.
fn observability_summary(id: &str, page: &ObservabilityPage, now: u64) -> Value {
    let mut payload = json_value!({
        "recordCount": page.items.len(),
        "limited": page.next_cursor.is_some(),
        "windowStartUnixSeconds": now.saturating_sub(3_600),
        "windowEndUnixSeconds": now,
    });
    if id == "activity" {
        let mut records = page.items.iter().collect::<Vec<_>>();
        records.sort_by_key(|item| std::cmp::Reverse(item.timestamp_unix_milliseconds));
        let events = records
            .into_iter()
            .filter_map(|item| {
                if let ObservabilityPayload::Audit {
                    actor_id,
                    action,
                    target,
                    outcome,
                    ..
                } = &item.payload
                {
                    Some(json_value!({
                        "timestampUnixMilliseconds": item.timestamp_unix_milliseconds,
                        "actorId": actor_id.chars().take(128).collect::<String>(),
                        "action": action.chars().take(128).collect::<String>(),
                        "target": target.chars().take(256).collect::<String>(),
                        "outcome": outcome,
                    }))
                } else {
                    None
                }
            })
            .take(8)
            .collect::<Vec<_>>();
        if events.len() < page.items.len() {
            payload["limited"] = Value::Bool(true);
        }
        payload["events"] = json_value!(events);
    } else if id == "usage" {
        let mut samples = BTreeMap::new();
        for item in &page.items {
            if let ObservabilityPayload::Usage {
                resource,
                quantity,
                unit,
            } = &item.payload
            {
                let sample = samples.entry(resource).or_insert((0, 0, ""));
                if item.timestamp_unix_milliseconds >= sample.0 {
                    *sample = (item.timestamp_unix_milliseconds, *quantity, unit.as_str());
                }
            }
        }
        payload["samples"] = json_value!(
            samples
                .into_iter()
                .map(|(resource, (timestamp, quantity, unit))| json_value!({
                    "resource": resource, "quantity": quantity, "unit": unit,
                    "timestampUnixMilliseconds": timestamp,
                }))
                .collect::<Vec<_>>()
        );
    }
    payload
}

fn workspace_navigation(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    no_query(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let project = graph
            .project_service()
            .get_project(&actor, tenant.project_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        graph
            .project_service()
            .get_environment(&actor, tenant.project_id(), tenant.environment_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        let membership = graph
            .organization_store()
            .get_membership(project.organization_id(), actor.identity_id())
            .await
            .map_err(|_| unavailable(request, "workspace navigation is unavailable"))?
            .ok_or_else(|| not_found(request, "workspace was not found"))?;
        let base = format!(
            "/projects/{}/environments/{}",
            tenant.project_id().as_str(),
            tenant.environment_id().as_str()
        );
        let destinations = [
            (
                "overview",
                "Overview",
                "overview",
                ProjectDataPermission::DataRead,
            ),
            ("data", "Data", "data", ProjectDataPermission::DataRead),
            (
                "collections",
                "Collections",
                "collections",
                ProjectDataPermission::DataRead,
            ),
            ("sync", "Sync", "sync", ProjectDataPermission::DataRead),
            ("users", "Users", "users", ProjectDataPermission::DataRead),
            (
                "policies",
                "Policies",
                "policies",
                ProjectDataPermission::DataRead,
            ),
            (
                "functions",
                "Functions",
                "functions",
                ProjectDataPermission::DataRead,
            ),
            (
                "observability",
                "Observability",
                "observability",
                ProjectDataPermission::DataRead,
            ),
            (
                "backups",
                "Backups",
                "backups",
                ProjectDataPermission::BackupRead,
            ),
            (
                "connect",
                "API & Connect",
                "connect",
                ProjectDataPermission::DataRead,
            ),
            (
                "credentials",
                "API keys",
                "credentials",
                ProjectDataPermission::DataAdmin,
            ),
            (
                "settings",
                "Settings",
                "settings",
                ProjectDataPermission::DataAdmin,
            ),
        ]
        .into_iter()
        .map(|(id, label, path, permission)| WorkspaceDestination {
            id: id.to_owned(),
            label: label.to_owned(),
            path: format!("{base}/{path}"),
            permitted: membership.role().allows_data(permission),
        })
        .collect::<Vec<_>>();
        json(request, 200, &destinations)
    })
}

fn connect_metadata(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    no_query(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        graph
            .project_service()
            .get_environment(&actor, tenant.project_id(), tenant.environment_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        let collections = graph
            .collection_service()
            .list_collections(
                &actor,
                &tenant,
                NonZeroUsize::new(1_000).expect("connect collection limit"),
                now,
            )
            .await
            .map_err(|_| unavailable(request, "connect metadata is unavailable"))?
            .into_iter()
            .filter(|collection| {
                collection.lifecycle() == mako_documents::CollectionLifecycle::Active
                    && collection.compatibility() == mako_documents::SchemaCompatibility::Compatible
            })
            .map(|collection| ConnectCollection {
                collection_id: collection.collection_id().clone(),
                active_schema_version: collection.schema_version().get(),
            })
            .collect();
        let public_key_id =
            active_public_key_id(graph, request, &tenant, actor.identity_id().as_str())?;
        json(
            request,
            200,
            &ConnectMetadata {
                tenant,
                public_endpoint: graph.public_origin().to_owned(),
                public_key_id,
                public_key: String::new(),
                collections,
                rxdb_client_range: ">=17.0.0 <18.0.0".to_owned(),
                template_version: 1,
            },
        )
    })
}

fn connection_check(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    let input: ConnectionCheckRequest = parse_json(request)?;
    if input
        .public_key_id
        .as_ref()
        .is_some_and(|value| value.len() > 200 || value.is_empty())
        || input.schema_version == Some(0)
        || input
            .rxdb_version
            .as_ref()
            .is_some_and(|value| value.len() > 64 || value.is_empty())
    {
        return Err(invalid(request, "connection check input is invalid"));
    }
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let environment_ready = graph
            .project_service()
            .get_environment(&actor, tenant.project_id(), tenant.environment_id(), now)
            .await
            .is_ok();
        let data_plane_ready = graph
            .data_plane_identity_admin()
            .dependency_ready()
            .unwrap_or(false);
        let https = graph.public_origin().starts_with("https://");
        let public_key =
            active_public_key_id(graph, request, &tenant, actor.identity_id().as_str())?;
        let collections = graph
            .collection_service()
            .list_collections(
                &actor,
                &tenant,
                NonZeroUsize::new(1_000).expect("connection collection limit"),
                now,
            )
            .await
            .unwrap_or_default();
        let collections_ready = collections.into_iter().any(|collection| {
            collection.lifecycle() == mako_documents::CollectionLifecycle::Active
                && collection.compatibility() == mako_documents::SchemaCompatibility::Compatible
                && input
                    .collection_id
                    .as_ref()
                    .is_none_or(|selected| selected == collection.collection_id())
                && input
                    .schema_version
                    .is_none_or(|version| version == collection.schema_version().get())
        });
        let public_key_ready = public_key != "setup_required"
            && input
                .public_key_id
                .as_ref()
                .is_none_or(|selected| selected == &public_key);
        let client_compatible = input
            .rxdb_version
            .as_deref()
            .is_none_or(supported_rxdb_version);
        let steps = vec![
            check_step("dns", https, "public_dns_unavailable", true),
            check_step("tls", https, "public_tls_unavailable", false),
            check_step(
                "public_routing",
                data_plane_ready,
                "public_route_unavailable",
                true,
            ),
            check_step(
                "environment_readiness",
                environment_ready,
                "environment_unready",
                true,
            ),
            check_step(
                "public_key_recognition",
                public_key_ready,
                "create_or_supply_public_key",
                false,
            ),
            check_step(
                "schema_compatibility",
                collections_ready,
                "schema_mismatch",
                false,
            ),
            check_step(
                "client_compatibility",
                client_compatible,
                "unsupported_rxdb_client",
                false,
            ),
            check_step(
                "replication_route",
                data_plane_ready,
                "replication_route_unavailable",
                true,
            ),
        ];
        json(
            request,
            200,
            &ConnectionCheckResult {
                checked_at_unix_seconds: now,
                steps,
            },
        )
    })
}

fn supported_rxdb_version(value: &str) -> bool {
    let version = value.strip_prefix('v').unwrap_or(value);
    let (without_build, build) = version
        .split_once('+')
        .map_or((version, None), |(core, build)| (core, Some(build)));
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    let mut parts = core.split('.');
    matches!(parts.next(), Some("17"))
        && parts.next().is_some_and(valid_numeric_version_part)
        && parts.next().is_some_and(valid_numeric_version_part)
        && parts.next().is_none()
        && prerelease.is_none_or(valid_version_suffix)
        && build.is_none_or(valid_version_suffix)
}

fn valid_numeric_version_part(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_version_suffix(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn active_public_key_id(
    graph: &ControlPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    actor_id: &str,
) -> Result<String, HttpApiError> {
    let metadata: Vec<ProjectCredentialMetadata> = graph
        .data_plane_identity_admin()
        .administer(
            tenant,
            request.request_id(),
            &format!(
                "connect-public-keys-{}-{}",
                tenant.project_id(),
                tenant.environment_id()
            ),
            &IdentityAdminCommand {
                operation: IdentityAdminOperation::ListProjectCredentials,
                actor_id: actor_id.to_owned(),
                permissions: std::collections::BTreeSet::from([
                    IdentityAdminPermission::ReadProjectCredentials,
                ]),
                input: json_value!({}),
            },
        )
        .map_err(|_| unavailable(request, "public key metadata is unavailable"))?;
    Ok(metadata
        .into_iter()
        .find(|item| {
            item.kind() == ProjectCredentialKind::Public
                && item.state() == ProjectCredentialState::Active
        })
        .map_or_else(
            || "setup_required".to_owned(),
            |item| item.id().as_str().to_owned(),
        ))
}

fn sync_summary(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    reject_unknown_query(request, &["collectionId", "from", "until"])?;
    let tenant = tenant(request)?;
    let collection_id = query_value(request, "collectionId")?
        .map(CollectionId::parse)
        .transpose()
        .map_err(|_| invalid(request, "sync collection is invalid"))?;
    let now_hint = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| unavailable(request, "system clock is unavailable"))?;
    let until = query_value(request, "until")?
        .map(str::parse)
        .transpose()
        .map_err(|_| invalid(request, "sync window is invalid"))?
        .unwrap_or(now_hint);
    let from = query_value(request, "from")?
        .map(str::parse)
        .transpose()
        .map_err(|_| invalid(request, "sync window is invalid"))?
        .unwrap_or_else(|| until.saturating_sub(60 * 60));
    if from > until || until.saturating_sub(from) > EXPLORER_MAX_SYNC_WINDOW_SECONDS {
        return Err(invalid(request, "sync window is invalid"));
    }
    with_developer(graph, request, |actor, now| async move {
        let query = ObservabilityQuery {
            cursor: None,
            from_unix_milliseconds: Some(from.saturating_mul(1_000)),
            until_unix_milliseconds: Some(until.saturating_mul(1_000)),
            limit: 1_000,
        };
        let page = graph
            .observability_service()
            .query_replication_errors(&actor, &tenant, &query, now.saturating_mul(1_000))
            .await
            .map_err(|_| unavailable(request, "sync summary is unavailable"))?;
        let mut conflicts = 0_u64;
        let mut policy_denials = 0_u64;
        let mut throttled = 0_u64;
        let mut checkpoint_expired = 0_u64;
        let mut stream_gaps = 0_u64;
        let mut resyncs = 0_u64;
        let mut schema_mismatches = 0_u64;
        for item in &page.items {
            let ObservabilityPayload::ReplicationError {
                collection_id: record_collection,
                category,
                ..
            } = &item.payload
            else {
                continue;
            };
            if collection_id
                .as_ref()
                .is_some_and(|selected| selected.as_str() != record_collection)
            {
                continue;
            }
            match category.as_str() {
                "conflict" => conflicts += 1,
                "policy_denied" => policy_denials += 1,
                "throttled" => throttled += 1,
                "checkpoint_expired" => checkpoint_expired += 1,
                "stream_gap" => stream_gaps += 1,
                "resync" => resyncs += 1,
                "schema_mismatch" => schema_mismatches += 1,
                _ => {}
            }
        }
        json(
            request,
            200,
            &SyncSummary {
                tenant,
                collection_id,
                window_start_unix_seconds: from,
                window_end_unix_seconds: until,
                observed_at_unix_seconds: page.retention.observed_at_unix_milliseconds / 1_000,
                retained_since_unix_seconds: page.retention.retained_from_unix_milliseconds / 1_000,
                pull_count: 0,
                push_count: 0,
                live_streams: 0,
                lag_p95_milliseconds: 0,
                conflicts,
                policy_denials,
                throttled,
                checkpoint_expired,
                stream_gaps,
                resyncs,
                schema_mismatches,
                client_version_classes: BTreeMap::new(),
            },
        )
    })
}

fn list_backups(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    no_query(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let project = graph
            .project_service()
            .get_project(&actor, tenant.project_id(), now)
            .await
            .map_err(|_| not_found(request, "workspace was not found"))?;
        let membership = graph
            .organization_store()
            .get_membership(project.organization_id(), actor.identity_id())
            .await
            .map_err(|_| unavailable(request, "backup inventory is unavailable"))?
            .ok_or_else(|| not_found(request, "workspace was not found"))?;
        if !membership
            .role()
            .allows_data(ProjectDataPermission::BackupRead)
        {
            return Err(forbidden(request, "backup inventory is forbidden"));
        }
        json(request, 200, &graph.developer_backups(&tenant, now))
    })
}

fn request_restore(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let project_id = project_id(request)?;
    let idempotency = require_idempotency(request)?.to_owned();
    let body: RestoreRequestWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let tenant = TenantScope::new(
            project_id.clone(),
            mako_api::EnvironmentId::parse(&body.environment_id)
                .map_err(|_| invalid(request, "restore environment is invalid"))?,
        );
        let backup = graph
            .developer_backups(&tenant, now)
            .into_iter()
            .find(|backup| backup.backup_id == body.request.backup_id)
            .ok_or_else(|| not_found(request, "backup was not found"))?;
        let view = graph
            .developer_restore_service()
            .request(&actor, &backup, body.request, &idempotency, now)
            .await
            .map_err(|error| workspace_error(request, error))?;
        json(request, 202, &view)
    })
}

fn list_restores(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    no_query(request)?;
    let project_id = project_id(request)?;
    with_developer(graph, request, |actor, _| async move {
        let items = graph
            .developer_restore_service()
            .list(&actor, &project_id)
            .await
            .map_err(|error| workspace_error(request, error))?;
        json(request, 200, &items)
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RestoreRequestWire {
    environment_id: String,
    #[serde(flatten)]
    request: DeveloperRestoreRequest,
}

fn current_section(now: u64, payload: Value) -> WorkspaceSummarySection {
    WorkspaceSummarySection {
        status: ProviderStatus::Current,
        observed_at_unix_seconds: now,
        fresh_until_unix_seconds: now.saturating_add(60),
        retained_since_unix_seconds: None,
        payload: Some(payload),
        remediation_code: None,
    }
}

fn unavailable_section(now: u64, remediation: &str) -> WorkspaceSummarySection {
    WorkspaceSummarySection {
        status: ProviderStatus::Unavailable,
        observed_at_unix_seconds: now,
        fresh_until_unix_seconds: now,
        retained_since_unix_seconds: None,
        payload: None,
        remediation_code: Some(remediation.to_owned()),
    }
}

fn check_step(id: &str, passed: bool, remediation: &str, retryable: bool) -> ConnectionCheckStep {
    ConnectionCheckStep {
        id: id.to_owned(),
        state: if passed {
            ConnectionCheckState::Passed
        } else {
            ConnectionCheckState::Failed
        },
        remediation_code: (!passed).then(|| remediation.to_owned()),
        retryable,
    }
}

const fn lifecycle_name(value: LifecycleState) -> &'static str {
    match value {
        LifecycleState::Provisioning => "provisioning",
        LifecycleState::Active => "active",
        LifecycleState::Suspended => "suspended",
        LifecycleState::Failed => "failed",
        LifecycleState::DeletionGrace => "deletion_grace",
        LifecycleState::Deleting => "deleting",
        LifecycleState::Deleted => "deleted",
    }
}

fn workspace_error(request: &HttpRequest, error: DeveloperWorkspaceError) -> HttpApiError {
    match error {
        DeveloperWorkspaceError::InvalidRequest => invalid(request, "restore request is invalid"),
        DeveloperWorkspaceError::StepUpRequired => forbidden(
            request,
            "recent developer password verification is required",
        ),
        DeveloperWorkspaceError::QuotaExceeded => {
            conflict(request, "restore request quota is exhausted")
        }
        DeveloperWorkspaceError::NotFound => not_found(request, "restore resource was not found"),
        DeveloperWorkspaceError::Forbidden => forbidden(request, "restore request is forbidden"),
        DeveloperWorkspaceError::Conflict => conflict(request, "restore request already exists"),
        _ => unavailable(request, "restore request service is unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::{observability_summary, supported_rxdb_version};
    use mako_api::{
        EnvironmentId, EventOutcome, ObservabilityPage, ObservabilityPayload, ObservabilityRecord,
        ProjectId, QuotaResource, RetentionWindow, TenantScope,
    };
    use serde_json::json;

    fn page(payloads: Vec<(u64, ObservabilityPayload)>) -> ObservabilityPage {
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").unwrap(),
            EnvironmentId::parse("env_abcdefgh").unwrap(),
        );
        ObservabilityPage {
            items: payloads
                .into_iter()
                .map(|(timestamp, payload)| ObservabilityRecord {
                    tenant: tenant.clone(),
                    timestamp_unix_milliseconds: timestamp,
                    payload,
                })
                .collect(),
            next_cursor: None,
            retention: RetentionWindow {
                observed_at_unix_milliseconds: 10_000_000,
                retained_from_unix_milliseconds: 0,
                retention_seconds: 7_776_000,
            },
        }
    }

    #[test]
    fn overview_activity_is_newest_first_bounded_and_omits_details() {
        let page = page(
            (0..12)
                .map(|i| {
                    (
                        i * 1_000,
                        ObservabilityPayload::Audit {
                            organization_id: "org_abcdefgh".into(),
                            actor_id: "dev_abcdefgh".into(),
                            action: "collection.create".into(),
                            target: "x".repeat(500),
                            outcome: EventOutcome::Allowed,
                            request_id: "internal-request".into(),
                            details: Some("private-audit-details".into()),
                        },
                    )
                })
                .collect(),
        );
        let result = observability_summary("activity", &page, 10_000);
        assert_eq!(result["events"].as_array().unwrap().len(), 8);
        assert_eq!(result["events"][0]["timestampUnixMilliseconds"], 11_000);
        assert_eq!(result["events"][0]["target"].as_str().unwrap().len(), 256);
        assert_eq!(result["limited"], true);
        assert!(!result.to_string().contains("private-audit-details"));
        assert!(!result.to_string().contains("internal-request"));
        assert_eq!(result["windowStartUnixSeconds"], 6_400);
    }

    #[test]
    fn overview_usage_reports_latest_sample_not_sum_and_preserves_partial_results() {
        let mut page = page(vec![
            (
                3_000,
                ObservabilityPayload::Usage {
                    resource: QuotaResource::StorageBytes,
                    quantity: 40,
                    unit: "bytes".into(),
                },
            ),
            (
                1_000,
                ObservabilityPayload::Usage {
                    resource: QuotaResource::StorageBytes,
                    quantity: 100,
                    unit: "bytes".into(),
                },
            ),
        ]);
        page.next_cursor = Some("opaque-private-cursor".into());
        let result = observability_summary("usage", &page, 10_000);
        assert_eq!(result["samples"][0]["quantity"], 40);
        assert_eq!(result["samples"][0]["timestampUnixMilliseconds"], 3_000);
        assert_eq!(result["limited"], true);
        assert!(!result.to_string().contains("opaque-private-cursor"));
        let empty = observability_summary("usage", &super::tests::page(vec![]), 1);
        assert_eq!(empty["samples"], json!([]));
        assert_eq!(empty["windowStartUnixSeconds"], 0);
    }

    #[test]
    fn connection_check_accepts_only_the_supported_rxdb_major() {
        for version in [
            "17.0.0",
            "17.12.3",
            "v17.4.0",
            "17.4.0-beta.1",
            "17.4.0-beta.1+build.9",
        ] {
            assert!(supported_rxdb_version(version), "{version}");
        }
        for version in [
            "",
            "17",
            "17.0",
            "16.9.0",
            "18.0.0",
            "17.x.0",
            "17.0.0.1",
            "17.0.0-beta..1",
            "17.0.0+",
        ] {
            assert!(!supported_rxdb_version(version), "{version}");
        }
    }
}
