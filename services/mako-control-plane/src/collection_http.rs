use std::sync::Arc;

use mako_api::{
    ApiError, ApiErrorEnvelope, CollectionId, ErrorCode, RetryAdvice, SafeDetail, TenantScope,
};
use mako_control_plane::{
    CollectionAdminError, CollectionAdminService, CollectionCompatibilityReport,
    DeveloperPrincipal, IndexBuildStatus, NewCollection, NewIndex, NewSchemaMigration,
    PublishSchema, SchemaMigrationId, SchemaMigrationRecord, SchemaMigrationState,
    SchemaPublicationOutcome,
};
use mako_documents::{
    CollectionMetadata, IndexDirection, IndexError, IndexField, IndexKind, IndexName, IndexState,
    IndexVersion, PrimaryKeyDefinition, StoredDocumentCheck,
};
use mako_internal_rpc::IdentityAdminOperation;
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ControlPlaneGraph, explorer_invalidation,
    identity_admin_http::administer,
    management_http::{
        conflict, forbidden, format_timestamp, invalid, json, limit, no_payload, no_query,
        not_found, parse_json, require_idempotency, require_json, stable_id, unavailable,
        with_developer,
    },
};

pub(crate) fn add_collection_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections",
            handle_list_collections as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections",
            handle_create_collection,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}",
            handle_get_collection,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/schemas",
            handle_publish_schema,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations",
            handle_create_migration,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations/{migrationId}",
            handle_get_migration,
        ),
        (
            HttpMethod::Patch,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/migrations/{migrationId}",
            handle_update_migration,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes",
            handle_list_indexes,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes",
            handle_create_index,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes/{indexName}/{indexVersion}",
            handle_get_index,
        ),
        (
            HttpMethod::Delete,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/indexes/{indexName}/{indexVersion}",
            handle_delete_index,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_list_collections(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .collection_service()
            .list_collections(&actor, &tenant, limit(), now)
            .await
            .map_err(|error| collection_error(request, error))?;
        let items = records.iter().map(collection_wire).collect();
        json(request, 200, &ItemsWire { items })
    })
}

/// A 409 whose message names what is taken, for refusals that must say which
/// version or name to use instead.
fn taken(request: &HttpRequest, message: String) -> HttpApiError {
    HttpApiError::new(
        409,
        ErrorCode::Conflict,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn handle_create_collection(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let body: CreateCollectionWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let input = NewCollection {
            tenant: tenant.clone(),
            collection_id: body.id.clone(),
            schema_version: body.schema_version,
            json_schema: body.json_schema.clone(),
            primary_key: body.primary_key.clone(),
            now_unix_seconds: now,
        };
        let record = match graph
            .collection_service()
            .create_collection(&actor, input)
            .await
        {
            Ok(record) => record,
            Err(CollectionAdminError::Conflict) => {
                let existing = graph
                    .collection_service()
                    .get_collection(&actor, &tenant, &body.id, now)
                    .await
                    .map_err(|error| collection_error(request, error))?;
                if collection_matches(&existing, &body) {
                    existing
                } else {
                    // A fresh request, not a retry: the ID is taken.
                    return Err(conflict(
                        request,
                        "a collection with this ID already exists with a different definition; choose another ID, or publish a new schema version of it",
                    ));
                }
            }
            Err(error) => return Err(collection_error(request, error)),
        };

        // The control plane owns the collection record, but document traffic is
        // served from the data plane's own store. Install the metadata there
        // before reporting the collection active; a failure here leaves the
        // record in its Creating state with a retryable diagnostic rather than
        // advertising a collection that document operations would reject.
        let servable = CollectionAdminService::servable_metadata(&record)
            .map_err(|error| collection_error(request, error))?;
        let encoded = servable
            .encode()
            .map_err(|_| unavailable(request, "collection metadata could not be encoded"))?;
        let metadata: Value = serde_json::from_slice(&encoded)
            .map_err(|_| unavailable(request, "collection metadata could not be encoded"))?;
        let _: Value = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::InstallCollection,
            json!({"collectionId": record.collection_id().as_str(), "metadata": metadata}),
            true,
        )
        .await?;

        let activated = graph
            .collection_service()
            .activate_collection(&tenant, record.collection_id(), now)
            .await
            .map_err(|error| collection_error(request, error))?;
        json(request, 201, &collection_wire(&activated))
    })
}

