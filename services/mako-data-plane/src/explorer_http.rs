use std::{
    collections::BTreeMap,
    num::{NonZeroU64, NonZeroUsize},
    sync::{Arc, Mutex},
    time::Instant,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::executor::block_on;
use mako_api::{
    CollectionId, CollectionScope, EXPLORER_MAX_PAGE_BYTES, ExplorerAccessMode,
    ExplorerCapabilityClaims, ExplorerCapabilityToken, ExplorerDocumentPage, ExplorerDocumentView,
    ExplorerMutationKind, ExplorerMutationRequest, ExplorerOperation, ExplorerPageRequest,
    ExplorerPredicateOperator, ExplorerQueryPlan, ExplorerQueryRequest, ExplorerRequiredIndex,
    ExplorerRevisionView, ExplorerSimulationResult, ExplorerSort, ExplorerSortDirection,
    TenantScope,
};
use mako_audit::{ActorIdentity, AttributeValue, AuditCategory, AuditOutcome, SafeAttributes};
use mako_documents::{
    CanonicalDocument, CommitPosition, DocumentId, DocumentMutationAuthorizer, DocumentValidator,
    IndexDirection, IndexValue, IndexedQuery, MutationAuthorizationContext,
    MutationAuthorizationOperation, MutationId, PrimaryKeyBrowseOptions, QueryBound, QueryCursor,
    QueryPredicate, QuerySort, RevisionToken, ScopedCollectionSnapshot, TrustedQuery,
    TrustedQueryError,
};
use mako_identity::{AppUserId, AppUserStatus};
use mako_policy::{
    AuditRequestId, CollectionPrivileges, CredentialPrivileges, DocumentOperation,
    DocumentPolicyAuthorizer, DocumentPolicyReadAuthorizer, OperatorActorId, OperatorGrantId,
    PrivilegedAuditWriteError, PrivilegedBypassAuditContext, PrivilegedBypassAuditEvent,
    PrivilegedBypassAuditSink, PrivilegedBypassAuthorizer, PrivilegedBypassReason,
    PrivilegedBypassRequest, SafeRequestMetadata, SubjectId, VerifiedIdentity,
    VerifiedOperatorGrant, VerifiedPrivilegedPrincipal, VerifiedRole,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use mako_storage::Durability;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    DataPlaneGraph,
    auth_http::{
        append_audit, append_audit_with_details, invalid, json, now_unix_seconds, parse_json,
        unauthenticated, unavailable,
    },
    document_http::{active_collection_metadata, active_policy},
    explorer_metrics::GrantFailureClass,
};

const CAPABILITY_HEADER: &str = "x-mako-explorer-capability";
const SNAPSHOT_LIMIT: usize = 128;

type SnapshotRegistry = Arc<Mutex<BTreeMap<String, SnapshotEntry>>>;

struct SnapshotEntry {
    snapshot: Arc<ScopedCollectionSnapshot>,
    tenant: TenantScope,
    collection_id: CollectionId,
    mode: ExplorerAccessMode,
    schema_version: u64,
    authorization_epoch: u64,
    expires_at_unix_seconds: u64,
}

pub(crate) fn add_explorer_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let snapshots = Arc::new(Mutex::new(BTreeMap::new()));
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/documents/{documentId}",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::Get,
        handle_get,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/browse",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::Browse,
        handle_browse,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/query/plan",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::Plan,
        handle_plan,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/query",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::Query,
        handle_query,
    )?;
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/documents/{documentId}/history",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::History,
        handle_history,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/simulate",
        Arc::clone(&graph),
        Arc::clone(&snapshots),
        ExplorerOperation::Simulate,
        handle_simulate,
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/explorer/collections/{collectionId}/mutate",
        graph,
        snapshots,
        ExplorerOperation::Mutate,
        handle_mutate,
    )
}

type Handler =
    fn(&Arc<DataPlaneGraph>, &SnapshotRegistry, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn add_route(
    router: &mut HttpRouter,
    method: HttpMethod,
    path: &str,
    graph: Arc<DataPlaneGraph>,
    snapshots: SnapshotRegistry,
    operation: ExplorerOperation,
    handler: Handler,
) -> Result<(), RouteRegistrationError> {
    router.add_route(method, path, move |request| {
        let started = Instant::now();
        let response = handler(&graph, &snapshots, &request);
        graph
            .explorer_metrics()
            .observe_request(operation, response.is_ok(), started.elapsed());
        response
    })
}

fn handle_get(
    graph: &Arc<DataPlaneGraph>,
    _: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_no_query_or_body(request)?;
    let (tenant, scope) = request_scope(request)?;
    let document_id = document_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ExplorerOperation::Get,
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        active_collection_metadata(request, &scoped).await?;
        let document = match claims.mode {
            ExplorerAccessMode::PolicyPreview => {
                let preview = preview_context(graph, request, &tenant, &scope, &claims).await?;
                let authorizer = DocumentPolicyReadAuthorizer::new(
                    preview.policy.as_ref(),
                    &preview.identity,
                    &preview.request,
                );
                scoped
                    .get_document_authorized(&document_id, &authorizer)
                    .await
            }
            ExplorerAccessMode::Administrative => {
                let authorizer = admin_authorizer(
                    graph,
                    request,
                    &scope,
                    &claims,
                    DocumentOperation::Read,
                    now,
                )?;
                scoped
                    .get_document_authorized(&document_id, &authorizer)
                    .await
            }
        }
        .map_err(|_| unavailable(request, "explorer document storage is unavailable"))?
        .filter(|document| !document.is_deleted())
        .ok_or_else(|| not_found(request))?;
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_get",
            &fingerprint_text(document_id.as_str()),
            None,
            now,
        )
        .await?;
        json(request, 200, &document_view(&document))
    })
}

