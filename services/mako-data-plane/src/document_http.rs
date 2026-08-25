use std::{collections::BTreeMap, num::NonZeroU64, num::NonZeroUsize, sync::Arc};

use futures::executor::block_on;
use mako_api::{CollectionId, CollectionScope, ErrorCode, RetryAdvice, TenantScope};
use mako_audit::{ActorIdentity, AttributeValue, AuditCategory, AuditOutcome, SafeAttributes};
use mako_documents::{
    CanonicalDocument, CollectionLifecycle, CommitPosition, DocumentId, DocumentValidator,
    IndexDirection, IndexValue, IndexedQuery, MutationCommitOutcome, MutationError, MutationId,
    MutationInput, QueryBound, QueryCursor, QueryPredicate, QuerySort, RevisionToken,
    SchemaCompatibility, TrustedQuery,
};
use mako_gateway::{
    GatewayQuotaCharge, GatewayQuotaDecision, GatewayQuotaPolicySource, GatewayQuotaResource,
    PresentedServiceCredential, ServiceBypassGateway, ServiceBypassGatewayError,
    ServiceBypassGatewayRequest, VerifiedAccessIdentity,
};
use mako_policy::{
    AuditRequestId, CompiledPolicySet, DocumentOperation, DocumentPolicyAuthorizer,
    DocumentPolicyReadAuthorizer, PolicyCompiler, PrivilegedAuditWriteError,
    PrivilegedBypassAuditEvent, PrivilegedBypassAuditSink, PrivilegedBypassReason,
    SafeRequestMetadata, SubjectId, VerifiedIdentity, VerifiedRole,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use mako_storage::Durability;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    DataPlaneGraph,
    auth_http::{
        append_audit, append_audit_with_details, invalid, json, now_unix_seconds, parse_json,
        tenant, unauthenticated, unavailable, verify_bearer,
    },
};

const SERVICE_KEY_HEADER: &str = "x-mako-service-key";
const BYPASS_REASON_HEADER: &str = "x-mako-bypass-reason";
const REQUEST_ID_HEADER: &str = "x-mako-request-id";
const IDEMPOTENCY_HEADER: &str = "idempotency-key";

pub fn add_document_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/documents/{documentId}",
        Arc::clone(&graph),
        |graph, request| handle_get(graph, request, false),
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/documents/{documentId}",
        Arc::clone(&graph),
        |graph, request| handle_mutation(graph, request, false),
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/documents/query",
        Arc::clone(&graph),
        |graph, request| handle_query(graph, request, false),
    )?;
    add_route(
        router,
        HttpMethod::Get,
        "/v1/projects/{projectId}/environments/{environmentId}/service/collections/{collectionId}/documents/{documentId}",
        Arc::clone(&graph),
        |graph, request| handle_get(graph, request, true),
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/service/collections/{collectionId}/documents/{documentId}",
        Arc::clone(&graph),
        |graph, request| handle_mutation(graph, request, true),
    )?;
    add_route(
        router,
        HttpMethod::Post,
        "/v1/projects/{projectId}/environments/{environmentId}/service/collections/{collectionId}/documents/query",
        graph,
        |graph, request| handle_query(graph, request, true),
    )?;
    Ok(())
}