fn handle_get_collection(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .collection_service()
            .get_collection(&actor, &tenant, &collection_id, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        json(request, 200, &collection_wire(&record))
    })
}

fn handle_publish_schema(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let body: PublishSchemaWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let input = PublishSchema {
            tenant: tenant.clone(),
            collection_id: collection_id.clone(),
            schema_version: body.schema_version,
            json_schema: body.json_schema.clone(),
            primary_key: body.primary_key.clone(),
            now_unix_seconds: now,
        };
        let outcome = match graph
            .collection_service()
            .publish_schema(&actor, input)
            .await
        {
            Ok(outcome) => outcome,
            Err(CollectionAdminError::SchemaVersionMustIncrease) => {
                let current = graph
                    .collection_service()
                    .get_collection(&actor, &tenant, &collection_id, now)
                    .await
                    .map_err(|error| collection_error(request, error))?;
                if current.schema_version().get() == body.schema_version
                    && Value::Object(current.json_schema().clone()) == body.json_schema
                    && current.primary_key() == &body.primary_key
                {
                    SchemaPublicationOutcome::Published(current)
                } else {
                    return Err(taken(
                        request,
                        format!(
                            "the collection is already at schema version {}; publish version {} or later",
                            current.schema_version().get(),
                            current.schema_version().get().saturating_add(1)
                        ),
                    ));
                }
            }
            Err(error) => return Err(collection_error(request, error)),
        };
        // The data plane serves documents and replication from its own copy of
        // the metadata, and `create` installs that copy -- a publish that only
        // rewrote the control plane's record left every environment refusing
        // the new schema version forever (finding #39). Install the published
        // metadata the same way; a refused install leaves the control-side
        // record ahead, and a retried publish lands in the idempotent arm
        // above and installs again, so the two sides converge.
        if let SchemaPublicationOutcome::Published(record) = &outcome {
            let servable = CollectionAdminService::servable_metadata(record)
                .map_err(|error| collection_error(request, error))?;
            let encoded = servable
                .encode()
                .map_err(|_| unavailable(request, "collection metadata could not be encoded"))?;
            let metadata: Value = serde_json::from_slice(&encoded)
                .map_err(|_| unavailable(request, "collection metadata could not be encoded"))?;
            let _: Value = administer(
                graph,
                request,
                &actor,
                &tenant,
                IdentityAdminOperation::InstallCollection,
                json!({"collectionId": record.collection_id().as_str(), "metadata": metadata}),
                true,
            )
            .await?;
        }
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "schema-publish",
        )?;
        json(request, 200, &publication_wire(&outcome))
    })
}

fn handle_create_migration(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let idempotency = require_idempotency(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let migration_id = SchemaMigrationId::parse(stable_id(
        "mig",
        &[
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
            collection_id.as_str(),
            idempotency,
        ],
    ))
    .map_err(|_| invalid(request, "migration identifier is invalid"))?;
    let body: CreateMigrationWire = parse_json(request)?;
    with_developer(graph, request, |actor, now| async move {
        let input = NewSchemaMigration {
            id: migration_id.clone(),
            tenant: tenant.clone(),
            collection_id: collection_id.clone(),
            target_schema_version: body.target_schema_version,
            target_json_schema: body.target_json_schema.clone(),
            target_primary_key: body.target_primary_key.clone(),
            reason: body.reason.clone(),
            now_unix_seconds: now,
        };
        let record = match graph
            .collection_service()
            .create_migration(&actor, input)
            .await
        {
            Ok(record) => record,
            Err(CollectionAdminError::Conflict) => {
                let existing = graph
                    .collection_service()
                    .get_migration(&actor, &tenant, &collection_id, &migration_id, now)
                    .await
                    .map_err(|error| collection_error(request, error))?;
                if existing.matches_request(
                    body.target_schema_version,
                    &body.target_json_schema,
                    &body.target_primary_key,
                    &body.reason,
                ) {
                    existing
                } else {
                    return Err(conflict(
                        request,
                        "a migration with this ID already exists with a different target or reason; choose another migration ID",
                    ));
                }
            }
            Err(error) => return Err(collection_error(request, error)),
        };
        json(request, 202, &migration_wire(request, &record)?)
    })
}

fn handle_get_migration(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let migration_id = migration_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .collection_service()
            .get_migration(&actor, &tenant, &collection_id, &migration_id, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        json(request, 200, &migration_wire(request, &record)?)
    })
}