fn handle_browse(
    graph: &Arc<DataPlaneGraph>,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json_without_query(request)?;
    let body: ExplorerPageRequest = parse_json(request)?;
    body.validate()
        .map_err(|_| invalid(request, "explorer page limits are invalid"))?;
    let (tenant, scope) = request_scope(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let operation = if body.include_retained_tombstones {
            ExplorerOperation::History
        } else {
            ExplorerOperation::Browse
        };
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            operation,
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        let (handle, snapshot, after) = resolve_snapshot(
            graph,
            snapshots,
            request,
            &scoped,
            &claims,
            metadata.schema_version().get(),
            body.cursor.as_deref(),
            now,
        )
        .await?;
        let options = PrimaryKeyBrowseOptions::new(
            NonZeroUsize::new(body.limit as usize).expect("validated positive limit"),
            NonZeroUsize::new(EXPLORER_MAX_PAGE_BYTES as usize)
                .expect("page byte limit is positive"),
            scoped.storage_capabilities().maximum_scan_items,
            body.include_retained_tombstones,
        );
        let page = match claims.mode {
            ExplorerAccessMode::PolicyPreview => {
                let preview = preview_context(graph, request, &tenant, &scope, &claims).await?;
                let authorizer = DocumentPolicyReadAuthorizer::new(
                    preview.policy.as_ref(),
                    &preview.identity,
                    &preview.request,
                );
                snapshot
                    .browse_primary_keys(options, after.as_ref(), Some(&authorizer))
                    .await
            }
            ExplorerAccessMode::Administrative => {
                let authorizer = admin_authorizer(
                    graph,
                    request,
                    &scope,
                    &claims,
                    DocumentOperation::Read,
                    now,
                )?;
                snapshot
                    .browse_primary_keys(options, after.as_ref(), Some(&authorizer))
                    .await
            }
        }
        .map_err(|_| unavailable(request, "explorer browse is unavailable"))?;
        let next_cursor = page
            .next_after()
            .map(|after| {
                encode_cursor(
                    graph,
                    &ExplorerCursorWire {
                        version: 1,
                        handle: handle.clone(),
                        tenant: tenant.clone(),
                        collection_id: scope.collection_id().clone(),
                        mode: claims.mode,
                        query_fingerprint: "primary-key-browse".to_owned(),
                        schema_version: metadata.schema_version().get(),
                        snapshot_id: page.snapshot_id(),
                        authorization_epoch: claims.authorization_epoch,
                        after_document_id: after.as_str().to_owned(),
                        expires_at_unix_seconds: claims.expires_at_unix_seconds,
                    },
                )
            })
            .transpose()?;
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_browse",
            &fingerprint_text("primary-key-browse"),
            None,
            now,
        )
        .await?;
        json(
            request,
            200,
            &ExplorerDocumentPage {
                items: page.documents().iter().map(document_view).collect(),
                next_cursor,
                snapshot: handle,
                exhausted: page.exhausted(),
            },
        )
    })
}

fn handle_plan(
    graph: &Arc<DataPlaneGraph>,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    query_operation(graph, snapshots, request, true)
}

fn handle_query(
    graph: &Arc<DataPlaneGraph>,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    query_operation(graph, snapshots, request, false)
}

fn query_operation(
    graph: &Arc<DataPlaneGraph>,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
    plan_only: bool,
) -> Result<HttpResponse, HttpApiError> {
    require_json_without_query(request)?;
    let body: ExplorerQueryRequest = parse_json(request)?;
    body.validate()
        .map_err(|_| invalid(request, "explorer query limits are invalid"))?;
    let (tenant, scope) = request_scope(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let operation = if plan_only {
            ExplorerOperation::Plan
        } else {
            ExplorerOperation::Query
        };
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            operation,
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        validate_query_schema(request, metadata.json_schema(), &body)?;
        let fingerprint = query_fingerprint(&body)?;
        let plan_query = explorer_query(request, &body, None)?;
        let plan = match scoped.plan_trusted_query(&plan_query).await {
            Ok(plan) => ExplorerQueryPlan {
                supported: true,
                index_name: match plan {
                    mako_documents::QueryPlan::Index { name, .. } => Some(name.as_str().to_owned()),
                    mako_documents::QueryPlan::PrimaryKey { .. } => None,
                },
                effective_order: body.sort.clone(),
                effective_limit: body.limit,
                query_fingerprint: fingerprint.clone(),
                required_index: None,
            },
            Err(TrustedQueryError::RequiredIndex { shape }) => ExplorerQueryPlan {
                supported: false,
                index_name: None,
                effective_order: body.sort.clone(),
                effective_limit: body.limit,
                query_fingerprint: fingerprint.clone(),
                required_index: Some(ExplorerRequiredIndex {
                    fields: required_index_fields(&shape),
                }),
            },
            Err(_) => return Err(invalid(request, "explorer query is invalid")),
        };
        if !plan.supported {
            graph.explorer_metrics().observe_query_plan_failure();
        }
        if plan_only || !plan.supported {
            audit_use(
                graph,
                request,
                &tenant,
                &scope,
                &claims,
                "explorer_plan",
                &fingerprint,
                plan.index_name.as_deref(),
                now,
            )
            .await?;
            return json(request, 200, &plan);
        }
        let (handle, snapshot, inner_cursor) = resolve_query_snapshot(
            graph,
            snapshots,
            request,
            &scoped,
            &claims,
            metadata.schema_version().get(),
            &fingerprint,
            body.cursor.as_deref(),
            now,
        )
        .await?;
        let query = explorer_query(request, &body, inner_cursor.as_deref())?;
        let page = match claims.mode {
            ExplorerAccessMode::PolicyPreview => {
                let preview = preview_context(graph, request, &tenant, &scope, &claims).await?;
                let authorizer = DocumentPolicyReadAuthorizer::new(
                    preview.policy.as_ref(),
                    &preview.identity,
                    &preview.request,
                );
                snapshot.trusted_query_authorized(&query, &authorizer).await
            }
            ExplorerAccessMode::Administrative => {
                let authorizer = admin_authorizer(
                    graph,
                    request,
                    &scope,
                    &claims,
                    DocumentOperation::Read,
                    now,
                )?;
                snapshot.trusted_query_authorized(&query, &authorizer).await
            }
        }
        .map_err(|_| unavailable(request, "explorer query is unavailable"))?;
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_query",
            &fingerprint,
            plan.index_name.as_deref(),
            now,
        )
        .await?;
        let next_cursor = page
            .next_cursor()
            .map(|inner| {
                encode_query_cursor(
                    graph,
                    &ExplorerQueryCursorWire {
                        version: 1,
                        handle: handle.clone(),
                        tenant: tenant.clone(),
                        collection_id: scope.collection_id().clone(),
                        mode: claims.mode,
                        query_fingerprint: fingerprint.clone(),
                        schema_version: metadata.schema_version().get(),
                        snapshot_id: snapshot.snapshot_id(),
                        authorization_epoch: claims.authorization_epoch,
                        inner_cursor: inner.as_str().to_owned(),
                        expires_at_unix_seconds: claims.expires_at_unix_seconds,
                    },
                )
            })
            .transpose()?;
        json(
            request,
            200,
            &ExplorerDocumentPage {
                items: page.documents().iter().map(document_view).collect(),
                next_cursor,
                snapshot: handle,
                exhausted: page.next_cursor().is_none(),
            },
        )
    })
}

fn handle_history(
    graph: &Arc<DataPlaneGraph>,
    _: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_no_query_or_body(request)?;
    let (tenant, scope) = request_scope(request)?;
    let document_id = document_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ExplorerOperation::History,
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        active_collection_metadata(request, &scoped).await?;
        let high_water = scoped
            .capture_committed_high_water()
            .await
            .map_err(|_| unavailable(request, "document history is unavailable"))?;
        let mut after = 0_u64;
        let mut revisions = Vec::new();
        while after < high_water && revisions.len() < 200 {
            let page = scoped
                .read_change_page(
                    after,
                    high_water,
                    NonZeroUsize::new(200).expect("history limit"),
                )
                .await
                .map_err(|_| unavailable(request, "document history is unavailable"))?;
            for change in page.changes() {
                if change.document().primary_key() == &document_id {
                    revisions.push(ExplorerRevisionView {
                        revision: change.document().revision().as_str().to_owned(),
                        schema_version: change.document().schema_version().get(),
                        commit_position: change.document().commit_position().get(),
                        committed_at_unix_seconds: None,
                        deleted: change.document().is_deleted(),
                        retained_until_unix_seconds: None,
                    });
                }
            }
            after = page.scanned_through();
            if page.is_exhausted() {
                break;
            }
        }
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_history",
            &fingerprint_text(document_id.as_str()),
            None,
            now,
        )
        .await?;
        json(request, 200, &revisions)
    })
}