fn add_route<F>(
    router: &mut HttpRouter,
    method: HttpMethod,
    path: &str,
    graph: Arc<DataPlaneGraph>,
    handler: F,
) -> Result<(), RouteRegistrationError>
where
    F: Fn(&Arc<DataPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>
        + Send
        + Sync
        + 'static,
{
    router.add_route(method, path, move |request| handler(&graph, &request))
}

fn handle_get(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let document_id = document_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        let document = if service {
            let authorizer = service_authorizer(
                graph,
                request,
                &tenant,
                &scope,
                DocumentOperation::Read,
                now,
            )
            .await?;
            charge_document(graph, &tenant, request, now).await?;
            scoped
                .get_document_authorized(&document_id, &authorizer)
                .await
                .map_err(|_| unavailable(request, "document storage is unavailable"))?
        } else {
            let access = verify_bearer(graph, &tenant, request, now).await?;
            charge_document(graph, &tenant, request, now).await?;
            let identity = policy_identity(request, &access)?;
            let request_metadata = policy_request_metadata(request)?;
            let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
            let authorizer =
                DocumentPolicyReadAuthorizer::new(policy.as_ref(), &identity, &request_metadata);
            let document = scoped
                .get_document_authorized(&document_id, &authorizer)
                .await
                .map_err(|_| unavailable(request, "document storage is unavailable"))?;
            append_document_audit(
                graph,
                &tenant,
                application_actor(&access),
                &scope,
                "document_read",
                document
                    .as_ref()
                    .map_or("not_found_or_denied", |_| "allowed"),
                request,
                now,
            )
            .await?;
            document
        };
        let document = document
            .filter(|document| !document.is_deleted())
            .ok_or_else(|| not_found(request, "document was not found"))?;
        json(request, 200, &DocumentRecordWire::from(&document))
    })
}

fn handle_mutation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let document_id = document_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let body: DocumentMutationWire = parse_json(request)?;
        let service_authorizer = if service {
            Some(
                service_authorizer(
                    graph,
                    request,
                    &tenant,
                    &scope,
                    body.operation.policy_operation(),
                    now,
                )
                .await?,
            )
        } else {
            None
        };
        let access = if service {
            None
        } else {
            Some(verify_bearer(graph, &tenant, request, now).await?)
        };
        charge_document(graph, &tenant, request, now).await?;
        // A write is the only thing that changes stored size, so it is what
        // makes the tenant due for another measurement.
        graph.storage_sampler().mark(&tenant);
        validate_mutation_wire(request, &body)?;
        let idempotency = request
            .header(IDEMPOTENCY_HEADER)
            .ok_or_else(|| invalid(request, "idempotency key is required"))?;
        if idempotency != body.mutation_id {
            return Err(conflict(request, "idempotency key must equal mutationId"));
        }
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        let metadata = active_collection_metadata(request, &scoped).await?;
        if metadata.schema_version().get() != body.schema_version {
            return Err(schema_mismatch(
                request,
                "document schema version is not active",
            ));
        }
        let validator = DocumentValidator::compile(&metadata)
            .map_err(|_| unavailable(request, "collection schema is unavailable"))?;
        let current = scoped
            .get_document(&document_id)
            .await
            .map_err(|_| unavailable(request, "document storage is unavailable"))?;
        let expected = body.expected_revision.0.as_deref();
        match body.operation {
            MutationOperationWire::Create if expected.is_some() => {
                return Err(invalid(request, "create requires a null expectedRevision"));
            }
            MutationOperationWire::Update | MutationOperationWire::Delete if expected.is_none() => {
                return Err(invalid(
                    request,
                    "update and delete require expectedRevision",
                ));
            }
            _ => {}
        }
        let validated = match body.operation {
            MutationOperationWire::Create => validator.validate_create(body.body.clone()),
            MutationOperationWire::Update | MutationOperationWire::Delete => {
                let current = current
                    .as_ref()
                    .ok_or_else(|| conflict(request, "document revision does not match"))?;
                validator.validate_update(current, body.body.clone())
            }
        }
        .map_err(|_| schema_mismatch(request, "document does not match the active schema"))?;
        if validated.primary_key() != &document_id {
            return Err(invalid(
                request,
                "document body primary key does not match the path",
            ));
        }
        let mutation_id = MutationId::parse(body.mutation_id.clone())
            .map_err(|_| invalid(request, "mutationId is invalid"))?;
        let sequencer = graph
            .document_engine()
            .scope_sequencer(&tenant, &tenant, Durability::Sync)
            .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("one is non-zero"))
            .await
            .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
        let position = lease.issue().expect("a one-position lease issues once");
        let input = MutationInput {
            mutation_id,
            commit_position: CommitPosition::new(position)
                .map_err(|_| unavailable(request, "document sequencer is unavailable"))?,
            document: validated,
            durability: Durability::Sync,
        };
        let outcome = if let Some(authorizer) = service_authorizer.as_ref() {
            commit_mutation(&scoped, &body, input, authorizer).await
        } else {
            let access = access.as_ref().expect("ordinary route verified access");
            let identity = policy_identity(request, access)?;
            let request_metadata = policy_request_metadata(request)?;
            let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
            let authorizer =
                DocumentPolicyAuthorizer::new(policy.as_ref(), &identity, &request_metadata);
            let outcome = commit_mutation(&scoped, &body, input, &authorizer).await;
            if outcome.is_ok() {
                append_document_audit(
                    graph,
                    &tenant,
                    application_actor(access),
                    &scope,
                    body.operation.audit_action(),
                    "policy_authorized",
                    request,
                    now,
                )
                .await?;
            }
            outcome
        };
        let outcome = match outcome {
            Ok(outcome) => {
                finalize_position(&sequencer, position, &outcome)
                    .await
                    .map_err(|_| unavailable(request, "document sequencer is unavailable"))?;
                outcome
            }
            Err(error) => {
                finalize_failed_position(&sequencer, position).await;
                return Err(map_mutation_error(request, &error));
            }
        };
        let response = mutation_response(request, &scoped, outcome).await?;
        json(request, 200, &response)
    })
}