fn handle_update_migration(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let migration_id = migration_id(request)?;
    let body: MigrationStateWire = parse_json(request)?;
    if body.state == SchemaMigrationState::Completed {
        return complete_migration(graph, request, &tenant, &collection_id, &migration_id);
    }
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .collection_service()
            .transition_migration(
                &actor,
                &tenant,
                &collection_id,
                &migration_id,
                body.state,
                now,
            )
            .await
            .map_err(|error| collection_error(request, error))?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "schema-migration",
        )?;
        json(request, 200, &migration_wire(request, &record)?)
    })
}

/// Completing a migration activates its target schema, once the data plane,
/// which holds the documents, reports that every one of them satisfies it.
/// Until then the migration stays running, and the answer names the documents
/// still to bring to the new shape.
fn complete_migration(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    migration_id: &SchemaMigrationId,
) -> Result<HttpResponse, HttpApiError> {
    with_developer(graph, request, |actor, now| async move {
        let service = graph.collection_service();
        let target = service
            .migration_target(&actor, tenant, collection_id, migration_id, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        let check = administer(
            graph,
            request,
            &actor,
            tenant,
            IdentityAdminOperation::CheckStoredDocuments,
            json!({"collectionId": collection_id.as_str(), "metadata": metadata_value(request, &target)?}),
            false,
        )
        .await?;
        let check: StoredDocumentCheck = serde_json::from_value(check)
            .map_err(|_| unavailable(request, "stored document check could not be read"))?;
        let (record, active) = service
            .complete_migration(&actor, tenant, collection_id, migration_id, &check, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        // As after a publish: the data plane serves the new version only once
        // it holds the metadata. A refused install leaves the control-side
        // record ahead; completing again installs it, as the target is active.
        let _: Value = administer(
            graph,
            request,
            &actor,
            tenant,
            IdentityAdminOperation::InstallCollection,
            json!({"collectionId": collection_id.as_str(), "metadata": metadata_value(request, &active)?}),
            false,
        )
        .await?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            tenant,
            actor.identity_id().as_str(),
            "schema-migration",
        )?;
        json(request, 200, &migration_wire(request, &record)?)
    })
}

fn metadata_value(
    request: &HttpRequest,
    metadata: &CollectionMetadata,
) -> Result<Value, HttpApiError> {
    let servable = CollectionAdminService::servable_metadata(metadata)
        .map_err(|error| collection_error(request, error))?;
    let encoded = servable
        .encode()
        .map_err(|_| unavailable(request, "collection metadata could not be encoded"))?;
    serde_json::from_slice(&encoded)
        .map_err(|_| unavailable(request, "collection metadata could not be encoded"))
}

fn handle_list_indexes(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    with_developer(graph, request, |actor, now| async move {
        let records = graph
            .collection_service()
            .list_indexes(&actor, &tenant, &collection_id, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        // Reported from the data plane too, so a listing cannot disagree with
        // the single-index read about whether a query can use an index.
        let mut items = Vec::with_capacity(records.len());
        for record in &records {
            let inspected =
                inspect_index(graph, request, &actor, &tenant, &collection_id, record).await;
            items.push(index_wire_with_state(
                record,
                listed_state(record, inspected),
            ));
        }
        json(request, 200, &ItemsWire { items })
    })
}

fn handle_create_index(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let body: CreateIndexWire = parse_json(request)?;
    let name = IndexName::parse(body.name.clone())
        .map_err(|_| invalid(request, "index name is invalid"))?;
    let version = IndexVersion::new(body.version)
        .map_err(|_| invalid(request, "index version is invalid"))?;
    let fields = body
        .fields
        .iter()
        .map(|field| IndexField::new(field.path.clone(), field.direction))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid(request, "index fields are invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let input = NewIndex {
            tenant: tenant.clone(),
            collection_id: collection_id.clone(),
            name: name.clone(),
            version: body.version,
            kind: body.kind,
            fields: fields.clone(),
            now_unix_seconds: now,
        };
        let record = match graph.collection_service().create_index(&actor, input).await {
            Ok(record) => record,
            Err(CollectionAdminError::Index(IndexError::DefinitionAlreadyExists)) => {
                let existing = graph
                    .collection_service()
                    .get_index(&actor, &tenant, &collection_id, &name, version, now)
                    .await
                    .map_err(|error| collection_error(request, error))?;
                if existing.definition.kind() == body.kind && existing.definition.fields() == fields
                {
                    existing
                } else {
                    return Err(taken(
                        request,
                        format!(
                            "index {} version {} already exists with a different kind or fields; choose another name or version",
                            name.as_str(),
                            version.get()
                        ),
                    ));
                }
            }
            Err(error) => return Err(collection_error(request, error)),
        };
        let state = install_index(graph, request, &actor, &tenant, &collection_id, &record).await?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "index-create",
        )?;
        json(request, 202, &index_wire_with_state(&record, state))
    })
}