fn handle_simulate(
    graph: &Arc<DataPlaneGraph>,
    _: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json_without_query(request)?;
    let body: ExplorerMutationRequest = parse_json(request)?;
    let (tenant, scope) = request_scope(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ExplorerOperation::Simulate,
            now,
        )
        .await?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        if metadata.schema_version().get() != body.schema_version {
            return Err(invalid(request, "explorer schema version is stale"));
        }
        let validator = DocumentValidator::compile(&metadata)
            .map_err(|_| unavailable(request, "collection schema is unavailable"))?;
        let id = DocumentId::parse(&body.document_id)
            .map_err(|_| invalid(request, "document id is invalid"))?;
        let current = scoped
            .get_document(&id)
            .await
            .map_err(|_| unavailable(request, "document storage is unavailable"))?;
        let (validated, conflict) =
            validate_simulation(request, &validator, &body, current.as_ref())?;
        let mutation_id = MutationId::parse(&body.idempotency_key)
            .map_err(|_| invalid(request, "idempotency key is invalid"))?;
        let proposed = CanonicalDocument::new(
            id,
            metadata.schema_version(),
            mutation_id.revision_for(validated.primary_key()),
            CommitPosition::new(
                scoped
                    .capture_committed_high_water()
                    .await
                    .unwrap_or_default()
                    .saturating_add(1)
                    .max(1),
            )
            .map_err(|_| unavailable(request, "document simulation is unavailable"))?,
            body.kind == ExplorerMutationKind::Delete,
            Value::Object(validated.body().clone()),
        )
        .map_err(|_| invalid(request, "document simulation is invalid"))?;
        let operation = mutation_operation(body.kind);
        let allowed = match claims.mode {
            ExplorerAccessMode::PolicyPreview => {
                let preview = preview_context(graph, request, &tenant, &scope, &claims).await?;
                let authorizer = DocumentPolicyAuthorizer::new(
                    preview.policy.as_ref(),
                    &preview.identity,
                    &preview.request,
                );
                authorizer
                    .authorize(MutationAuthorizationContext::new(
                        &scoped,
                        operation,
                        current.as_ref(),
                        &proposed,
                    ))
                    .is_allowed()
            }
            ExplorerAccessMode::Administrative => {
                let authorizer = admin_authorizer(
                    graph,
                    request,
                    &scope,
                    &claims,
                    document_operation(body.kind),
                    now,
                )?;
                authorizer
                    .authorize(MutationAuthorizationContext::new(
                        &scoped,
                        operation,
                        current.as_ref(),
                        &proposed,
                    ))
                    .is_allowed()
            }
        };
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_simulate",
            &fingerprint_text(&body.document_id),
            None,
            now,
        )
        .await?;
        json(
            request,
            200,
            &ExplorerSimulationResult {
                allowed,
                schema_valid: true,
                diagnostics: Vec::new(),
                would_conflict: conflict,
            },
        )
    })
}

fn handle_mutate(
    graph: &Arc<DataPlaneGraph>,
    _: &SnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    require_json_without_query(request)?;
    let body: ExplorerMutationRequest = parse_json(request)?;
    let (tenant, scope) = request_scope(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let claims = authorize_capability(
            graph,
            request,
            &tenant,
            scope.collection_id(),
            ExplorerOperation::Mutate,
            now,
        )
        .await?;
        if claims.mode != ExplorerAccessMode::Administrative {
            return Err(permission_denied(
                request,
                "policy preview cannot commit mutations",
            ));
        }
        if request.header("idempotency-key") != Some(body.idempotency_key.as_str()) {
            return Err(invalid(
                request,
                "idempotency key must match the mutation request",
            ));
        }
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "explorer scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        if metadata.schema_version().get() != body.schema_version {
            return Err(invalid(request, "explorer schema version is stale"));
        }
        let validator = DocumentValidator::compile(&metadata)
            .map_err(|_| unavailable(request, "collection schema is unavailable"))?;
        let document_id = DocumentId::parse(&body.document_id)
            .map_err(|_| invalid(request, "document id is invalid"))?;
        let current = scoped
            .get_document(&document_id)
            .await
            .map_err(|_| unavailable(request, "document storage is unavailable"))?;
        let (validated, _) = validate_simulation(request, &validator, &body, current.as_ref())?;
        if validated.primary_key() != &document_id {
            return Err(invalid(
                request,
                "document primary key does not match the request",
            ));
        }
        let mutation_id = MutationId::parse(&body.idempotency_key)
            .map_err(|_| invalid(request, "idempotency key is invalid"))?;
        let sequencer = graph
            .document_engine()
            .scope_sequencer(&tenant, &tenant, Durability::Sync)
            .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("one is non-zero"))
            .await
            .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
        let position = lease.issue().expect("one-position lease issues once");
        let input = mako_documents::MutationInput {
            mutation_id,
            commit_position: CommitPosition::new(position)
                .map_err(|_| unavailable(request, "document sequencer is unavailable"))?,
            document: validated,
            durability: Durability::Sync,
        };
        let authorizer = admin_authorizer(
            graph,
            request,
            &scope,
            &claims,
            document_operation(body.kind),
            now,
        )?;
        let outcome = match body.kind {
            ExplorerMutationKind::Create => {
                if body.expected_revision.is_some() {
                    return Err(invalid(request, "create cannot have an expected revision"));
                }
                scoped.create_document_authorized(input, &authorizer).await
            }
            ExplorerMutationKind::Update => {
                scoped
                    .update_document_authorized(
                        expected_revision(request, &body)?,
                        input,
                        &authorizer,
                    )
                    .await
            }
            ExplorerMutationKind::Delete => {
                scoped
                    .delete_document_authorized(
                        expected_revision(request, &body)?,
                        input,
                        &authorizer,
                    )
                    .await
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_) => {
                let _ = sequencer.mark_aborted(position).await;
                let _ = sequencer.recover_high_water().await;
                return Err(unavailable(request, "document mutation failed"));
            }
        };
        let (committed, conflict) = match outcome {
            mako_documents::MutationCommitOutcome::Applied(_) => {
                sequencer
                    .mark_committed(position)
                    .await
                    .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
                let document = scoped
                    .get_document(&document_id)
                    .await
                    .map_err(|_| unavailable(request, "document storage is unavailable"))?
                    .map(|document| document_view(&document));
                (document, None)
            }
            mako_documents::MutationCommitOutcome::Replayed(_) => {
                sequencer
                    .mark_aborted(position)
                    .await
                    .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
                let document = scoped
                    .get_document(&document_id)
                    .await
                    .map_err(|_| unavailable(request, "document storage is unavailable"))?
                    .map(|document| document_view(&document));
                (document, None)
            }
            mako_documents::MutationCommitOutcome::RevisionConflict { .. } => {
                sequencer
                    .mark_aborted(position)
                    .await
                    .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
                let current = scoped
                    .get_document(&document_id)
                    .await
                    .map_err(|_| unavailable(request, "document storage is unavailable"))?
                    .map(|document| document_view(&document));
                (None, current)
            }
        };
        if conflict.is_some() {
            graph.explorer_metrics().observe_conflict();
        }
        sequencer
            .recover_high_water()
            .await
            .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
        audit_use(
            graph,
            request,
            &tenant,
            &scope,
            &claims,
            "explorer_mutate",
            &fingerprint_text(&body.document_id),
            None,
            now,
        )
        .await?;
        json(
            request,
            200,
            &mako_api::ExplorerMutationResult {
                committed: committed.is_some(),
                document: committed,
                conflict,
                audit_reference: request.request_id().to_owned(),
            },
        )
    })
}