fn handle_query(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    service: bool,
) -> Result<HttpResponse, HttpApiError> {
    require_json(request)?;
    let tenant = tenant(request)?;
    let scope = collection_scope(request, &tenant)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let service_authorizer = if service {
            Some(
                service_authorizer(
                    graph,
                    request,
                    &tenant,
                    &scope,
                    DocumentOperation::Read,
                    now,
                )
                .await?,
            )
        } else {
            None
        };
        let access = if service {
            None
        } else {
            Some(verify_bearer(graph, &tenant, request, now).await?)
        };
        charge_document(graph, &tenant, request, now).await?;
        let body: DocumentQueryWire = parse_json(request)?;
        let query = trusted_query(request, body)?;
        let scoped = graph
            .document_engine()
            .scope_collection(&tenant, scope.clone())
            .map_err(|_| invalid(request, "collection scope is invalid"))?;
        active_collection_metadata(request, &scoped).await?;
        let page = if let Some(authorizer) = service_authorizer.as_ref() {
            scoped
                .trusted_query_authorized(&query, authorizer)
                .await
                .map_err(|error| map_query_error(request, &error))?
        } else {
            let access = access.as_ref().expect("ordinary route verified access");
            let identity = policy_identity(request, access)?;
            let request_metadata = policy_request_metadata(request)?;
            let policy = active_policy(graph, request, &tenant, &scope, &scoped).await?;
            let authorizer =
                DocumentPolicyReadAuthorizer::new(policy.as_ref(), &identity, &request_metadata);
            let page = scoped
                .trusted_query_authorized(&query, &authorizer)
                .await
                .map_err(|error| map_query_error(request, &error))?;
            append_document_audit(
                graph,
                &tenant,
                application_actor(access),
                &scope,
                "document_query",
                "policy_filtered",
                request,
                now,
            )
            .await?;
            page
        };
        let response = DocumentQueryPageWire {
            documents: page
                .documents()
                .iter()
                .map(DocumentRecordWire::from)
                .collect(),
            next_cursor: page.next_cursor().map(|cursor| cursor.as_str().to_owned()),
        };
        json(request, 200, &response)
    })
}

pub(crate) async fn active_collection_metadata(
    request: &HttpRequest,
    scoped: &mako_documents::ScopedCollectionEngine,
) -> Result<mako_documents::CollectionMetadata, HttpApiError> {
    let metadata = scoped
        .collection_metadata()
        .await
        .map_err(|_| unavailable(request, "collection metadata is unavailable"))?
        .ok_or_else(|| not_found(request, "collection was not found"))?;
    if metadata.lifecycle() != CollectionLifecycle::Active
        || metadata.compatibility() != SchemaCompatibility::Compatible
    {
        return Err(schema_mismatch(request, "collection schema is not active"));
    }
    Ok(metadata)
}