fn handle_get_index(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let (name, version) = index_identity(request)?;
    with_developer(graph, request, |actor, now| async move {
        let record = graph
            .collection_service()
            .get_index(&actor, &tenant, &collection_id, &name, version, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        let inspected =
            inspect_index(graph, request, &actor, &tenant, &collection_id, &record).await;
        json(
            request,
            200,
            &index_wire_with_state(&record, listed_state(&record, inspected)),
        )
    })
}

fn handle_delete_index(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let tenant = tenant(request)?;
    let collection_id = collection_id(request)?;
    let (name, version) = index_identity(request)?;
    with_developer(graph, request, |actor, now| async move {
        graph
            .collection_service()
            .delete_index(&actor, &tenant, &collection_id, &name, version, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        let record = graph
            .collection_service()
            .get_index(&actor, &tenant, &collection_id, &name, version, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        // The data plane is what answers queries and enforces uniqueness, so
        // the deletion has to reach it: recorded here alone, a deleted unique
        // index went on refusing duplicates and listed as active. Should this
        // fail, the index stays "deleting" and deleting it again finishes.
        let _: Value = administer(
            graph,
            request,
            &actor,
            &tenant,
            IdentityAdminOperation::RemoveIndex,
            json!({
                "collectionId": collection_id.as_str(),
                "name": name.as_str(),
                "version": version.get(),
            }),
            false,
        )
        .await?;
        graph
            .collection_service()
            .forget_index(&actor, &tenant, &collection_id, &name, version, now)
            .await
            .map_err(|error| collection_error(request, error))?;
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "index-delete",
        )?;
        json(request, 202, &index_wire(&record))
    })
}

fn tenant(request: &HttpRequest) -> Result<TenantScope, HttpApiError> {
    TenantScope::require(
        request.path_parameter("projectId"),
        request.path_parameter("environmentId"),
    )
    .map_err(|_| invalid(request, "tenant path is invalid"))
}

fn collection_id(request: &HttpRequest) -> Result<CollectionId, HttpApiError> {
    CollectionId::parse(request.path_parameter("collectionId").unwrap_or_default())
        .map_err(|_| invalid(request, "collection path is invalid"))
}

fn migration_id(request: &HttpRequest) -> Result<SchemaMigrationId, HttpApiError> {
    SchemaMigrationId::parse(request.path_parameter("migrationId").unwrap_or_default())
        .map_err(|_| invalid(request, "migration path is invalid"))
}

fn index_identity(request: &HttpRequest) -> Result<(IndexName, IndexVersion), HttpApiError> {
    let name = IndexName::parse(request.path_parameter("indexName").unwrap_or_default())
        .map_err(|_| invalid(request, "index name path is invalid"))?;
    let version = request
        .path_parameter("indexVersion")
        .and_then(|value| value.parse().ok())
        .and_then(|value| IndexVersion::new(value).ok())
        .ok_or_else(|| invalid(request, "index version path is invalid"))?;
    Ok((name, version))
}

fn collection_matches(record: &CollectionMetadata, body: &CreateCollectionWire) -> bool {
    record.schema_version().get() == body.schema_version
        && Value::Object(record.json_schema().clone()) == body.json_schema
        && record.primary_key() == &body.primary_key
}

fn collection_error(request: &HttpRequest, error: CollectionAdminError) -> HttpApiError {
    match error {
        CollectionAdminError::NotFound
        | CollectionAdminError::Index(IndexError::DefinitionNotFound) => {
            not_found(request, "collection resource was not found")
        }
        CollectionAdminError::Forbidden => forbidden(request, "collection action is forbidden"),
        CollectionAdminError::MigrationDocumentsInvalid {
            documents_checked,
            failing,
        } => {
            let envelope = ApiErrorEnvelope::new(
                ApiError::new(
                    ErrorCode::Conflict,
                    "stored documents do not satisfy the migration's target schema yet; \
                     update them, then complete the migration again",
                    request.request_id(),
                    RetryAdvice::Never,
                )
                .with_detail(
                    "documentsChecked",
                    SafeDetail::String(documents_checked.to_string()),
                )
                .with_detail(
                    "failingDocuments",
                    SafeDetail::String(
                        failing
                            .iter()
                            .take(20)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                ),
            );
            HttpApiError::from_envelope(409, envelope)
        }
        CollectionAdminError::MigrationTargetUnknown => conflict(
            request,
            "this migration predates recorded targets; plan it again to complete it",
        ),
        CollectionAdminError::MigrationStale => conflict(
            request,
            "the collection is no longer on the schema version this migration starts from",
        ),
        CollectionAdminError::MigrationChangesPrimaryKey => {
            conflict(request, "a migration cannot change the primary key")
        }
        CollectionAdminError::Conflict
        | CollectionAdminError::InvalidMigrationTransition
        | CollectionAdminError::MigrationNotRequired
        | CollectionAdminError::SchemaVersionMustIncrease
        | CollectionAdminError::Index(IndexError::DefinitionAlreadyExists) => {
            conflict(request, "collection lifecycle or version conflict")
        }
        CollectionAdminError::InvalidMigrationId
        | CollectionAdminError::InvalidMigrationReason
        | CollectionAdminError::Scope(_)
        | CollectionAdminError::EngineScope(_)
        | CollectionAdminError::Metadata(_)
        | CollectionAdminError::Validation(_)
        | CollectionAdminError::Index(IndexError::InvalidDefinition { .. }) => {
            invalid(request, "collection input is invalid")
        }
        _ => unavailable(request, "collection administration is unavailable"),
    }
}

fn collection_wire(record: &CollectionMetadata) -> CollectionWire {
    CollectionWire {
        id: record.collection_id().as_str().to_owned(),
        metadata_version: record.metadata_version().get(),
        schema_version: record.schema_version().get(),
        json_schema: Value::Object(record.json_schema().clone()),
        primary_key: record.primary_key().clone(),
        compatibility: record.compatibility(),
        state: record.lifecycle(),
    }
}

fn publication_wire(outcome: &SchemaPublicationOutcome) -> PublicationWire {
    match outcome {
        SchemaPublicationOutcome::Published(record) => PublicationWire {
            status: "published",
            collection: Some(collection_wire(record)),
            compatibility: None,
        },
        SchemaPublicationOutcome::MigrationRequired(report) => PublicationWire {
            status: "migration_required",
            collection: None,
            compatibility: Some(compatibility_wire(report)),
        },
    }
}

fn compatibility_wire(report: &CollectionCompatibilityReport) -> CompatibilityWire {
    CompatibilityWire {
        compatible: report.is_compatible(),
        documents_checked: report.documents_checked(),
        issues: report.issues().to_vec(),
    }
}

fn migration_wire(
    request: &HttpRequest,
    record: &SchemaMigrationRecord,
) -> Result<MigrationWire, HttpApiError> {
    Ok(MigrationWire {
        id: record.id().as_str().to_owned(),
        project_id: record.project_id().as_str().to_owned(),
        environment_id: record.environment_id().as_str().to_owned(),
        collection_id: record.collection_id().as_str().to_owned(),
        from_schema_version: record.from_schema_version(),
        to_schema_version: record.to_schema_version(),
        state: record.state(),
        reason: record.reason().to_owned(),
        compatibility_issues: record.compatibility_issues().to_vec(),
        created_at: format_timestamp(request, record.created_at_unix_seconds())?,
        updated_at: format_timestamp(request, record.updated_at_unix_seconds())?,
    })
}

/// Grant the index to the data plane and report the state it reached there.
///
/// The control plane records the definition, but documents live in the data
/// plane and so does the build. An index that exists only here is one a query
/// can never use, which is why creation is not complete until this returns.
async fn install_index(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    record: &IndexBuildStatus,
) -> Result<IndexState, HttpApiError> {
    let definition = &record.definition;
    let fields: Vec<Value> = definition
        .fields()
        .iter()
        .map(|field| {
            json!({
                "path": field.path(),
                "direction": match field.direction() {
                    mako_documents::IndexDirection::Ascending => "ascending",
                    mako_documents::IndexDirection::Descending => "descending",
                },
            })
        })
        .collect();
    let reported = administer(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::InstallIndex,
        json!({
            "collectionId": collection_id.as_str(),
            "name": definition.name().as_str(),
            "version": definition.version().get(),
            "kind": match definition.kind() {
                IndexKind::NonUnique => "non_unique",
                IndexKind::Unique => "unique",
            },
            "fields": fields,
        }),
        true,
    )
    .await?;
    Ok(reported_index_state(&reported, definition.state()))
}

/// The state a query would actually be answered against.
async fn inspect_index(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    record: &IndexBuildStatus,
) -> Result<IndexState, HttpApiError> {
    let definition = &record.definition;
    let reported = administer(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::InspectIndex,
        json!({
            "collectionId": collection_id.as_str(),
            "name": definition.name().as_str(),
            "version": definition.version().get(),
        }),
        // A read carries no idempotency key.
        false,
    )
    .await?;
    Ok(reported_index_state(&reported, definition.state()))
}

/// The state a read reports for one index. The data plane's report is
/// preferred; when it cannot give one -- the index was recorded but its
/// install was refused or never arrived, the reader's role may not inspect,
/// or the data plane is briefly unreachable -- the control plane's own record
/// stands in. One such index used to fail the whole listing (409 for owners,
/// 403 for everyone else), hiding every other index of the collection.
/// Resubmitting the same index definition completes its install.
fn listed_state(
    record: &IndexBuildStatus,
    inspected: Result<IndexState, HttpApiError>,
) -> IndexState {
    inspected.unwrap_or_else(|_| record.definition.state())
}

fn reported_index_state(reported: &Value, fallback: IndexState) -> IndexState {
    match reported["state"].as_str() {
        Some("active") => IndexState::Active,
        Some("building") => IndexState::Building,
        Some("failed") => IndexState::Failed,
        Some("deleting") => IndexState::Deleting,
        _ => fallback,
    }
}

fn index_wire_with_state(record: &IndexBuildStatus, state: IndexState) -> IndexWire {
    IndexWire {
        state,
        ..index_wire(record)
    }
}

fn index_wire(record: &IndexBuildStatus) -> IndexWire {
    let definition = &record.definition;
    IndexWire {
        collection_id: definition.collection_id().as_str().to_owned(),
        name: definition.name().as_str().to_owned(),
        version: definition.version().get(),
        kind: definition.kind(),
        fields: definition.fields().to_vec(),
        state: definition.state(),
        activation_fenced: definition.activation_fenced(),
        failure: definition.failure().map(|failure| IndexFailureWire {
            code: failure.code(),
            affected_values: failure.affected_values(),
            message: failure.safe_message().to_owned(),
        }),
        progress: record.progress.as_ref().map(|progress| IndexProgressWire {
            captured_position: progress.captured_position(),
            last_document_id: progress.last_document_id().map(|id| id.as_str().to_owned()),
            backfill_complete: progress.backfill_complete(),
            caught_up_position: progress.caught_up_position(),
        }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateCollectionWire {
    id: CollectionId,
    schema_version: u64,
    json_schema: Value,
    primary_key: PrimaryKeyDefinition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PublishSchemaWire {
    schema_version: u64,
    json_schema: Value,
    primary_key: PrimaryKeyDefinition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateMigrationWire {
    target_schema_version: u64,
    target_json_schema: Value,
    target_primary_key: PrimaryKeyDefinition,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationStateWire {
    state: SchemaMigrationState,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexFieldWire {
    path: String,
    direction: IndexDirection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIndexWire {
    name: String,
    version: u64,
    kind: IndexKind,
    fields: Vec<IndexFieldWire>,
}

#[derive(Serialize)]
struct ItemsWire<T> {
    items: Vec<T>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionWire {
    id: String,
    metadata_version: u64,
    schema_version: u64,
    json_schema: Value,
    primary_key: PrimaryKeyDefinition,
    compatibility: mako_documents::SchemaCompatibility,
    state: mako_documents::CollectionLifecycle,
}

#[derive(Serialize)]
struct PublicationWire {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection: Option<CollectionWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<CompatibilityWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompatibilityWire {
    compatible: bool,
    documents_checked: u64,
    issues: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationWire {
    id: String,
    project_id: String,
    environment_id: String,
    collection_id: String,
    from_schema_version: u64,
    to_schema_version: u64,
    state: SchemaMigrationState,
    reason: String,
    compatibility_issues: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexWire {
    collection_id: String,
    name: String,
    version: u64,
    kind: IndexKind,
    fields: Vec<IndexField>,
    state: mako_documents::IndexState,
    activation_fenced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<IndexFailureWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<IndexProgressWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexFailureWire {
    code: mako_documents::IndexFailureCode,
    affected_values: u64,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexProgressWire {
    captured_position: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_document_id: Option<String>,
    backfill_complete: bool,
    caught_up_position: u64,
}

#[cfg(test)]
mod tests {
    use mako_documents::{
        CollectionLifecycle, CollectionMetadataVersion, IndexDefinition, SchemaCompatibility,
        SchemaVersion,
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn an_index_the_data_plane_cannot_report_keeps_its_recorded_state() {
        let record = IndexBuildStatus {
            definition: IndexDefinition::new_building(
                CollectionId::parse("todos").expect("collection"),
                IndexName::parse("by_owner").expect("index name"),
                IndexVersion::new(1).expect("index version"),
                IndexKind::NonUnique,
                [IndexField::ascending("ownerId").expect("index field")],
            )
            .expect("index definition"),
            progress: None,
        };
        let request = HttpRequest::for_test(HttpMethod::Get, "/", Vec::new(), Vec::new(), None);
        // The data plane has never heard of it (install refused), or the
        // reader may not inspect: the listing still answers.
        for refused in [
            conflict(&request, "identity operation conflicts with current state"),
            forbidden(&request, "your team role does not allow this change"),
        ] {
            assert_eq!(listed_state(&record, Err(refused)), IndexState::Building);
        }
        assert_eq!(
            listed_state(&record, Ok(IndexState::Active)),
            IndexState::Active
        );
    }

    #[test]
    fn collection_and_index_wires_match_the_public_contract() {
        let collection = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(2).expect("schema version"),
            json!({"type": "object"}),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        let collection_json =
            serde_json::to_value(collection_wire(&collection)).expect("collection wire");
        assert_eq!(collection_json["metadataVersion"], 1);
        assert_eq!(collection_json["schemaVersion"], 2);
        assert_eq!(collection_json["compatibility"], "compatible");

        let definition = IndexDefinition::new_building(
            CollectionId::parse("todos").expect("collection"),
            IndexName::parse("by_owner").expect("index name"),
            IndexVersion::new(1).expect("index version"),
            IndexKind::NonUnique,
            [IndexField::ascending("ownerId").expect("index field")],
        )
        .expect("index definition");
        let index_json = serde_json::to_value(index_wire(&IndexBuildStatus {
            definition,
            progress: None,
        }))
        .expect("index wire");
        assert_eq!(index_json["activationFenced"], false);
        assert_eq!(index_json["state"], "building");
        assert_eq!(index_json["fields"][0]["direction"], "ascending");
    }
}