fn expected_revision(
    request: &HttpRequest,
    body: &ExplorerMutationRequest,
) -> Result<RevisionToken, HttpApiError> {
    RevisionToken::parse(
        body.expected_revision
            .as_deref()
            .ok_or_else(|| invalid(request, "expected revision is required"))?,
    )
    .map_err(|_| invalid(request, "expected revision is invalid"))
}

async fn authorize_capability(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    operation: ExplorerOperation,
    now: u64,
) -> Result<ExplorerCapabilityClaims, HttpApiError> {
    let Some(presented) = request.header(CAPABILITY_HEADER) else {
        graph
            .explorer_metrics()
            .observe_grant_failure(GrantFailureClass::Missing);
        audit_denied(
            graph,
            request,
            tenant,
            collection_id,
            ActorIdentity::Anonymous,
            operation,
            "missing_capability",
            None,
            now,
        )
        .await?;
        return Err(unauthenticated(request, "explorer capability is required"));
    };
    let token = match ExplorerCapabilityToken::parse(presented.to_owned()) {
        Ok(token) => token,
        Err(_) => {
            graph
                .explorer_metrics()
                .observe_grant_failure(GrantFailureClass::Malformed);
            audit_denied(
                graph,
                request,
                tenant,
                collection_id,
                ActorIdentity::Anonymous,
                operation,
                "malformed_capability",
                None,
                now,
            )
            .await?;
            return Err(unauthenticated(request, "explorer capability is invalid"));
        }
    };
    let claims = match graph.explorer_capability_keys().verify(&token, now) {
        Ok(claims) => claims,
        Err(_) => {
            graph
                .explorer_metrics()
                .observe_grant_failure(GrantFailureClass::Unverified);
            audit_denied(
                graph,
                request,
                tenant,
                collection_id,
                ActorIdentity::Anonymous,
                operation,
                "unverified_capability",
                None,
                now,
            )
            .await?;
            return Err(unauthenticated(request, "explorer capability is invalid"));
        }
    };
    if &claims.tenant != tenant
        || &claims.collection_id != collection_id
        || !claims.operations.contains(&operation)
    {
        graph
            .explorer_metrics()
            .observe_grant_failure(GrantFailureClass::ScopeOrOperation);
        audit_denied(
            graph,
            request,
            tenant,
            collection_id,
            ActorIdentity::Developer {
                actor_id: claims.developer_identity_id.clone(),
            },
            operation,
            "scope_or_operation_denied",
            Some(&claims),
            now,
        )
        .await?;
        return Err(permission_denied(
            request,
            "explorer capability does not permit this operation",
        ));
    }
    if graph
        .explorer_authorization_store(tenant, tenant)
        .map_err(|_| unavailable(request, "explorer authorization is unavailable"))?
        .validate_grant(
            &claims.nonce,
            &claims.developer_identity_id,
            claims.mode,
            claims.authorization_epoch,
            now,
        )
        .await
        .is_err()
    {
        graph
            .explorer_metrics()
            .observe_grant_failure(GrantFailureClass::RevokedOrStale);
        audit_denied(
            graph,
            request,
            tenant,
            collection_id,
            ActorIdentity::Developer {
                actor_id: claims.developer_identity_id.clone(),
            },
            operation,
            "revoked_or_stale_grant",
            Some(&claims),
            now,
        )
        .await?;
        return Err(unauthenticated(request, "explorer capability is invalid"));
    }
    graph
        .explorer_metrics()
        .observe_authorized_mode(claims.mode);
    Ok(claims)
}

#[allow(clippy::too_many_arguments)]
async fn audit_denied(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    actor: ActorIdentity,
    operation: ExplorerOperation,
    failure: &str,
    claims: Option<&ExplorerCapabilityClaims>,
    now: u64,
) -> Result<(), HttpApiError> {
    let mut values = vec![
        (
            "operation".to_owned(),
            AttributeValue::Text(explorer_operation_name(operation).to_owned()),
        ),
        (
            "grant_failure".to_owned(),
            AttributeValue::Text(failure.to_owned()),
        ),
    ];
    if let Some(claims) = claims {
        values.extend(explorer_audit_claim_attributes(claims));
    }
    let details = SafeAttributes::try_from_iter(values)
        .map_err(|_| unavailable(request, "explorer audit is unavailable"))?;
    append_audit_with_details(
        graph,
        tenant,
        AuditCategory::Policy,
        actor,
        "collection",
        collection_id.as_str(),
        "explorer_denied",
        AuditOutcome::Denied,
        failure,
        request.request_id(),
        now,
        details,
    )
    .await
}

struct PreviewContext {
    policy: Option<mako_policy::CompiledPolicySet>,
    identity: VerifiedIdentity,
    request: SafeRequestMetadata,
}