pub(crate) async fn active_policy(
    graph: &DataPlaneGraph,
    request: &HttpRequest,
    tenant: &TenantScope,
    scope: &CollectionScope,
    scoped: &mako_documents::ScopedCollectionEngine,
) -> Result<Option<CompiledPolicySet>, HttpApiError> {
    let metadata = active_collection_metadata(request, scoped).await?;
    let policy = graph
        .policy_store(tenant, scope.clone())
        .map_err(|_| unavailable(request, "document policy is unavailable"))?
        .active_policy()
        .await
        .map_err(|_| unavailable(request, "document policy is unavailable"))?;
    policy
        .map(|policy| {
            PolicyCompiler::default()
                .compile(&policy, &Value::Object(metadata.json_schema().clone()))
                .map_err(|_| unavailable(request, "document policy is unavailable"))?
                .into_compiled()
                .ok_or_else(|| unavailable(request, "document policy is unavailable"))
        })
        .transpose()
}

async fn service_authorizer(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    scope: &CollectionScope,
    operation: DocumentOperation,
    now: u64,
) -> Result<mako_policy::PrivilegedBypassAuthorizer, HttpApiError> {
    require_presented_request_id(request)?;
    let credential = request
        .header(SERVICE_KEY_HEADER)
        .ok_or_else(|| unauthenticated(request, "service project key is required"))?;
    let reason = request
        .header(BYPASS_REASON_HEADER)
        .ok_or_else(|| invalid(request, "service bypass reason is required"))?;
    let request_id = AuditRequestId::parse(request.request_id())
        .map_err(|_| invalid(request, "service request id is invalid"))?;
    let audit = DataPlaneBypassAudit {
        graph: Arc::clone(graph),
    };
    ServiceBypassGateway
        .authorize(
            ServiceBypassGatewayRequest {
                tenant: tenant.clone(),
                collection_id: scope.collection_id().clone(),
                operation,
                credential: PresentedServiceCredential::parse(credential.to_owned())
                    .map_err(|_| unauthenticated(request, "service project key is invalid"))?,
                request_id,
                reason: PrivilegedBypassReason::parse(reason.to_owned())
                    .map_err(|_| invalid(request, "service bypass reason is invalid"))?,
                now_unix_seconds: now,
            },
            &graph
                .identity_store(tenant, tenant)
                .map_err(|_| unavailable(request, "project credentials are unavailable"))?,
            &audit,
        )
        .await
        .map_err(|error| match error {
            ServiceBypassGatewayError::Unauthenticated
            | ServiceBypassGatewayError::InvalidRequest => {
                unauthenticated(request, "service project key is invalid")
            }
            ServiceBypassGatewayError::Forbidden => {
                permission_denied(request, "service credential does not permit this operation")
            }
            _ => unavailable(request, "service authorization is unavailable"),
        })
}

struct DataPlaneBypassAudit {
    graph: Arc<DataPlaneGraph>,
}

impl PrivilegedBypassAuditSink for DataPlaneBypassAudit {
    fn record(&self, event: &PrivilegedBypassAuditEvent) -> Result<(), PrivilegedAuditWriteError> {
        let now = now_unix_seconds(event.request_id().as_str())
            .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit is unavailable"))?;
        let details = SafeAttributes::try_from_iter([(
            "bypass_reason".to_owned(),
            AttributeValue::Text(event.reason().as_str().to_owned()),
        )])
        .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit is invalid"))?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(append_audit_with_details(
                        &self.graph,
                        event.scope().tenant(),
                        AuditCategory::ServiceBypass,
                        ActorIdentity::Service {
                            actor_id: event.actor_id().to_owned(),
                        },
                        "collection",
                        event.scope().collection_id().as_str(),
                        operation_audit_action(event.operation()),
                        AuditOutcome::Allowed,
                        "service_bypass_verified",
                        event.request_id().as_str(),
                        now,
                        details,
                    ))
                })
                .join()
        })
        .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit worker failed"))?
        .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit is unavailable"))
    }
}