async fn preview_context(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    scope: &CollectionScope,
    claims: &ExplorerCapabilityClaims,
) -> Result<PreviewContext, HttpApiError> {
    if claims.mode != ExplorerAccessMode::PolicyPreview {
        return Err(permission_denied(request, "explorer mode is invalid"));
    }
    let user_id = AppUserId::parse(claims.application_user_id.as_deref().unwrap_or_default())
        .map_err(|_| unauthenticated(request, "preview application user is invalid"))?;
    let user = graph
        .identity_store(tenant, tenant)
        .map_err(|_| unavailable(request, "application-user authority is unavailable"))?
        .user_by_id(&user_id)
        .await
        .map_err(|_| unavailable(request, "application-user authority is unavailable"))?
        .filter(|user| user.status() == AppUserStatus::Active)
        .ok_or_else(|| unauthenticated(request, "preview application user is invalid"))?;
    let role = user
        .trusted_metadata()
        .values()
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("authenticated");
    let identity = VerifiedIdentity::user(
        SubjectId::parse(user_id.as_str())
            .map_err(|_| unauthenticated(request, "preview application user is invalid"))?,
        VerifiedRole::parse(role)
            .map_err(|_| unauthenticated(request, "preview application user is invalid"))?,
        Value::Object(user.trusted_metadata().values().clone()),
    )
    .map_err(|_| unauthenticated(request, "preview application user is invalid"))?;
    let scoped = graph
        .document_engine()
        .scope_collection(tenant, scope.clone())
        .map_err(|_| invalid(request, "explorer scope is invalid"))?;
    Ok(PreviewContext {
        policy: active_policy(graph, request, tenant, scope, &scoped).await?,
        identity,
        request: SafeRequestMetadata::new([
            (
                "method".to_owned(),
                method_name(request.method()).to_owned(),
            ),
            ("request_id".to_owned(), request.request_id().to_owned()),
            ("explorer_mode".to_owned(), "policy_preview".to_owned()),
        ])
        .map_err(|_| invalid(request, "explorer request metadata is invalid"))?,
    })
}

fn admin_authorizer(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    scope: &CollectionScope,
    claims: &ExplorerCapabilityClaims,
    operation: DocumentOperation,
    now: u64,
) -> Result<PrivilegedBypassAuthorizer, HttpApiError> {
    if claims.mode != ExplorerAccessMode::Administrative {
        return Err(permission_denied(request, "explorer mode is invalid"));
    }
    let privileges = CredentialPrivileges::new(
        CollectionPrivileges::only([scope.collection_id().clone()])
            .map_err(|_| permission_denied(request, "explorer scope is invalid"))?,
        [operation],
    )
    .map_err(|_| permission_denied(request, "explorer operation is invalid"))?;
    let grant = VerifiedOperatorGrant::from_verified_grant(
        OperatorGrantId::parse(grant_id(&claims.nonce))
            .map_err(|_| unauthenticated(request, "explorer capability is invalid"))?,
        OperatorActorId::parse(claims.developer_identity_id.clone())
            .map_err(|_| unauthenticated(request, "explorer capability is invalid"))?,
        claims.tenant.clone(),
        privileges,
        claims.expires_at_unix_seconds,
    );
    let reason_hash = claims.reason_hash.as_deref().unwrap_or_default();
    PrivilegedBypassAuthorizer::establish(
        PrivilegedBypassRequest::new(
            VerifiedPrivilegedPrincipal::Operator(grant),
            scope.clone(),
            operation,
            PrivilegedBypassAuditContext::new(
                AuditRequestId::parse(request.request_id())
                    .map_err(|_| invalid(request, "explorer request id is invalid"))?,
                PrivilegedBypassReason::parse(format!("explorer-reason-hash:{reason_hash}"))
                    .map_err(|_| invalid(request, "explorer reason is invalid"))?,
            ),
            now,
        ),
        &ExplorerBypassAudit {
            graph: Arc::clone(graph),
        },
    )
    .map_err(|_| permission_denied(request, "explorer administrative access was denied"))
}

struct ExplorerBypassAudit {
    graph: Arc<DataPlaneGraph>,
}

impl PrivilegedBypassAuditSink for ExplorerBypassAudit {
    fn record(&self, event: &PrivilegedBypassAuditEvent) -> Result<(), PrivilegedAuditWriteError> {
        let now = now_unix_seconds(event.request_id().as_str())
            .map_err(|_| PrivilegedAuditWriteError::new("explorer bypass audit failed"))?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(append_audit(
                        &self.graph,
                        event.scope().tenant(),
                        AuditCategory::ServiceBypass,
                        ActorIdentity::Developer {
                            actor_id: event.actor_id().to_owned(),
                        },
                        "collection",
                        event.scope().collection_id().as_str(),
                        "explorer_administrative_bypass",
                        AuditOutcome::Allowed,
                        "reason_hash_recorded",
                        event.request_id().as_str(),
                        now,
                    ))
                })
                .join()
        })
        .map_err(|_| PrivilegedAuditWriteError::new("explorer bypass audit worker failed"))?
        .map_err(|_| PrivilegedAuditWriteError::new("explorer bypass audit failed"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn audit_use(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    scope: &CollectionScope,
    claims: &ExplorerCapabilityClaims,
    action: &str,
    target_fingerprint: &str,
    index_name: Option<&str>,
    now: u64,
) -> Result<(), HttpApiError> {
    let mut attributes = explorer_audit_claim_attributes(claims);
    attributes.extend([
        (
            "operation".to_owned(),
            AttributeValue::Text(action.to_owned()),
        ),
        (
            "target_fingerprint".to_owned(),
            AttributeValue::Text(target_fingerprint.to_owned()),
        ),
    ]);
    if let Some(index_name) = index_name {
        attributes.push((
            "index".to_owned(),
            AttributeValue::Text(index_name.to_owned()),
        ));
    }
    let details = SafeAttributes::try_from_iter(attributes)
        .map_err(|_| unavailable(request, "explorer audit is unavailable"))?;
    append_audit_with_details(
        graph,
        tenant,
        AuditCategory::Policy,
        ActorIdentity::Developer {
            actor_id: claims.developer_identity_id.clone(),
        },
        "collection",
        scope.collection_id().as_str(),
        action,
        AuditOutcome::Allowed,
        match claims.mode {
            ExplorerAccessMode::PolicyPreview => "policy_preview",
            ExplorerAccessMode::Administrative => "administrative",
        },
        request.request_id(),
        now,
        details,
    )
    .await
}

fn explorer_audit_claim_attributes(
    claims: &ExplorerCapabilityClaims,
) -> Vec<(String, AttributeValue)> {
    vec![
        (
            "grant_fingerprint".to_owned(),
            AttributeValue::Text(fingerprint_text(&claims.nonce)),
        ),
        (
            "reason_hash".to_owned(),
            AttributeValue::Text(
                claims
                    .reason_hash
                    .clone()
                    .unwrap_or_else(|| "none".to_owned()),
            ),
        ),
        (
            "mode".to_owned(),
            AttributeValue::Text(match claims.mode {
                ExplorerAccessMode::PolicyPreview => "policy_preview".to_owned(),
                ExplorerAccessMode::Administrative => "administrative".to_owned(),
            }),
        ),
        (
            "authorization_epoch".to_owned(),
            AttributeValue::Unsigned(claims.authorization_epoch),
        ),
    ]
}

fn fingerprint_text(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

const fn explorer_operation_name(operation: ExplorerOperation) -> &'static str {
    match operation {
        ExplorerOperation::Get => "get",
        ExplorerOperation::Browse => "browse",
        ExplorerOperation::Query => "query",
        ExplorerOperation::Plan => "plan",
        ExplorerOperation::History => "history",
        ExplorerOperation::Simulate => "simulate",
        ExplorerOperation::Mutate => "mutate",
        ExplorerOperation::Import => "import",
        ExplorerOperation::Export => "export",
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_snapshot(
    graph: &DataPlaneGraph,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
    scoped: &mako_documents::ScopedCollectionEngine,
    claims: &ExplorerCapabilityClaims,
    schema_version: u64,
    cursor: Option<&str>,
    now: u64,
) -> Result<(String, Arc<ScopedCollectionSnapshot>, Option<DocumentId>), HttpApiError> {
    if let Some(cursor) = cursor {
        let wire = decode_cursor(graph, request, cursor)?;
        if wire.tenant != claims.tenant
            || wire.collection_id != claims.collection_id
            || wire.mode != claims.mode
            || wire.query_fingerprint != "primary-key-browse"
            || wire.schema_version != schema_version
            || wire.authorization_epoch != claims.authorization_epoch
            || now >= wire.expires_at_unix_seconds
        {
            return Err(invalid(request, "explorer cursor is invalid"));
        }
        let registry = snapshots
            .lock()
            .map_err(|_| unavailable(request, "explorer snapshots are unavailable"))?;
        let entry = registry
            .get(&wire.handle)
            .filter(|entry| {
                entry.tenant == claims.tenant
                    && entry.collection_id == claims.collection_id
                    && entry.mode == claims.mode
                    && entry.schema_version == schema_version
                    && entry.authorization_epoch == claims.authorization_epoch
                    && entry.expires_at_unix_seconds > now
                    && entry.snapshot.snapshot_id() == wire.snapshot_id
            })
            .ok_or_else(|| invalid(request, "explorer cursor is expired"))?;
        let after = DocumentId::parse(wire.after_document_id)
            .map_err(|_| invalid(request, "explorer cursor is invalid"))?;
        return Ok((wire.handle, Arc::clone(&entry.snapshot), Some(after)));
    }
    let snapshot = Arc::new(
        scoped
            .snapshot()
            .await
            .map_err(|_| unavailable(request, "explorer snapshot is unavailable"))?,
    );
    let handle = format!(
        "xsnap_{}",
        &blake3::hash(format!("{}:{}", request.request_id(), claims.nonce).as_bytes()).to_hex()
            [..24]
    );
    let mut registry = snapshots
        .lock()
        .map_err(|_| unavailable(request, "explorer snapshots are unavailable"))?;
    registry.retain(|_, entry| entry.expires_at_unix_seconds > now);
    if registry.len() >= SNAPSHOT_LIMIT {
        return Err(unavailable(
            request,
            "explorer snapshot capacity is exhausted",
        ));
    }
    registry.insert(
        handle.clone(),
        SnapshotEntry {
            snapshot: Arc::clone(&snapshot),
            tenant: claims.tenant.clone(),
            collection_id: claims.collection_id.clone(),
            mode: claims.mode,
            schema_version,
            authorization_epoch: claims.authorization_epoch,
            expires_at_unix_seconds: claims.expires_at_unix_seconds,
        },
    );
    Ok((handle, snapshot, None))
}

#[allow(clippy::too_many_arguments)]
async fn resolve_query_snapshot(
    graph: &DataPlaneGraph,
    snapshots: &SnapshotRegistry,
    request: &HttpRequest,
    scoped: &mako_documents::ScopedCollectionEngine,
    claims: &ExplorerCapabilityClaims,
    schema_version: u64,
    query_fingerprint: &str,
    cursor: Option<&str>,
    now: u64,
) -> Result<(String, Arc<ScopedCollectionSnapshot>, Option<String>), HttpApiError> {
    if let Some(cursor) = cursor {
        let wire = decode_query_cursor(graph, request, cursor)?;
        if wire.tenant != claims.tenant
            || wire.collection_id != claims.collection_id
            || wire.mode != claims.mode
            || wire.query_fingerprint != query_fingerprint
            || wire.schema_version != schema_version
            || wire.authorization_epoch != claims.authorization_epoch
            || now >= wire.expires_at_unix_seconds
        {
            return Err(invalid(request, "explorer cursor is invalid"));
        }
        let registry = snapshots
            .lock()
            .map_err(|_| unavailable(request, "explorer snapshots are unavailable"))?;
        let entry = registry
            .get(&wire.handle)
            .filter(|entry| {
                entry.tenant == claims.tenant
                    && entry.collection_id == claims.collection_id
                    && entry.mode == claims.mode
                    && entry.schema_version == schema_version
                    && entry.authorization_epoch == claims.authorization_epoch
                    && entry.expires_at_unix_seconds > now
                    && entry.snapshot.snapshot_id() == wire.snapshot_id
            })
            .ok_or_else(|| invalid(request, "explorer cursor is expired"))?;
        return Ok((
            wire.handle,
            Arc::clone(&entry.snapshot),
            Some(wire.inner_cursor),
        ));
    }
    let snapshot = Arc::new(
        scoped
            .snapshot()
            .await
            .map_err(|_| unavailable(request, "explorer snapshot is unavailable"))?,
    );
    let handle = format!(
        "xsnap_{}",
        &blake3::hash(format!("query:{}:{}", request.request_id(), claims.nonce).as_bytes())
            .to_hex()[..24]
    );
    let mut registry = snapshots
        .lock()
        .map_err(|_| unavailable(request, "explorer snapshots are unavailable"))?;
    registry.retain(|_, entry| entry.expires_at_unix_seconds > now);
    if registry.len() >= SNAPSHOT_LIMIT {
        return Err(unavailable(
            request,
            "explorer snapshot capacity is exhausted",
        ));
    }
    registry.insert(
        handle.clone(),
        SnapshotEntry {
            snapshot: Arc::clone(&snapshot),
            tenant: claims.tenant.clone(),
            collection_id: claims.collection_id.clone(),
            mode: claims.mode,
            schema_version,
            authorization_epoch: claims.authorization_epoch,
            expires_at_unix_seconds: claims.expires_at_unix_seconds,
        },
    );
    Ok((handle, snapshot, None))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExplorerCursorWire {
    version: u8,
    handle: String,
    tenant: TenantScope,
    collection_id: CollectionId,
    mode: ExplorerAccessMode,
    query_fingerprint: String,
    schema_version: u64,
    snapshot_id: u64,
    authorization_epoch: u64,
    after_document_id: String,
    expires_at_unix_seconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExplorerQueryCursorWire {
    version: u8,
    handle: String,
    tenant: TenantScope,
    collection_id: CollectionId,
    mode: ExplorerAccessMode,
    query_fingerprint: String,
    schema_version: u64,
    snapshot_id: u64,
    authorization_epoch: u64,
    inner_cursor: String,
    expires_at_unix_seconds: u64,
}

fn encode_cursor(
    graph: &DataPlaneGraph,
    wire: &ExplorerCursorWire,
) -> Result<String, HttpApiError> {
    let payload = serde_json::to_vec(wire).map_err(|_| {
        HttpApiError::new(
            500,
            mako_api::ErrorCode::Internal,
            "explorer cursor could not be created",
            "req_explorer_cursor",
            mako_api::RetryAdvice::Never,
        )
    })?;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let signature = blake3::keyed_hash(graph.explorer_cursor_key(), encoded.as_bytes());
    Ok(format!(
        "xc1_{encoded}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_bytes())
    ))
}

fn decode_cursor(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    cursor: &str,
) -> Result<ExplorerCursorWire, HttpApiError> {
    let (payload, signature) = cursor
        .strip_prefix("xc1_")
        .and_then(|value| value.split_once('.'))
        .ok_or_else(|| invalid(request, "explorer cursor is invalid"))?;
    if payload.len() > 8_192 || signature.len() > 128 {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    let actual = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid(request, "explorer cursor is invalid"))?;
    let expected = blake3::keyed_hash(graph.explorer_cursor_key(), payload.as_bytes());
    if !constant_time_eq(&actual, expected.as_bytes()) {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    let wire: ExplorerCursorWire = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| invalid(request, "explorer cursor is invalid"))?,
    )
    .map_err(|_| invalid(request, "explorer cursor is invalid"))?;
    if wire.version != 1 {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    Ok(wire)
}

fn encode_query_cursor(
    graph: &DataPlaneGraph,
    wire: &ExplorerQueryCursorWire,
) -> Result<String, HttpApiError> {
    let payload = serde_json::to_vec(wire).map_err(|_| {
        HttpApiError::new(
            500,
            mako_api::ErrorCode::Internal,
            "explorer cursor could not be created",
            "req_explorer_query_cursor",
            mako_api::RetryAdvice::Never,
        )
    })?;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let signature = blake3::keyed_hash(graph.explorer_cursor_key(), encoded.as_bytes());
    Ok(format!(
        "xq1_{encoded}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_bytes())
    ))
}

fn decode_query_cursor(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    cursor: &str,
) -> Result<ExplorerQueryCursorWire, HttpApiError> {
    let (payload, signature) = cursor
        .strip_prefix("xq1_")
        .and_then(|value| value.split_once('.'))
        .ok_or_else(|| invalid(request, "explorer cursor is invalid"))?;
    if payload.len() > 8_192 || signature.len() > 128 {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    let actual = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid(request, "explorer cursor is invalid"))?;
    let expected = blake3::keyed_hash(graph.explorer_cursor_key(), payload.as_bytes());
    if !constant_time_eq(&actual, expected.as_bytes()) {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    let wire: ExplorerQueryCursorWire = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| invalid(request, "explorer cursor is invalid"))?,
    )
    .map_err(|_| invalid(request, "explorer cursor is invalid"))?;
    if wire.version != 1 || wire.inner_cursor.len() > 4_096 {
        return Err(invalid(request, "explorer cursor is invalid"));
    }
    Ok(wire)
}

fn explorer_query(
    request: &HttpRequest,
    body: &ExplorerQueryRequest,
    inner_cursor: Option<&str>,
) -> Result<TrustedQuery, HttpApiError> {
    let mut grouped: BTreeMap<String, PredicateParts> = BTreeMap::new();
    for predicate in &body.predicates {
        let value = IndexValue::from_json(&predicate.value)
            .map_err(|_| invalid(request, "explorer query value is invalid"))?;
        let parts = grouped.entry(predicate.field.clone()).or_default();
        match predicate.operator {
            ExplorerPredicateOperator::Equal if parts.equal.is_none() => parts.equal = Some(value),
            ExplorerPredicateOperator::GreaterThan if parts.lower.is_none() => {
                parts.lower = Some(QueryBound::exclusive(value));
            }
            ExplorerPredicateOperator::GreaterThanOrEqual if parts.lower.is_none() => {
                parts.lower = Some(QueryBound::inclusive(value));
            }
            ExplorerPredicateOperator::LessThan if parts.upper.is_none() => {
                parts.upper = Some(QueryBound::exclusive(value));
            }
            ExplorerPredicateOperator::LessThanOrEqual if parts.upper.is_none() => {
                parts.upper = Some(QueryBound::inclusive(value));
            }
            _ => return Err(invalid(request, "explorer query predicates conflict")),
        }
    }
    let mut predicates = Vec::new();
    for (field, parts) in grouped {
        let predicate = match (parts.equal, parts.lower, parts.upper) {
            (Some(value), None, None) => QueryPredicate::equal(field, value),
            (None, Some(lower), Some(upper)) => QueryPredicate::bounded_range(field, lower, upper),
            (None, Some(lower), None) if lower.is_inclusive() => {
                QueryPredicate::greater_than_or_equal(field, lower.into_value())
            }
            (None, Some(lower), None) => QueryPredicate::greater_than(field, lower.into_value()),
            (None, None, Some(upper)) if upper.is_inclusive() => {
                QueryPredicate::less_than_or_equal(field, upper.into_value())
            }
            (None, None, Some(upper)) => QueryPredicate::less_than(field, upper.into_value()),
            _ => return Err(invalid(request, "explorer query predicate is invalid")),
        }
        .map_err(|_| invalid(request, "explorer query predicate is invalid"))?;
        predicates.push(predicate);
    }
    let sort = body
        .sort
        .iter()
        .map(|sort| {
            QuerySort::new(
                &sort.field,
                match sort.direction {
                    ExplorerSortDirection::Ascending => IndexDirection::Ascending,
                    ExplorerSortDirection::Descending => IndexDirection::Descending,
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid(request, "explorer query sort is invalid"))?;
    let mut query = IndexedQuery::new(
        predicates,
        sort,
        NonZeroUsize::new(body.limit as usize).expect("validated query limit"),
    )
    .map_err(|_| invalid(request, "explorer query is invalid"))?;
    if let Some(cursor) = inner_cursor {
        query = query.with_cursor(
            QueryCursor::parse(cursor)
                .map_err(|_| invalid(request, "explorer cursor is invalid"))?,
        );
    }
    Ok(TrustedQuery::indexed(query))
}

#[derive(Default)]
struct PredicateParts {
    equal: Option<IndexValue>,
    lower: Option<QueryBound>,
    upper: Option<QueryBound>,
}

fn validate_query_schema(
    request: &HttpRequest,
    schema: &serde_json::Map<String, Value>,
    query: &ExplorerQueryRequest,
) -> Result<(), HttpApiError> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(request, "collection schema has no queryable properties"))?;
    for predicate in &query.predicates {
        let expected = properties
            .get(&predicate.field)
            .and_then(Value::as_object)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(request, "explorer query field is not in the active schema"))?;
        let matches = matches!(
            (expected, &predicate.value),
            ("string", Value::String(_))
                | ("number" | "integer", Value::Number(_))
                | ("boolean", Value::Bool(_))
                | (_, Value::Null)
        );
        if !matches {
            return Err(invalid(
                request,
                "explorer query value does not match the active schema",
            ));
        }
    }
    if query
        .sort
        .iter()
        .any(|sort| !properties.contains_key(&sort.field))
    {
        return Err(invalid(
            request,
            "explorer sort field is not in the active schema",
        ));
    }
    Ok(())
}

fn required_index_fields(shape: &mako_documents::RequiredIndexShape) -> Vec<ExplorerSort> {
    let mut fields = shape
        .equality_fields()
        .iter()
        .map(|field| ExplorerSort {
            field: field.clone(),
            direction: ExplorerSortDirection::Ascending,
        })
        .collect::<Vec<_>>();
    if let Some(field) = shape.range_field() {
        fields.push(ExplorerSort {
            field: field.to_owned(),
            direction: ExplorerSortDirection::Ascending,
        });
    }
    fields.extend(shape.sort().iter().map(|sort| ExplorerSort {
        field: sort.field().to_owned(),
        direction: match sort.direction() {
            IndexDirection::Ascending => ExplorerSortDirection::Ascending,
            IndexDirection::Descending => ExplorerSortDirection::Descending,
        },
    }));
    fields
}

fn validate_simulation(
    request: &HttpRequest,
    validator: &DocumentValidator,
    body: &ExplorerMutationRequest,
    current: Option<&CanonicalDocument>,
) -> Result<(mako_documents::ValidatedDocumentBody, bool), HttpApiError> {
    let expected_matches = match (&body.expected_revision, current) {
        (None, None) => true,
        (Some(expected), Some(current)) => expected == current.revision().as_str(),
        _ => false,
    };
    let content = body.content.clone().ok_or_else(|| {
        invalid(
            request,
            "document content is required for mutation simulation",
        )
    })?;
    let validated = match body.kind {
        ExplorerMutationKind::Create => validator
            .validate_create(content)
            .map_err(|_| invalid(request, "document does not match the active schema"))?,
        ExplorerMutationKind::Update | ExplorerMutationKind::Delete => validator
            .validate_update(
                current.ok_or_else(|| invalid(request, "document does not exist"))?,
                content,
            )
            .map_err(|_| invalid(request, "document does not match the active schema"))?,
    };
    Ok((validated, !expected_matches))
}

fn query_fingerprint(body: &ExplorerQueryRequest) -> Result<String, HttpApiError> {
    let mut canonical = body.clone();
    canonical.cursor = None;
    serde_json::to_vec(&canonical)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .map_err(|_| {
            HttpApiError::new(
                500,
                mako_api::ErrorCode::Internal,
                "explorer query fingerprint is unavailable",
                "req_explorer_query",
                mako_api::RetryAdvice::Never,
            )
        })
}

fn document_view(document: &CanonicalDocument) -> ExplorerDocumentView {
    ExplorerDocumentView {
        document_id: document.primary_key().as_str().to_owned(),
        revision: document.revision().as_str().to_owned(),
        schema_version: document.schema_version().get(),
        deleted: document.is_deleted(),
        content: (!document.is_deleted()).then(|| Value::Object(document.body().clone())),
    }
}

fn request_scope(request: &HttpRequest) -> Result<(TenantScope, CollectionScope), HttpApiError> {
    let tenant = crate::auth_http::tenant(request)?;
    let collection = CollectionId::parse(
        request
            .path_parameter("collectionId")
            .ok_or_else(|| invalid(request, "collection path is invalid"))?,
    )
    .map_err(|_| invalid(request, "collection path is invalid"))?;
    Ok((tenant.clone(), CollectionScope::new(tenant, collection)))
}

fn document_id(request: &HttpRequest) -> Result<DocumentId, HttpApiError> {
    DocumentId::parse(
        request
            .path_parameter("documentId")
            .ok_or_else(|| invalid(request, "document path is invalid"))?,
    )
    .map_err(|_| invalid(request, "document path is invalid"))
}

fn require_json_without_query(request: &HttpRequest) -> Result<(), HttpApiError> {
    if !request.query().is_empty() {
        return Err(invalid(request, "query parameters are not supported"));
    }
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some("application/json") {
        return Err(invalid(request, "content type must be application/json"));
    }
    Ok(())
}

fn require_no_query_or_body(request: &HttpRequest) -> Result<(), HttpApiError> {
    if !request.query().is_empty() || !request.body().is_empty() {
        return Err(invalid(request, "request payload is not supported"));
    }
    Ok(())
}

fn not_found(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        404,
        mako_api::ErrorCode::NotFound,
        "explorer resource was not found",
        request.request_id(),
        mako_api::RetryAdvice::Never,
    )
}

fn permission_denied(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        403,
        mako_api::ErrorCode::PermissionDenied,
        message,
        request.request_id(),
        mako_api::RetryAdvice::Never,
    )
}

fn grant_id(nonce: &str) -> String {
    nonce.replacen("xnonce_", "xgr_", 1)
}

fn mutation_operation(kind: ExplorerMutationKind) -> MutationAuthorizationOperation {
    match kind {
        ExplorerMutationKind::Create => MutationAuthorizationOperation::Create,
        ExplorerMutationKind::Update => MutationAuthorizationOperation::Update,
        ExplorerMutationKind::Delete => MutationAuthorizationOperation::Delete,
    }
}

fn document_operation(kind: ExplorerMutationKind) -> DocumentOperation {
    match kind {
        ExplorerMutationKind::Create => DocumentOperation::Create,
        ExplorerMutationKind::Update => DocumentOperation::Update,
        ExplorerMutationKind::Delete => DocumentOperation::Delete,
    }
}

const fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Delete => "DELETE",
        HttpMethod::Get => "GET",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    #[test]
    fn explorer_audit_attributes_fingerprint_sensitive_identifiers() {
        let nonce = "xnonce_super_sensitive_never_record";
        let document_id = "customer-document-never-record";
        let claims = ExplorerCapabilityClaims {
            issuer: mako_api::EXPLORER_CAPABILITY_ISSUER.to_owned(),
            audience: mako_api::EXPLORER_CAPABILITY_AUDIENCE.to_owned(),
            developer_identity_id: "dev_abcdefgh".to_owned(),
            tenant: TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            collection_id: CollectionId::parse("todos").expect("collection"),
            mode: ExplorerAccessMode::Administrative,
            operations: BTreeSet::from([ExplorerOperation::Get]),
            reason_hash: Some("reason-hash-safe".to_owned()),
            application_user_id: None,
            nonce: nonce.to_owned(),
            authorization_epoch: 2,
            issued_at_unix_seconds: 100,
            expires_at_unix_seconds: 200,
            key_id: "key-1".to_owned(),
        };
        let attributes = explorer_audit_claim_attributes(&claims);
        let rendered = format!("{attributes:?}:{}", fingerprint_text(document_id));
        assert!(!rendered.contains(nonce));
        assert!(!rendered.contains(document_id));
        assert!(rendered.contains("reason-hash-safe"));
        assert_eq!(fingerprint_text(nonce).len(), 64);
    }

    #[test]
    fn constant_time_comparison_rejects_changed_or_truncated_signatures() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"samf"));
        assert!(!constant_time_eq(b"same", b"same-longer"));
    }
}