async fn commit_mutation(
    scoped: &mako_documents::ScopedCollectionEngine,
    body: &DocumentMutationWire,
    input: MutationInput,
    authorizer: &dyn mako_documents::DocumentMutationAuthorizer,
) -> Result<MutationCommitOutcome, MutationError> {
    match body.operation {
        MutationOperationWire::Create => scoped.create_document_authorized(input, authorizer).await,
        MutationOperationWire::Update => {
            let revision = RevisionToken::parse(
                body.expected_revision
                    .0
                    .as_deref()
                    .expect("validated update revision"),
            )?;
            scoped
                .update_document_authorized(revision, input, authorizer)
                .await
        }
        MutationOperationWire::Delete => {
            let revision = RevisionToken::parse(
                body.expected_revision
                    .0
                    .as_deref()
                    .expect("validated delete revision"),
            )?;
            scoped
                .delete_document_authorized(revision, input, authorizer)
                .await
        }
    }
}

async fn finalize_position(
    sequencer: &mako_documents::EnvironmentSequencer,
    position: u64,
    outcome: &MutationCommitOutcome,
) -> Result<(), mako_documents::SequencerError> {
    match outcome {
        MutationCommitOutcome::Applied(_) => {
            sequencer.mark_committed(position).await?;
        }
        MutationCommitOutcome::Replayed(_) | MutationCommitOutcome::RevisionConflict { .. } => {
            sequencer.mark_aborted(position).await?;
        }
    }
    sequencer.recover_high_water().await?;
    Ok(())
}

async fn finalize_failed_position(sequencer: &mako_documents::EnvironmentSequencer, position: u64) {
    if matches!(sequencer.disposition(position).await, Ok(None)) {
        let _ = sequencer.mark_aborted(position).await;
    }
    let _ = sequencer.recover_high_water().await;
}

async fn mutation_response(
    request: &HttpRequest,
    scoped: &mako_documents::ScopedCollectionEngine,
    outcome: MutationCommitOutcome,
) -> Result<DocumentMutationResultWire, HttpApiError> {
    match outcome {
        MutationCommitOutcome::Applied(receipt) => {
            let document = scoped
                .get_document(&receipt.document_id)
                .await
                .map_err(|_| unavailable(request, "document storage is unavailable"))?
                .ok_or_else(|| unavailable(request, "document receipt is unavailable"))?;
            Ok(DocumentMutationResultWire {
                mutation_id: receipt.mutation_id.as_str().to_owned(),
                status: "applied",
                document: Some(DocumentRecordWire::from(&document)),
                current_revision: None,
            })
        }
        MutationCommitOutcome::Replayed(receipt) => {
            let document = scoped
                .get_document(&receipt.document_id)
                .await
                .map_err(|_| unavailable(request, "document storage is unavailable"))?
                .ok_or_else(|| unavailable(request, "document receipt is unavailable"))?;
            Ok(DocumentMutationResultWire {
                mutation_id: receipt.mutation_id.as_str().to_owned(),
                status: "replayed",
                document: Some(DocumentRecordWire::from(&document)),
                current_revision: None,
            })
        }
        MutationCommitOutcome::RevisionConflict { current_revision } => {
            Ok(DocumentMutationResultWire {
                mutation_id: request
                    .header(IDEMPOTENCY_HEADER)
                    .unwrap_or_default()
                    .to_owned(),
                status: "conflict",
                document: None,
                current_revision: current_revision.map(|revision| revision.as_str().to_owned()),
            })
        }
    }
}

fn trusted_query(
    request: &HttpRequest,
    body: DocumentQueryWire,
) -> Result<TrustedQuery, HttpApiError> {
    if body.predicates.is_empty()
        || body.predicates.len() > 16
        || body.sort.len() > 16
        || !(1..=1_000).contains(&body.limit)
    {
        return Err(invalid(request, "document query limits are invalid"));
    }
    let mut fields: BTreeMap<String, PredicateParts> = BTreeMap::new();
    for predicate in body.predicates {
        if predicate.field.is_empty() || predicate.field.len() > 512 {
            return Err(invalid(request, "document query predicate is invalid"));
        }
        let value = IndexValue::from_json(&predicate.value)
            .map_err(|_| invalid(request, "document query value is invalid"))?;
        let parts = fields.entry(predicate.field).or_default();
        match predicate.operator {
            QueryOperatorWire::Eq
                if parts.equal.is_none() && parts.lower.is_none() && parts.upper.is_none() =>
            {
                parts.equal = Some(value);
            }
            QueryOperatorWire::Gt if parts.equal.is_none() && parts.lower.is_none() => {
                parts.lower = Some(QueryBound::exclusive(value));
            }
            QueryOperatorWire::Gte if parts.equal.is_none() && parts.lower.is_none() => {
                parts.lower = Some(QueryBound::inclusive(value));
            }
            QueryOperatorWire::Lt if parts.equal.is_none() && parts.upper.is_none() => {
                parts.upper = Some(QueryBound::exclusive(value));
            }
            QueryOperatorWire::Lte if parts.equal.is_none() && parts.upper.is_none() => {
                parts.upper = Some(QueryBound::inclusive(value));
            }
            _ => return Err(invalid(request, "document query predicates conflict")),
        }
    }
    let mut predicates = Vec::with_capacity(fields.len());
    for (field, parts) in fields {
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
            _ => return Err(invalid(request, "document query predicate is invalid")),
        }
        .map_err(|_| invalid(request, "document query predicate is invalid"))?;
        predicates.push(predicate);
    }
    let sort = body
        .sort
        .into_iter()
        .map(|sort| match sort.direction {
            QueryDirectionWire::Asc => QuerySort::new(sort.field, IndexDirection::Ascending),
            QueryDirectionWire::Desc => QuerySort::new(sort.field, IndexDirection::Descending),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid(request, "document query sort is invalid"))?;
    let mut indexed = IndexedQuery::new(
        predicates,
        sort,
        NonZeroUsize::new(body.limit).expect("validated positive query limit"),
    )
    .map_err(|_| invalid(request, "document query is invalid"))?;
    if let Some(cursor) = body.cursor.0 {
        indexed = indexed.with_cursor(
            QueryCursor::parse(cursor)
                .map_err(|_| invalid(request, "document query cursor is invalid"))?,
        );
    }
    Ok(TrustedQuery::indexed(indexed))
}

async fn charge_document(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    request: &HttpRequest,
    now: u64,
) -> Result<(), HttpApiError> {
    let mut charges = vec![GatewayQuotaCharge {
        resource: GatewayQuotaResource::DocumentRequests,
        amount: NonZeroU64::new(1).expect("one is non-zero"),
    }];
    if let Some(bytes) = NonZeroU64::new(
        u64::try_from(request.body().len())
            .map_err(|_| invalid(request, "request body is too large"))?,
    ) {
        charges.push(GatewayQuotaCharge {
            resource: GatewayQuotaResource::DocumentBytes,
            amount: bytes,
        });
    }
    let route_digest = &blake3::hash(request.path().as_bytes()).to_hex()[..16];
    let reservation = format!("{}.document.{route_digest}", request.request_id());
    match graph
        .quota_engine()
        .check_and_reserve(
            tenant,
            &reservation,
            &charges,
            // Resolved per tenant, so an operator override or a plan actually
            // changes what this tenant is held to.
            &graph
                .quota_policies()
                .policy_for(tenant)
                .await
                .map_err(|_| unavailable(request, "quota authority is unavailable"))?,
            now.saturating_mul(1_000),
        )
        .await
        .map_err(|_| unavailable(request, "quota authority is unavailable"))?
    {
        GatewayQuotaDecision::Allowed => Ok(()),
        GatewayQuotaDecision::Throttled {
            retry_after_milliseconds,
            ..
        } => Err(HttpApiError::new(
            429,
            ErrorCode::RateLimited,
            "document request rate exceeded",
            request.request_id(),
            RetryAdvice::AfterDelay {
                after_ms: retry_after_milliseconds,
            },
        )),
        GatewayQuotaDecision::HardLimit { .. } => Err(HttpApiError::new(
            429,
            ErrorCode::QuotaExceeded,
            "document request quota exceeded",
            request.request_id(),
            RetryAdvice::Never,
        )),
    }
}

fn policy_identity(
    request: &HttpRequest,
    access: &VerifiedAccessIdentity,
) -> Result<VerifiedIdentity, HttpApiError> {
    VerifiedIdentity::user(
        SubjectId::parse(access.user_id().as_str())
            .map_err(|_| unauthenticated(request, "application access token is invalid"))?,
        VerifiedRole::parse(access.role())
            .map_err(|_| unauthenticated(request, "application access token is invalid"))?,
        Value::Object(access.trusted_claims().clone()),
    )
    .map_err(|_| unauthenticated(request, "application access token is invalid"))
}

fn policy_request_metadata(request: &HttpRequest) -> Result<SafeRequestMetadata, HttpApiError> {
    SafeRequestMetadata::new([
        (
            "method".to_owned(),
            http_method_name(request.method()).to_owned(),
        ),
        ("request_id".to_owned(), request.request_id().to_owned()),
    ])
    .map_err(|_| invalid(request, "request metadata is invalid"))
}

const fn http_method_name(method: HttpMethod) -> &'static str {
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

fn collection_scope(
    request: &HttpRequest,
    tenant: &TenantScope,
) -> Result<CollectionScope, HttpApiError> {
    let collection = request
        .path_parameter("collectionId")
        .ok_or_else(|| invalid(request, "collection path is invalid"))?;
    Ok(CollectionScope::new(
        tenant.clone(),
        CollectionId::parse(collection)
            .map_err(|_| invalid(request, "collection path is invalid"))?,
    ))
}

fn document_id(request: &HttpRequest) -> Result<DocumentId, HttpApiError> {
    DocumentId::parse(
        request
            .path_parameter("documentId")
            .ok_or_else(|| invalid(request, "document path is invalid"))?,
    )
    .map_err(|_| invalid(request, "document path is invalid"))
}

fn require_presented_request_id(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.header(REQUEST_ID_HEADER) != Some(request.request_id()) {
        return Err(invalid(
            request,
            "service request id must be explicitly presented",
        ));
    }
    Ok(())
}

fn validate_mutation_wire(
    request: &HttpRequest,
    body: &DocumentMutationWire,
) -> Result<(), HttpApiError> {
    if !(16..=200).contains(&body.mutation_id.len())
        || body.mutation_id.chars().any(char::is_control)
        || body.schema_version == 0
    {
        return Err(invalid(request, "document mutation is invalid"));
    }
    if body.expected_revision.0.as_ref().is_some_and(|revision| {
        revision.is_empty() || revision.len() > 1_024 || revision.chars().any(char::is_control)
    }) {
        return Err(invalid(request, "expectedRevision is invalid"));
    }
    Ok(())
}

fn require_json(request: &HttpRequest) -> Result<(), HttpApiError> {
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some("application/json") {
        return Err(invalid(request, "content type must be application/json"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn append_document_audit(
    graph: &DataPlaneGraph,
    tenant: &TenantScope,
    actor: ActorIdentity,
    scope: &CollectionScope,
    action: &str,
    reason: &str,
    request: &HttpRequest,
    now: u64,
) -> Result<(), HttpApiError> {
    append_audit(
        graph,
        tenant,
        AuditCategory::Policy,
        actor,
        "collection",
        scope.collection_id().as_str(),
        action,
        AuditOutcome::Allowed,
        reason,
        request.request_id(),
        now,
    )
    .await
}

fn application_actor(identity: &VerifiedAccessIdentity) -> ActorIdentity {
    ActorIdentity::ApplicationUser {
        actor_id: identity.user_id().as_str().to_owned(),
        session_id: identity.session_id().as_str().to_owned(),
    }
}

const fn operation_audit_action(operation: DocumentOperation) -> &'static str {
    match operation {
        DocumentOperation::Create => "service_document_create",
        DocumentOperation::Read => "service_document_read",
        DocumentOperation::Update => "service_document_update",
        DocumentOperation::Delete => "service_document_delete",
    }
}

fn map_mutation_error(request: &HttpRequest, error: &MutationError) -> HttpApiError {
    match error {
        MutationError::AuthorizationDenied { .. } => {
            permission_denied(request, "document mutation is denied by policy")
        }
        MutationError::IdempotencyMismatch
        | MutationError::UniqueConstraintViolation { .. }
        | MutationError::AtomicConflict { .. }
        | MutationError::IndexCatalogChanged => {
            conflict(request, "document mutation conflicts with current state")
        }
        MutationError::Document(_) => schema_mismatch(request, "document schema is invalid"),
        _ => unavailable(request, "document mutation is unavailable"),
    }
}

fn map_query_error(
    request: &HttpRequest,
    error: &mako_documents::TrustedQueryError,
) -> HttpApiError {
    match error {
        mako_documents::TrustedQueryError::RequiredIndex { .. } => {
            conflict(request, "document query requires an active index")
        }
        mako_documents::TrustedQueryError::Storage(_)
        | mako_documents::TrustedQueryError::Document(_)
        | mako_documents::TrustedQueryError::CorruptIndexEntry => {
            unavailable(request, "document query is unavailable")
        }
        _ => invalid(request, "document query is invalid"),
    }
}

fn not_found(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        404,
        ErrorCode::NotFound,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn conflict(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::Conflict,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn schema_mismatch(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::SchemaMismatch,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn permission_denied(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        403,
        ErrorCode::PermissionDenied,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentMutationWire {
    mutation_id: String,
    operation: MutationOperationWire,
    expected_revision: RequiredNullableString,
    schema_version: u64,
    body: Value,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MutationOperationWire {
    Create,
    Update,
    Delete,
}

impl MutationOperationWire {
    const fn policy_operation(self) -> DocumentOperation {
        match self {
            Self::Create => DocumentOperation::Create,
            Self::Update => DocumentOperation::Update,
            Self::Delete => DocumentOperation::Delete,
        }
    }

    const fn audit_action(self) -> &'static str {
        match self {
            Self::Create => "document_create",
            Self::Update => "document_update",
            Self::Delete => "document_delete",
        }
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct RequiredNullableString(Option<String>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentQueryWire {
    predicates: Vec<DocumentQueryPredicateWire>,
    sort: Vec<DocumentQuerySortWire>,
    cursor: RequiredNullableString,
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentQueryPredicateWire {
    field: String,
    operator: QueryOperatorWire,
    value: Value,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum QueryOperatorWire {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentQuerySortWire {
    field: String,
    direction: QueryDirectionWire,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum QueryDirectionWire {
    Asc,
    Desc,
}

#[derive(Default)]
struct PredicateParts {
    equal: Option<IndexValue>,
    lower: Option<QueryBound>,
    upper: Option<QueryBound>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentRecordWire {
    primary_key: String,
    schema_version: u64,
    revision: String,
    commit_position: u64,
    #[serde(rename = "_deleted")]
    deleted: bool,
    body: Map<String, Value>,
}

impl From<&CanonicalDocument> for DocumentRecordWire {
    fn from(document: &CanonicalDocument) -> Self {
        Self {
            primary_key: document.primary_key().as_str().to_owned(),
            schema_version: document.schema_version().get(),
            revision: document.revision().as_str().to_owned(),
            commit_position: document.commit_position().get(),
            deleted: document.is_deleted(),
            body: document.body().clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentMutationResultWire {
    mutation_id: String,
    status: &'static str,
    document: Option<DocumentRecordWire>,
    current_revision: Option<String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DocumentQueryPageWire {
    documents: Vec<DocumentRecordWire>,
    next_cursor: Option<String>,
}
