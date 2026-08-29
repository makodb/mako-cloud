use std::{
    collections::BTreeMap,
    num::{NonZeroU64, NonZeroUsize},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice};
use mako_audit::{ActorIdentity, AuditCategory, AuditOutcome};
use mako_identity::{
    AdminAuditOutcome, AdminCreateUserRequest, AdminRequestContext, AdminUpdateUserMetadataRequest,
    AdminUserAction, AdminUserApiError, AdminUserAuditError, AdminUserAuditEvent,
    AdminUserAuditSink, AdminUserPermission, AdminUserService, AppUserId,
    ApplicationUserInvitation, ApplicationUserInvitationSink, IdentityStoreError,
    IssuedProjectCredential, NormalizedEmail, ProjectCredentialId, ProjectCredentialKind,
    ProjectCredentialMetadata, ProjectSigningKeyRecord, ServiceCredentialOperation,
    ServiceCredentialScope, SessionId, TrustedAppMetadata, TrustedMetadataInvalidationError,
    TrustedMetadataInvalidationSink, UserProfileMetadata, VerifiedProjectCredential,
};
use mako_internal_rpc::{
    ChangeFeedEntry, ChangeFeedEvent, DataJobExportPageInput, DataJobExportPageOutput,
    DataJobImportBatchInput, DataJobImportBatchOutput, DataJobRowError, GuardDecision,
    IdentityAdminCommand, IdentityAdminOperation, IdentityAdminPermission,
    IdentityVerificationOperation, IdentityVerificationRequest, IdentityVerificationResponse,
    InstallCollectionInput, InstallCustomDomainsInput, InstallPolicyInput, InternalCaller,
    InternalReplayGuard, InternalRoute, PreparedResponseJournal, ReadChangeFeedInput,
    ReadChangeFeedOutput, ResponseJournalLookup, ResponseJournalStoreOutcome,
    RocksInternalReplayGuardError,
};
use mako_policy::{ExplorerGrantAuthorityRecord, SubjectId};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mako_documents::{
    CollectionMetadata, CollectionMetadataInstallError, CollectionMetadataInstallOutcome,
    CommitPosition, DocumentId, DocumentValidator, MutationCommitOutcome, MutationId,
    MutationInput, PrimaryKeyBrowseOptions, ScopedCollectionSnapshot,
};
use mako_storage::Durability;

use crate::{DataPlaneGraph, auth_http};

type ExportSnapshotRegistry = Arc<Mutex<BTreeMap<String, ExportSnapshotEntry>>>;

struct ExportSnapshotEntry {
    snapshot: Arc<ScopedCollectionSnapshot>,
    tenant: mako_api::TenantScope,
    collection_id: String,
    job_id: String,
}

pub fn add_internal_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let export_snapshots = Arc::new(Mutex::new(BTreeMap::new()));
    let identity_admin_graph = Arc::clone(&graph);
    let identity_admin_snapshots = Arc::clone(&export_snapshots);
    router.add_route(
        HttpMethod::Post,
        InternalRoute::IdentityAdmin.path(),
        move |request| {
            handle_identity_admin(&identity_admin_graph, &identity_admin_snapshots, &request)
        },
    )?;
    router.add_route(
        HttpMethod::Post,
        InternalRoute::IdentityVerify.path(),
        move |request| handle_identity_verify(&graph, &request),
    )
}

fn handle_identity_verify(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        let verified = graph
            .internal_authenticator(InternalCaller::EdgeGateway)
            .verify(InternalRoute::IdentityVerify, request, now)
            .map_err(|error| error.to_http_error(request.request_id()))?;
        let input: IdentityVerificationRequest = serde_json::from_slice(request.body())
            .map_err(|_| auth_http::invalid(request, "identity verification input is invalid"))?;
        let guard = graph
            .internal_replay_guard(&verified.tenant, &verified.tenant)
            .map_err(|_| {
                auth_http::unavailable(request, "identity replay protection is unavailable")
            })?;
        if guard
            .claim(&verified, now)
            .await
            .map_err(|error| map_guard_error(request, error))?
            == GuardDecision::Duplicate
        {
            return Err(conflict(
                request,
                "internal identity request was already used",
            ));
        }

        let response = match input.operation {
            IdentityVerificationOperation::AccessToken => {
                if input.collection_id.is_some() || input.requested_operation.is_some() {
                    return Err(auth_http::invalid(
                        request,
                        "access-token verification scope is invalid",
                    ));
                }
                let identity = graph
                    .verify_access_token(&input.presented_credential, &verified.tenant, now)
                    .await
                    .map_err(|_| {
                        auth_http::unauthenticated(request, "application-user session is invalid")
                    })?;
                let epochs = identity.authorization_epochs();
                IdentityVerificationResponse::AccessToken {
                    user_id: identity.user_id().as_str().to_owned(),
                    role: identity.role().to_owned(),
                    session_id: identity.session_id().as_str().to_owned(),
                    environment_authorization_epoch: epochs.environment,
                    user_authorization_epoch: epochs.user,
                    trusted_claims: identity.trusted_claims().clone(),
                }
            }
            IdentityVerificationOperation::ProjectCredential => {
                let credential = graph
                    .identity_store(&verified.tenant, &verified.tenant)
                    .map_err(|_| {
                        auth_http::unavailable(request, "identity authority is unavailable")
                    })?
                    .verify_project_credential(&input.presented_credential, now)
                    .await
                    .map_err(|_| {
                        auth_http::unavailable(request, "identity authority is unavailable")
                    })?
                    .ok_or_else(|| {
                        auth_http::unauthenticated(request, "project credential is invalid")
                    })?;
                let (credential_id, service) = match &credential {
                    VerifiedProjectCredential::Public(value) => {
                        (value.credential_id().as_str().to_owned(), false)
                    }
                    VerifiedProjectCredential::Service(value) => {
                        (value.credential_id().as_str().to_owned(), true)
                    }
                };
                IdentityVerificationResponse::ProjectCredential {
                    credential_id,
                    service,
                    bypasses_document_policies: credential.can_bypass_document_policies(),
                }
            }
        };
        HttpResponse::json(200, &response).map_err(|_| {
            auth_http::internal_from_id(
                request.request_id(),
                "identity verification response serialization failed",
            )
        })
    })
}

fn handle_identity_admin(
    graph: &Arc<DataPlaneGraph>,
    export_snapshots: &ExportSnapshotRegistry,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = auth_http::now_unix_seconds(request.request_id())?;
    block_on(async {
        let verified = graph
            .internal_authenticator(InternalCaller::ControlPlane)
            .verify(InternalRoute::IdentityAdmin, request, now)
            .map_err(|error| error.to_http_error(request.request_id()))?;
        let command: IdentityAdminCommand = serde_json::from_slice(request.body())
            .map_err(|_| auth_http::invalid(request, "identity administration input is invalid"))?;
        let guard = graph
            .internal_replay_guard(&verified.tenant, &verified.tenant)
            .map_err(|_| {
                auth_http::unavailable(request, "identity replay protection is unavailable")
            })?;
        let decision = guard
            .claim(&verified, now)
            .await
            .map_err(|error| map_guard_error(request, error))?;
        let journal = graph
            .internal_response_journal(&verified.tenant, &verified.tenant)
            .map_err(|_| {
                auth_http::unavailable(request, "identity response journal is unavailable")
            })?;

        if decision == GuardDecision::Duplicate {
            return match journal.lookup(&verified, now).await {
                Ok(ResponseJournalLookup::Replay(body)) => {
                    append_admin_audit(
                        graph,
                        &verified.tenant,
                        &command,
                        "identity_admin_idempotent_replay",
                        operation_name(command.operation),
                        AuditOutcome::Allowed,
                        "exact_response_replayed",
                        request.request_id(),
                        now,
                    )
                    .await?;
                    Ok(json_bytes(body))
                }
                Ok(ResponseJournalLookup::Conflict) => Err(conflict(
                    request,
                    "idempotency key was reused for another identity operation",
                )),
                Ok(ResponseJournalLookup::Expired) => Err(conflict(
                    request,
                    "the identity operation retry result has expired",
                )),
                Ok(ResponseJournalLookup::Missing) => Err(auth_http::unavailable(
                    request,
                    "the identity operation result is not yet recoverable",
                )),
                Err(_) => Err(auth_http::unavailable(
                    request,
                    "identity response journal is unavailable",
                )),
            };
        }

        if matches!(
            command.operation,
            IdentityAdminOperation::CreateProjectCredential
                | IdentityAdminOperation::RotateProjectCredential
        ) {
            return execute_one_time_credential(graph, request, &verified, &command, now).await;
        }

        let body = execute_operation(
            graph,
            export_snapshots,
            request,
            &verified.tenant,
            &command,
            now,
        )
        .await?;
        match journal.store(&verified, &body, now).await {
            Ok(ResponseJournalStoreOutcome::Stored) => Ok(json_bytes(body)),
            Ok(ResponseJournalStoreOutcome::Replayed(original)) => Ok(json_bytes(original)),
            Err(_) => Err(auth_http::unavailable(
                request,
                "identity response journal could not be committed",
            )),
        }
    })
}

async fn execute_one_time_credential(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    verified: &mako_internal_rpc::VerifiedInternalRequest,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<HttpResponse, HttpApiError> {
    require_permission(
        graph,
        request,
        &verified.tenant,
        command,
        IdentityAdminPermission::ManageProjectCredentials,
        "project_credential_mutate",
        "project-credentials",
        now,
    )
    .await?;
    let store = graph
        .identity_store(&verified.tenant, &verified.tenant)
        .map_err(|_| auth_http::unavailable(request, "identity authority is unavailable"))?;
    let journal = graph
        .internal_response_journal(&verified.tenant, &verified.tenant)
        .map_err(|_| auth_http::unavailable(request, "identity response journal is unavailable"))?;

    let (body, commit_result, target): (
        Vec<u8>,
        Result<IssuedProjectCredential, IdentityStoreError>,
        String,
    ) = match command.operation {
        IdentityAdminOperation::CreateProjectCredential => {
            let input: CreateProjectCredentialWire = parse_input(request, &command.input)?;
            let id = ProjectCredentialId::parse(input.id)
                .map_err(|_| auth_http::invalid(request, "project credential id is invalid"))?;
            let target = id.as_str().to_owned();
            let prepared = match input.kind {
                ProjectCredentialKind::Public => {
                    if input.service_scope.is_some() {
                        return Err(auth_http::invalid(
                            request,
                            "public credential scope is invalid",
                        ));
                    }
                    store.prepare_public_project_key(id, now)
                }
                ProjectCredentialKind::Service => {
                    let scope = input
                        .service_scope
                        .ok_or_else(|| {
                            auth_http::invalid(request, "service credential scope is required")
                        })?
                        .build(request)?;
                    store.prepare_service_credential(id, scope, now)
                }
            }
            .map_err(|error| map_identity_error(request, error))?;
            let body = issue_body(prepared.issued(), request)?;
            let supplemental = match journal.prepare_atomic(verified, &body, now).await {
                Ok(PreparedResponseJournal::Fresh(write)) => write,
                Ok(PreparedResponseJournal::Replay(original)) => return Ok(json_bytes(original)),
                Ok(PreparedResponseJournal::Conflict) => {
                    return Err(conflict(request, "idempotency key conflicts"));
                }
                Ok(PreparedResponseJournal::Expired) => {
                    return Err(conflict(request, "identity retry result expired"));
                }
                Err(_) => {
                    return Err(auth_http::unavailable(
                        request,
                        "identity response journal is unavailable",
                    ));
                }
            };
            (
                body,
                store
                    .commit_prepared_project_credential(prepared, Some(supplemental))
                    .await,
                target,
            )
        }
        IdentityAdminOperation::RotateProjectCredential => {
            let input: RotateProjectCredentialWire = parse_input(request, &command.input)?;
            let current_id = ProjectCredentialId::parse(input.current_id)
                .map_err(|_| auth_http::invalid(request, "current credential id is invalid"))?;
            let replacement_id = ProjectCredentialId::parse(input.replacement_id)
                .map_err(|_| auth_http::invalid(request, "replacement credential id is invalid"))?;
            let target = current_id.as_str().to_owned();
            let prepared = store
                .prepare_project_credential_rotation(
                    &current_id,
                    replacement_id,
                    input.overlap_seconds,
                    now,
                )
                .await
                .map_err(|error| map_identity_error(request, error))?;
            let body = issue_body(prepared.issued(), request)?;
            let supplemental = match journal.prepare_atomic(verified, &body, now).await {
                Ok(PreparedResponseJournal::Fresh(write)) => write,
                Ok(PreparedResponseJournal::Replay(original)) => return Ok(json_bytes(original)),
                Ok(PreparedResponseJournal::Conflict) => {
                    return Err(conflict(request, "idempotency key conflicts"));
                }
                Ok(PreparedResponseJournal::Expired) => {
                    return Err(conflict(request, "identity retry result expired"));
                }
                Err(_) => {
                    return Err(auth_http::unavailable(
                        request,
                        "identity response journal is unavailable",
                    ));
                }
            };
            (
                body,
                store
                    .commit_prepared_project_credential_rotation(prepared, Some(supplemental))
                    .await,
                target,
            )
        }
        _ => unreachable!("one-time dispatcher restricts operations"),
    };

    match commit_result {
        Ok(_) => {
            append_admin_audit(
                graph,
                &verified.tenant,
                command,
                "project_credential_mutate",
                &target,
                AuditOutcome::Allowed,
                "allowed",
                request.request_id(),
                now,
            )
            .await?;
            Ok(json_bytes(body))
        }
        Err(error) => match journal.lookup(verified, now).await {
            Ok(ResponseJournalLookup::Replay(original)) => {
                append_admin_audit(
                    graph,
                    &verified.tenant,
                    command,
                    "identity_admin_idempotent_replay",
                    operation_name(command.operation),
                    AuditOutcome::Allowed,
                    "concurrent_exact_response_replayed",
                    request.request_id(),
                    now,
                )
                .await?;
                Ok(json_bytes(original))
            }
            _ => Err(map_identity_error(request, error)),
        },
    }
}

async fn execute_operation(
    graph: &Arc<DataPlaneGraph>,
    export_snapshots: &ExportSnapshotRegistry,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    match command.operation {
        IdentityAdminOperation::SearchUsers
        | IdentityAdminOperation::InspectUser
        | IdentityAdminOperation::CreateUser
        | IdentityAdminOperation::InviteUser
        | IdentityAdminOperation::UpdateUserMetadata
        | IdentityAdminOperation::DisableUser
        | IdentityAdminOperation::RestoreUser
        | IdentityAdminOperation::RevokeSession
        | IdentityAdminOperation::RevokeAllSessions
        | IdentityAdminOperation::DeleteUser => {
            let result = execute_user_operation(graph, request, tenant, command, now).await;
            // An operation that adds or removes a user changes the count this
            // tenant is measured on. Marking only on success keeps a rejected
            // call from scheduling work.
            if result.is_ok()
                && matches!(
                    command.operation,
                    IdentityAdminOperation::CreateUser
                        | IdentityAdminOperation::InviteUser
                        | IdentityAdminOperation::DeleteUser
                )
            {
                graph.storage_sampler().mark_users(tenant);
            }
            result
        }
        IdentityAdminOperation::ListProjectCredentials
        | IdentityAdminOperation::InspectProjectCredential
        | IdentityAdminOperation::RetireProjectCredential => {
            execute_credential_operation(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InitializeSigningKey
        | IdentityAdminOperation::RotateSigningKey
        | IdentityAdminOperation::ListSigningKeys => {
            execute_signing_operation(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::IssueExplorerGrant
        | IdentityAdminOperation::RevokeExplorerGrant
        | IdentityAdminOperation::AdvanceExplorerEpoch => {
            execute_explorer_authorization_operation(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::ImportDataJobBatch | IdentityAdminOperation::ExportDataJobPage => {
            execute_data_job_operation(graph, export_snapshots, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallCollection => {
            execute_collection_operation(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallPolicy => {
            execute_policy_operation(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallQuotaPolicy => {
            execute_quota_policy_install(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallIndex => {
            execute_index_install(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InspectIndex => {
            execute_index_inspect(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallAuthProviders
        | IdentityAdminOperation::InspectAuthProviders => {
            crate::auth_provider_http::execute_auth_provider_operation(
                graph, request, tenant, command, now,
            )
            .await
        }
        IdentityAdminOperation::InstallBucket
        | IdentityAdminOperation::RemoveBucket
        | IdentityAdminOperation::ListBuckets
        | IdentityAdminOperation::InspectBucket
        | IdentityAdminOperation::ListBucketObjects
        | IdentityAdminOperation::DeleteBucketObject => {
            crate::storage_http::execute_bucket_operation(graph, request, tenant, command, now)
                .await
        }
        IdentityAdminOperation::ReadChangeFeed => {
            execute_change_feed(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::InstallCustomDomains => {
            execute_custom_domains_install(graph, request, tenant, command, now).await
        }
        IdentityAdminOperation::CreateProjectCredential
        | IdentityAdminOperation::RotateProjectCredential => Err(auth_http::invalid(
            request,
            "identity operation dispatch is invalid",
        )),
    }
}

/// The most change-feed entries one call hands out. The webhook worker pages
/// through a busy collection in slices of this size or smaller.
const CHANGE_FEED_MAXIMUM_LIMIT: u32 = 500;

/// Serve one page of a collection's committed change log to the control
/// plane's webhook worker. Only positions, revisions, and the kind of change
/// cross this boundary; document fields stay in the data plane, so a webhook
/// delivery can never carry data the endpoint was not entitled to.
/// Replaces the verified custom hostnames an environment is served on. The
/// list is validated to the shape the control plane stores -- lowercase
/// DNS names -- and installed whole; an empty list withdraws every
/// hostname.
async fn execute_custom_domains_install(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageProjectCredentials,
        "custom_domains_install",
        "custom-domains",
        now,
    )
    .await?;
    let input: InstallCustomDomainsInput = parse_input(request, &command.input)?;
    if input.hostnames.len() > crate::graph::MAXIMUM_CUSTOM_DOMAINS_PER_ENVIRONMENT {
        return Err(auth_http::invalid(
            request,
            "custom domain list is too long",
        ));
    }
    let mut hostnames = Vec::with_capacity(input.hostnames.len());
    for hostname in input.hostnames {
        let valid = !hostname.is_empty()
            && hostname.len() <= 253
            && hostname.contains('.')
            && hostname.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
            });
        if !valid || hostnames.contains(&hostname) {
            return Err(auth_http::invalid(
                request,
                "custom domain hostname is invalid",
            ));
        }
        hostnames.push(hostname);
    }
    let installed = hostnames.len();
    graph
        .custom_domains()
        .install(tenant, hostnames)
        .await
        .map_err(|_| auth_http::unavailable(request, "custom domains could not be recorded"))?;
    append_admin_audit(
        graph,
        tenant,
        command,
        "custom_domains_install",
        "custom-domains",
        AuditOutcome::Allowed,
        "installed",
        request.request_id(),
        now,
    )
    .await?;
    serde_json::to_vec(&serde_json::json!({ "installed": installed }))
        .map_err(|_| auth_http::unavailable(request, "custom domain response could not be encoded"))
}

async fn execute_change_feed(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ReadChangeFeed,
        "change_feed_read",
        "change-feed",
        now,
    )
    .await?;
    let input: ReadChangeFeedInput = parse_input(request, &command.input)?;
    if !(1..=CHANGE_FEED_MAXIMUM_LIMIT).contains(&input.limit) {
        return Err(auth_http::invalid(request, "change feed limit is invalid"));
    }
    let collection_id = mako_api::CollectionId::parse(input.collection_id.clone())
        .map_err(|_| auth_http::invalid(request, "collection id is invalid"))?;
    let scoped = graph
        .document_engine()
        .scope_collection(
            tenant,
            mako_api::CollectionScope::new(tenant.clone(), collection_id),
        )
        .map_err(|_| auth_http::invalid(request, "collection scope is invalid"))?;
    let output = read_change_feed(&scoped, input.after_position, input.limit)
        .await
        .map_err(|failure| match failure {
            ChangeFeedFailure::CollectionNotFound => not_found(request, "collection was not found"),
            ChangeFeedFailure::Unavailable => {
                auth_http::unavailable(request, "collection change log is unavailable")
            }
        })?;
    serde_json::to_vec(&output).map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "change feed response serialization failed",
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChangeFeedFailure {
    CollectionNotFound,
    Unavailable,
}

/// Read the change log after `after_position` against the high water
/// committed when the call started. A page is never wider than the committed
/// high water, so a caller that advances to `scanned_through` sees every
/// change exactly once and in commit order.
async fn read_change_feed(
    scoped: &mako_documents::ScopedCollectionEngine,
    after_position: u64,
    limit: u32,
) -> Result<ReadChangeFeedOutput, ChangeFeedFailure> {
    if scoped
        .collection_metadata()
        .await
        .map_err(|_| ChangeFeedFailure::Unavailable)?
        .is_none()
    {
        return Err(ChangeFeedFailure::CollectionNotFound);
    }
    let high_water = scoped
        .capture_committed_high_water()
        .await
        .map_err(|_| ChangeFeedFailure::Unavailable)?;
    let limit = NonZeroUsize::new(limit as usize).ok_or(ChangeFeedFailure::Unavailable)?;
    let page = scoped
        .read_change_page(after_position, high_water, limit)
        .await
        .map_err(|_| ChangeFeedFailure::Unavailable)?;
    let changes = page
        .changes()
        .iter()
        .map(|change| {
            let change = change.change();
            ChangeFeedEntry {
                document_id: change.document_id().to_owned(),
                revision: change.revision().to_owned(),
                previous_revision: change.previous_revision().map(str::to_owned),
                commit_position: change.commit_position(),
                event: if change.is_deleted() {
                    ChangeFeedEvent::Delete
                } else if change.previous_revision().is_none() {
                    ChangeFeedEvent::Insert
                } else {
                    ChangeFeedEvent::Update
                },
            }
        })
        .collect();
    Ok(ReadChangeFeedOutput {
        changes,
        scanned_through: page.scanned_through(),
        high_water,
        exhausted: page.is_exhausted(),
    })
}

/// Install collection metadata so this data plane can serve the collection.
///
/// The control plane owns the collection record but writes it to its own store,
/// which this data plane cannot read. Without this propagation every document
/// read and write for the collection is rejected as not found.
/// One backfill or catch-up page. Bounded so a single build step cannot hold
/// the request or the write path open indefinitely.
const INDEX_BUILD_PAGE: usize = 512;
/// Enough pages to build any collection this deployment is sized for. A build
/// that exhausts the budget stays `building` and resumes on the next call,
/// because every page commits its progress marker with its entries.
const INDEX_BUILD_MAX_PAGES: usize = 4_096;

/// Record the limits this tenant is held to.
///
/// The policy arrives already resolved: the control plane owns the plan and any
/// exception made to it, so translating those into limits happens there and
/// this only has to store what it was given. Written durably, because a tenant
/// silently reverting to the deployment default after a restart would be a
/// customer quietly losing what they pay for.
async fn execute_quota_policy_install(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageCollections,
        "quota_policy_install",
        "quotas",
        now,
    )
    .await?;
    let input: mako_internal_rpc::InstallQuotaPolicyInput = parse_input(request, &command.input)?;
    let encoded = serde_json::to_vec(&input.policy)
        .map_err(|_| auth_http::invalid(request, "quota policy is invalid"))?;
    // Decoded before it is stored, so a policy the gateway could not read is
    // rejected here rather than on the request path of every later call.
    let _: mako_gateway::GatewayQuotaPolicy = serde_json::from_slice(&encoded)
        .map_err(|_| auth_http::invalid(request, "quota policy is invalid"))?;
    let key = mako_gateway::PersistentQuotaPolicySource::policy_key(tenant)
        .map_err(|_| auth_http::invalid(request, "tenant scope is invalid"))?;
    let mut batch = mako_storage::WriteBatch::new();
    batch.put(key, encoded);
    graph
        .storage_adapter()
        .write(batch, Durability::Sync)
        .await
        .map_err(|_| auth_http::unavailable(request, "quota policy could not be recorded"))?;

    serde_json::to_vec(&serde_json::json!({ "installed": true }))
        .map_err(|_| auth_http::unavailable(request, "quota response could not be encoded"))
}

async fn execute_index_install(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageCollections,
        "index_install",
        "indexes",
        now,
    )
    .await?;
    let input: mako_internal_rpc::InstallIndexInput = parse_input(request, &command.input)?;
    let (scoped, name, version) = index_target(
        graph,
        request,
        tenant,
        &input.collection_id,
        &input.name,
        input.version,
    )?;

    let kind = match input.kind.as_str() {
        "non_unique" => mako_documents::IndexKind::NonUnique,
        "unique" => mako_documents::IndexKind::Unique,
        _ => return Err(auth_http::invalid(request, "index kind is invalid")),
    };
    let mut fields = Vec::with_capacity(input.fields.len());
    for field in &input.fields {
        let direction = match field.direction.as_str() {
            "ascending" => mako_documents::IndexDirection::Ascending,
            "descending" => mako_documents::IndexDirection::Descending,
            _ => return Err(auth_http::invalid(request, "index direction is invalid")),
        };
        fields.push(
            mako_documents::IndexField::new(field.path.clone(), direction)
                .map_err(|_| auth_http::invalid(request, "index field path is invalid"))?,
        );
    }
    let collection_id = mako_api::CollectionId::parse(input.collection_id.clone())
        .map_err(|_| auth_http::invalid(request, "collection id is invalid"))?;
    let definition = mako_documents::IndexDefinition::new_building(
        collection_id,
        name.clone(),
        version,
        kind,
        fields,
    )
    .map_err(|_| auth_http::invalid(request, "index definition is invalid"))?;

    // Recording is idempotent: a repeat of the same definition is not an error,
    // because the control plane retries propagation.
    match scoped.create_index(definition, Durability::Sync).await {
        Ok(()) => {}
        Err(mako_documents::IndexError::DefinitionAlreadyExists) => {}
        Err(_) => {
            return Err(conflict(
                request,
                "index definition conflicts with the recorded one",
            ));
        }
    }
    let state = build_index(&scoped, request, &name, version).await?;
    report_index_state(
        graph,
        tenant,
        &input.collection_id,
        &input.name,
        input.version,
        state,
        now,
    );
    index_response(request, &input.name, input.version, state)
}

async fn execute_index_inspect(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageCollections,
        "index_inspect",
        "indexes",
        now,
    )
    .await?;
    let input: mako_internal_rpc::InspectIndexInput = parse_input(request, &command.input)?;
    let (scoped, name, version) = index_target(
        graph,
        request,
        tenant,
        &input.collection_id,
        &input.name,
        input.version,
    )?;
    // Reading also advances an unfinished build, so an index that ran out of
    // page budget converges instead of waiting for another write.
    let state = build_index(&scoped, request, &name, version).await?;
    report_index_state(
        graph,
        tenant,
        &input.collection_id,
        &input.name,
        input.version,
        state,
        now,
    );
    index_response(request, &input.name, input.version, state)
}

/// An index's state deciding whether queries are answerable is exactly what
/// the index-state signal exists to show, and this is where that state is
/// computed.
fn report_index_state(
    graph: &Arc<DataPlaneGraph>,
    tenant: &mako_api::TenantScope,
    collection_id: &str,
    index_name: &str,
    index_version: u64,
    state: &str,
    now: u64,
) {
    graph.telemetry().record(mako_api::ObservabilityRecord {
        tenant: tenant.clone(),
        timestamp_unix_milliseconds: now.saturating_mul(1_000),
        payload: mako_api::ObservabilityPayload::IndexState {
            collection_id: collection_id.to_owned(),
            index_name: index_name.to_owned(),
            index_version,
            state: state.to_owned(),
            // Progress in percent is not something the build reports; claiming
            // a number would be invention. Done and not-done are true.
            progress_percent: if state == "active" { 100 } else { 0 },
            message: None,
        },
    });
}

fn index_target(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    collection_id: &str,
    name: &str,
    version: u64,
) -> Result<
    (
        mako_documents::ScopedCollectionEngine,
        mako_documents::IndexName,
        mako_documents::IndexVersion,
    ),
    HttpApiError,
> {
    let collection_id = mako_api::CollectionId::parse(collection_id.to_owned())
        .map_err(|_| auth_http::invalid(request, "collection id is invalid"))?;
    let name = mako_documents::IndexName::parse(name.to_owned())
        .map_err(|_| auth_http::invalid(request, "index name is invalid"))?;
    let version = mako_documents::IndexVersion::new(version)
        .map_err(|_| auth_http::invalid(request, "index version is invalid"))?;
    let scoped = graph
        .document_engine()
        .scope_collection(
            tenant,
            mako_api::CollectionScope::new(tenant.clone(), collection_id),
        )
        .map_err(|_| auth_http::invalid(request, "collection scope is invalid"))?;
    Ok((scoped, name, version))
}

/// Drive an online build as far as one call is allowed to, and report the state
/// the index ended in.
async fn build_index(
    scoped: &mako_documents::ScopedCollectionEngine,
    request: &HttpRequest,
    name: &mako_documents::IndexName,
    version: mako_documents::IndexVersion,
) -> Result<&'static str, HttpApiError> {
    let page = NonZeroUsize::new(INDEX_BUILD_PAGE).expect("page size is not zero");
    if index_state(scoped, request, name, version).await? == "active" {
        return Ok("active");
    }
    for _ in 0..INDEX_BUILD_MAX_PAGES {
        let progress = scoped
            .backfill_index(name, version, page, Durability::Sync)
            .await
            .map_err(|_| conflict(request, "index backfill failed"))?;
        if progress.backfill_complete() {
            break;
        }
    }
    let mut caught_up = u64::MAX;
    for _ in 0..INDEX_BUILD_MAX_PAGES {
        match scoped
            .catch_up_index(name, version, page, Durability::Sync)
            .await
        {
            // Catch-up reports the position it reached; activation is what
            // decides whether that is far enough, so this stops as soon as a
            // pass stops advancing.
            Ok(progress) if progress.caught_up_position() == caught_up => break,
            Ok(progress) => caught_up = progress.caught_up_position(),
            Err(mako_documents::IndexBuildError::BackfillIncomplete) => {
                return Ok("building");
            }
            Err(_) => return Err(conflict(request, "index catch-up failed")),
        }
    }
    match scoped.activate_index(name, version, Durability::Sync).await {
        Ok(_) => Ok("active"),
        Err(
            mako_documents::IndexBuildError::BackfillIncomplete
            | mako_documents::IndexBuildError::CatchUpIncomplete { .. },
        ) => Ok("building"),
        Err(_) => Err(conflict(request, "index activation failed")),
    }
}

async fn index_state(
    scoped: &mako_documents::ScopedCollectionEngine,
    request: &HttpRequest,
    name: &mako_documents::IndexName,
    version: mako_documents::IndexVersion,
) -> Result<&'static str, HttpApiError> {
    let definitions = scoped
        .index_definitions()
        .await
        .map_err(|_| conflict(request, "index catalog is unreadable"))?;
    Ok(definitions
        .iter()
        .find(|definition| definition.name() == name && definition.version() == version)
        .map_or("absent", |definition| match definition.state() {
            mako_documents::IndexState::Active => "active",
            mako_documents::IndexState::Building => "building",
            mako_documents::IndexState::Failed => "failed",
            mako_documents::IndexState::Deleting => "deleting",
        }))
}

fn index_response(
    request: &HttpRequest,
    name: &str,
    version: u64,
    state: &str,
) -> Result<Vec<u8>, HttpApiError> {
    serde_json::to_vec(&serde_json::json!({
        "name": name,
        "version": version,
        "state": state,
    }))
    .map_err(|_| auth_http::unavailable(request, "index response could not be encoded"))
}

async fn execute_collection_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageCollections,
        "collection_install",
        "collections",
        now,
    )
    .await?;
    let input: InstallCollectionInput = parse_input(request, &command.input)?;
    let collection_id = mako_api::CollectionId::parse(input.collection_id)
        .map_err(|_| auth_http::invalid(request, "collection id is invalid"))?;
    let encoded = serde_json::to_vec(&input.metadata)
        .map_err(|_| auth_http::invalid(request, "collection metadata is invalid"))?;
    let metadata = CollectionMetadata::decode(&encoded)
        .map_err(|_| auth_http::invalid(request, "collection metadata is invalid"))?;
    if metadata.collection_id() != &collection_id {
        return Err(auth_http::invalid(
            request,
            "collection metadata does not match the requested collection",
        ));
    }
    let scoped = graph
        .document_engine()
        .scope_collection(
            tenant,
            mako_api::CollectionScope::new(tenant.clone(), collection_id),
        )
        .map_err(|_| auth_http::invalid(request, "collection scope is invalid"))?;
    let outcome = scoped
        .install_collection_metadata(&metadata, Durability::Sync)
        .await
        .map_err(|error| map_collection_install_error(request, error))?;
    let status = match outcome {
        CollectionMetadataInstallOutcome::Created => "created",
        CollectionMetadataInstallOutcome::Unchanged => "unchanged",
        CollectionMetadataInstallOutcome::Updated => "updated",
    };
    serde_json::to_vec(&serde_json::json!({
        "collectionId": metadata.collection_id().as_str(),
        "metadataVersion": metadata.metadata_version().get(),
        "status": status,
    }))
    .map_err(|_| auth_http::unavailable(request, "collection response could not be encoded"))
}

/// Install and activate a document policy so this data plane enforces it.
///
/// Document authorization is default-deny. A policy that exists only in the
/// control plane's own store leaves replication pull returning an empty batch
/// and every push denied, which reads as a working sync that silently carries
/// nothing.
async fn execute_policy_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManagePolicies,
        "policy_install",
        "policies",
        now,
    )
    .await?;
    let input: InstallPolicyInput = parse_input(request, &command.input)?;
    let collection_id = mako_api::CollectionId::parse(input.collection_id)
        .map_err(|_| auth_http::invalid(request, "collection id is invalid"))?;
    let version = mako_policy::PolicyVersion::new(input.version)
        .map_err(|_| auth_http::invalid(request, "policy version is invalid"))?;
    let encoded = serde_json::to_vec(&input.policy)
        .map_err(|_| auth_http::invalid(request, "policy is invalid"))?;
    let policy = mako_policy::PolicySet::decode(&encoded)
        .map_err(|_| auth_http::invalid(request, "policy is invalid"))?;
    let scope = mako_api::CollectionScope::new(tenant.clone(), collection_id.clone());
    if policy.scope() != &scope || policy.version() != version {
        return Err(auth_http::invalid(
            request,
            "policy does not match the requested scope",
        ));
    }

    // Compile against the schema this data plane actually validates documents
    // with, so an activated policy can never disagree with the stored metadata.
    let scoped = graph
        .document_engine()
        .scope_collection(tenant, scope.clone())
        .map_err(|_| auth_http::invalid(request, "collection scope is invalid"))?;
    let metadata = scoped
        .collection_metadata()
        .await
        .map_err(|_| auth_http::unavailable(request, "collection metadata is unavailable"))?
        .ok_or_else(|| not_found(request, "collection was not found"))?;
    let schema = Value::Object(metadata.json_schema().clone());

    let store = graph
        .policy_store(tenant, scope)
        .map_err(|_| auth_http::unavailable(request, "policy storage is unavailable"))?;
    // Idempotent: a retried propagation must converge rather than conflict.
    if store
        .policy_version(version)
        .await
        .map_err(|_| auth_http::unavailable(request, "policy storage is unavailable"))?
        .is_none()
    {
        // The control plane activates locally before propagating, so the policy
        // arrives already active. This store records a version as a draft and
        // activates it separately, so it is normalised back before recording.
        let draft = policy
            .with_state(mako_policy::PolicyState::Draft, Vec::new())
            .map_err(|_| auth_http::invalid(request, "policy is invalid"))?;
        store.create_draft(&draft).await.map_err(|error| {
            conflict(
                request,
                match error {
                    mako_policy::PolicyStoreError::VersionAlreadyExists => {
                        "policy version already exists"
                    }
                    _ => "policy version could not be recorded",
                },
            )
        })?;
    }
    store
        .activate(version, &schema, &mako_policy::PolicyCompiler::default())
        .await
        .map_err(|error| {
            conflict(
                request,
                match error {
                    mako_policy::PolicyStoreError::ValidationFailed(_)
                    | mako_policy::PolicyStoreError::Compile(_) => {
                        "policy does not compile against the collection schema"
                    }
                    mako_policy::PolicyStoreError::NewVersionMustBeDraft => {
                        "policy version is not a draft"
                    }
                    mako_policy::PolicyStoreError::ScopeMismatch => {
                        "policy scope does not match the collection"
                    }
                    mako_policy::PolicyStoreError::VersionNotFound => {
                        "policy version was not recorded"
                    }
                    mako_policy::PolicyStoreError::ConcurrentLifecycleChange => {
                        "policy changed during activation"
                    }
                    _ => "policy could not be activated",
                },
            )
        })?;

    serde_json::to_vec(&serde_json::json!({
        "collectionId": collection_id.as_str(),
        "version": version.get(),
        "status": "active",
    }))
    .map_err(|_| auth_http::unavailable(request, "policy response could not be encoded"))
}

fn map_collection_install_error(
    request: &HttpRequest,
    error: CollectionMetadataInstallError,
) -> HttpApiError {
    match error {
        CollectionMetadataInstallError::ScopeMismatch
        | CollectionMetadataInstallError::Scope(_)
        | CollectionMetadataInstallError::Metadata(_) => {
            auth_http::invalid(request, "collection metadata is invalid")
        }
        CollectionMetadataInstallError::StaleMetadataVersion => conflict(
            request,
            "collection metadata version does not advance the installed version",
        ),
        CollectionMetadataInstallError::ConcurrentModification => {
            conflict(request, "collection metadata changed during installation")
        }
        CollectionMetadataInstallError::Storage(_) => {
            auth_http::unavailable(request, "collection metadata storage is unavailable")
        }
    }
}

async fn execute_data_job_operation(
    graph: &Arc<DataPlaneGraph>,
    export_snapshots: &ExportSnapshotRegistry,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ExecuteDataJobs,
        "data_job_execute",
        "data-jobs",
        now,
    )
    .await?;
    match command.operation {
        IdentityAdminOperation::ImportDataJobBatch => {
            execute_import_batch(graph, request, tenant, command).await
        }
        IdentityAdminOperation::ExportDataJobPage => {
            execute_export_page(graph, export_snapshots, request, tenant, command).await
        }
        _ => unreachable!("data-job dispatcher restricts operations"),
    }
}

async fn execute_import_batch(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
) -> Result<Vec<u8>, HttpApiError> {
    let input: DataJobImportBatchInput = parse_input(request, &command.input)?;
    if input.rows.is_empty()
        || input.rows.len() > 64
        || !valid_data_job_id(&input.job_id)
        || input.collection_id.is_empty()
    {
        return Err(auth_http::invalid(request, "data-job batch is invalid"));
    }
    let collection_id = mako_api::CollectionId::parse(&input.collection_id)
        .map_err(|_| auth_http::invalid(request, "data-job collection is invalid"))?;
    let scope = mako_api::CollectionScope::new(tenant.clone(), collection_id);
    let scoped = graph
        .document_engine()
        .scope_collection(tenant, scope)
        .map_err(|_| auth_http::invalid(request, "data-job scope is invalid"))?;
    let metadata = scoped
        .collection_metadata()
        .await
        .map_err(|_| auth_http::unavailable(request, "data-job schema is unavailable"))?
        .filter(|metadata| {
            metadata.lifecycle() == mako_documents::CollectionLifecycle::Active
                && metadata.compatibility() == mako_documents::SchemaCompatibility::Compatible
                && metadata.schema_version().get() == input.schema_version
        })
        .ok_or_else(|| auth_http::invalid(request, "data-job schema changed"))?;
    let validator = DocumentValidator::compile(&metadata)
        .map_err(|_| auth_http::unavailable(request, "data-job schema is unavailable"))?;
    let sequencer = graph
        .document_engine()
        .scope_sequencer(tenant, tenant, Durability::Sync)
        .map_err(|_| auth_http::unavailable(request, "data-job sequencer is unavailable"))?;
    let mut output = DataJobImportBatchOutput {
        processed: 0,
        committed: 0,
        failed: 0,
        skipped: 0,
        errors: Vec::new(),
    };
    for (offset, row) in input.rows.into_iter().enumerate() {
        let row_number = input.start_row.saturating_add(offset as u64);
        output.processed = output.processed.saturating_add(1);
        let create_validated = match validator.validate_create(row) {
            Ok(validated) => validated,
            Err(_) => {
                output.failed = output.failed.saturating_add(1);
                push_row_error(&mut output, row_number, "schema_invalid");
                continue;
            }
        };
        let document_id = create_validated.primary_key().clone();
        let current = scoped
            .get_document(&document_id)
            .await
            .map_err(|_| auth_http::unavailable(request, "data-job storage is unavailable"))?;
        let (validated, expected) = match (input.conflict_strategy, current.as_ref()) {
            (mako_api::ImportConflictStrategy::CreateOnly, None) => (create_validated, None),
            (mako_api::ImportConflictStrategy::CreateOnly, Some(_)) => {
                output.failed = output.failed.saturating_add(1);
                push_row_error(&mut output, row_number, "already_exists");
                continue;
            }
            (mako_api::ImportConflictStrategy::UpdateExisting, None) => {
                output.skipped = output.skipped.saturating_add(1);
                continue;
            }
            (mako_api::ImportConflictStrategy::UpdateExisting, Some(current))
            | (mako_api::ImportConflictStrategy::Upsert, Some(current)) => {
                let proposed = Value::Object(create_validated.body().clone());
                let validated = match validator.validate_update(current, proposed) {
                    Ok(validated) => validated,
                    Err(_) => {
                        output.failed = output.failed.saturating_add(1);
                        push_row_error(&mut output, row_number, "schema_invalid");
                        continue;
                    }
                };
                (validated, Some(current.revision().clone()))
            }
            (mako_api::ImportConflictStrategy::Upsert, None) => (create_validated, None),
        };
        let mutation_id = MutationId::parse(format!(
            "djob_{}",
            &blake3::hash(format!("{}:{row_number}", input.job_id).as_bytes()).to_hex()[..32]
        ))
        .map_err(|_| auth_http::invalid(request, "data-job mutation is invalid"))?;
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("one is non-zero"))
            .await
            .map_err(|_| auth_http::unavailable(request, "data-job sequencer is unavailable"))?;
        let position = lease.issue().expect("one-position lease issues once");
        let mutation = MutationInput {
            mutation_id,
            commit_position: CommitPosition::new(position).map_err(|_| {
                auth_http::unavailable(request, "data-job sequencer is unavailable")
            })?,
            document: validated,
            durability: Durability::Sync,
        };
        let result = match expected {
            Some(revision) => scoped.update_document(revision, mutation).await,
            None => scoped.create_document(mutation).await,
        };
        match result {
            Ok(MutationCommitOutcome::Applied(_)) => {
                sequencer.mark_committed(position).await.map_err(|_| {
                    auth_http::unavailable(request, "data-job sequencer is unavailable")
                })?;
                output.committed = output.committed.saturating_add(1);
            }
            Ok(MutationCommitOutcome::Replayed(_)) => {
                sequencer.mark_aborted(position).await.map_err(|_| {
                    auth_http::unavailable(request, "data-job sequencer is unavailable")
                })?;
                output.committed = output.committed.saturating_add(1);
            }
            Ok(MutationCommitOutcome::RevisionConflict { .. }) => {
                sequencer.mark_aborted(position).await.map_err(|_| {
                    auth_http::unavailable(request, "data-job sequencer is unavailable")
                })?;
                output.failed = output.failed.saturating_add(1);
                push_row_error(&mut output, row_number, "concurrent_change");
            }
            Err(_) => {
                let _ = sequencer.mark_aborted(position).await;
                let _ = sequencer.recover_high_water().await;
                return Err(auth_http::unavailable(
                    request,
                    "data-job mutation is unavailable",
                ));
            }
        }
        sequencer
            .recover_high_water()
            .await
            .map_err(|_| auth_http::unavailable(request, "data-job sequencer is unavailable"))?;
    }
    serde_json::to_vec(&output).map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "data-job response serialization failed",
        )
    })
}

async fn execute_export_page(
    graph: &Arc<DataPlaneGraph>,
    export_snapshots: &ExportSnapshotRegistry,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
) -> Result<Vec<u8>, HttpApiError> {
    let input: DataJobExportPageInput = parse_input(request, &command.input)?;
    if !(1..=64).contains(&input.limit)
        || !valid_data_job_id(&input.job_id)
        || input.collection_id.is_empty()
    {
        return Err(auth_http::invalid(
            request,
            "data-job export page is invalid",
        ));
    }
    let collection_id = mako_api::CollectionId::parse(&input.collection_id)
        .map_err(|_| auth_http::invalid(request, "data-job collection is invalid"))?;
    let scope = mako_api::CollectionScope::new(tenant.clone(), collection_id.clone());
    let scoped = graph
        .document_engine()
        .scope_collection(tenant, scope)
        .map_err(|_| auth_http::invalid(request, "data-job scope is invalid"))?;
    let metadata = scoped
        .collection_metadata()
        .await
        .map_err(|_| auth_http::unavailable(request, "data-job schema is unavailable"))?
        .filter(|metadata| {
            metadata.lifecycle() == mako_documents::CollectionLifecycle::Active
                && metadata.compatibility() == mako_documents::SchemaCompatibility::Compatible
        })
        .ok_or_else(|| auth_http::invalid(request, "data-job collection is unavailable"))?;
    let (handle, snapshot, after) = if let Some(cursor) = input.cursor.as_deref() {
        let cursor = decode_export_cursor(request, cursor)?;
        let registry = export_snapshots
            .lock()
            .map_err(|_| auth_http::unavailable(request, "data-job snapshot is unavailable"))?;
        let entry = registry
            .get(&cursor.handle)
            .filter(|entry| {
                entry.tenant == *tenant
                    && entry.collection_id == input.collection_id
                    && entry.job_id == input.job_id
                    && entry.snapshot.snapshot_id() == cursor.snapshot_id
            })
            .ok_or_else(|| auth_http::invalid(request, "data-job cursor expired"))?;
        let after = DocumentId::parse(cursor.after_document_id)
            .map_err(|_| auth_http::invalid(request, "data-job cursor is invalid"))?;
        (cursor.handle, Arc::clone(&entry.snapshot), Some(after))
    } else {
        let snapshot =
            Arc::new(scoped.snapshot().await.map_err(|_| {
                auth_http::unavailable(request, "data-job snapshot is unavailable")
            })?);
        let handle = format!(
            "djsnap_{}",
            &blake3::hash(format!("{}:{}", input.job_id, request.request_id()).as_bytes()).to_hex()
                [..24]
        );
        let mut registry = export_snapshots
            .lock()
            .map_err(|_| auth_http::unavailable(request, "data-job snapshot is unavailable"))?;
        if registry.len() >= 32 {
            return Err(auth_http::unavailable(
                request,
                "data-job snapshot capacity is exhausted",
            ));
        }
        registry.insert(
            handle.clone(),
            ExportSnapshotEntry {
                snapshot: Arc::clone(&snapshot),
                tenant: tenant.clone(),
                collection_id: input.collection_id.clone(),
                job_id: input.job_id.clone(),
            },
        );
        (handle, snapshot, None)
    };
    let page = snapshot
        .browse_primary_keys(
            PrimaryKeyBrowseOptions::new(
                NonZeroUsize::new(input.limit as usize).expect("validated export limit"),
                NonZeroUsize::new(192 * 1024).expect("export byte limit"),
                scoped.storage_capabilities().maximum_scan_items,
                false,
            ),
            after.as_ref(),
            None,
        )
        .await
        .map_err(|_| auth_http::unavailable(request, "data-job export is unavailable"))?;
    let next_cursor = page
        .next_after()
        .map(|after| {
            encode_export_cursor(&ExportCursorWire {
                handle: handle.clone(),
                snapshot_id: snapshot.snapshot_id(),
                after_document_id: after.as_str().to_owned(),
            })
        })
        .transpose()
        .map_err(|_| auth_http::unavailable(request, "data-job cursor is unavailable"))?;
    if next_cursor.is_none() {
        export_snapshots
            .lock()
            .map_err(|_| auth_http::unavailable(request, "data-job snapshot is unavailable"))?
            .remove(&handle);
    }
    let output = DataJobExportPageOutput {
        rows: page
            .documents()
            .iter()
            .map(|document| Value::Object(document.body().clone()))
            .collect(),
        next_cursor,
        snapshot: format!("{}:{}", handle, snapshot.snapshot_id()),
        schema_version: metadata.schema_version().get(),
    };
    serde_json::to_vec(&output).map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "data-job response serialization failed",
        )
    })
}

fn push_row_error(output: &mut DataJobImportBatchOutput, row: u64, code: &str) {
    if output.errors.len() < 100 {
        output.errors.push(DataJobRowError {
            row,
            code: code.to_owned(),
        });
    }
}

fn valid_data_job_id(value: &str) -> bool {
    value.starts_with("djob_")
        && (13..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExportCursorWire {
    handle: String,
    snapshot_id: u64,
    after_document_id: String,
}

fn encode_export_cursor(cursor: &ExportCursorWire) -> Result<String, serde_json::Error> {
    serde_json::to_vec(cursor).map(|bytes| format!("djc1_{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn decode_export_cursor(
    request: &HttpRequest,
    cursor: &str,
) -> Result<ExportCursorWire, HttpApiError> {
    if cursor.len() > 4_096 {
        return Err(auth_http::invalid(request, "data-job cursor is invalid"));
    }
    let bytes = cursor
        .strip_prefix("djc1_")
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or_else(|| auth_http::invalid(request, "data-job cursor is invalid"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| auth_http::invalid(request, "data-job cursor is invalid"))
}

async fn execute_explorer_authorization_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    require_permission(
        graph,
        request,
        tenant,
        command,
        IdentityAdminPermission::ManageExplorerGrants,
        "explorer_grant_admin",
        "explorer-grants",
        now,
    )
    .await?;
    let store = graph
        .explorer_authorization_store(tenant, tenant)
        .map_err(|_| {
            auth_http::unavailable(request, "explorer authorization state is unavailable")
        })?;
    match command.operation {
        IdentityAdminOperation::IssueExplorerGrant => {
            let record: ExplorerGrantAuthorityRecord = parse_input(request, &command.input)?;
            if record.developer_identity_id != command.actor_id {
                return Err(auth_http::invalid(
                    request,
                    "explorer grant actor is invalid",
                ));
            }
            store.record_issue(&record).await.map_err(|_| {
                auth_http::unavailable(request, "explorer grant could not be recorded")
            })?;
            serde_json::to_vec(&MutationAcceptedWire { accepted: true })
        }
        IdentityAdminOperation::RevokeExplorerGrant => {
            let input: ExplorerGrantRevocationWire = parse_input(request, &command.input)?;
            if input.developer_identity_id != command.actor_id {
                return Err(auth_http::invalid(
                    request,
                    "explorer grant actor is invalid",
                ));
            }
            store
                .revoke(&input.nonce, &input.developer_identity_id, now)
                .await
                .map_err(|_| not_found(request, "explorer grant was not found"))?;
            serde_json::to_vec(&MutationAcceptedWire { accepted: true })
        }
        IdentityAdminOperation::AdvanceExplorerEpoch => {
            let input: ExplorerEpochAdvanceWire = parse_input(request, &command.input)?;
            let (epoch, advanced) = if input.all_developers {
                let advanced = store.advance_all_epochs().await.map_err(|_| {
                    auth_http::unavailable(request, "explorer epochs could not be advanced")
                })?;
                (None, advanced)
            } else {
                let developer = input.developer_identity_id.as_deref().ok_or_else(|| {
                    auth_http::invalid(request, "explorer epoch target is required")
                })?;
                let epoch = store.advance_epoch(developer).await.map_err(|_| {
                    auth_http::unavailable(request, "explorer epoch could not be advanced")
                })?;
                (Some(epoch), 1)
            };
            serde_json::to_vec(&ExplorerEpochWire { epoch, advanced })
        }
        _ => unreachable!("explorer dispatcher restricts operations"),
    }
    .map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "explorer authorization response serialization failed",
        )
    })
}

async fn execute_user_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    let required = if matches!(
        command.operation,
        IdentityAdminOperation::SearchUsers | IdentityAdminOperation::InspectUser
    ) {
        IdentityAdminPermission::ReadApplicationUsers
    } else {
        IdentityAdminPermission::ManageApplicationUsers
    };
    require_permission(
        graph,
        request,
        tenant,
        command,
        required,
        "application_user_admin",
        "application-users",
        now,
    )
    .await?;
    let store = graph
        .identity_store(tenant, tenant)
        .map_err(|_| auth_http::unavailable(request, "identity authority is unavailable"))?;
    let audit = DataPlaneIdentityAudit {
        graph: Arc::clone(graph),
    };
    let invitations = PersistentInvitationAudit::new(
        Arc::clone(graph),
        command.actor_id.clone(),
        request.request_id().to_owned(),
        now,
    );
    let invalidations = DataPlaneMetadataInvalidation {
        graph: Arc::clone(graph),
    };
    let context = AdminRequestContext::new(
        tenant.clone(),
        &command.actor_id,
        request.request_id(),
        user_permissions(command),
        now,
    )
    .map_err(|_| auth_http::invalid(request, "identity audit context is invalid"))?;
    let service = AdminUserService::new(&store, &audit, &invitations, &invalidations);
    let body = match command.operation {
        IdentityAdminOperation::SearchUsers => {
            let input: SearchUsersWire = parse_input(request, &command.input)?;
            let page = service
                .search(&context, input.query.as_deref(), input.limit)
                .await
                .map_err(|error| map_admin_error(request, error))?;
            serde_json::to_vec(&SearchUsersResponseWire {
                users: page.users,
                truncated: page.truncated,
            })
        }
        IdentityAdminOperation::InspectUser => {
            let user = user_id_input(request, &command.input)?;
            serde_json::to_vec(
                &service
                    .inspect(&context, &user)
                    .await
                    .map_err(|error| map_admin_error(request, error))?,
            )
        }
        IdentityAdminOperation::CreateUser | IdentityAdminOperation::InviteUser => {
            let input: CreateUserWire = parse_input(request, &command.input)?;
            let create = create_user_request(request, input)?;
            let view = if command.operation == IdentityAdminOperation::CreateUser {
                service.create(&context, create).await
            } else {
                service.invite(&context, create).await
            }
            .map_err(|error| map_admin_error(request, error))?;
            serde_json::to_vec(&view)
        }
        IdentityAdminOperation::UpdateUserMetadata => {
            let input: UpdateUserWire = parse_input(request, &command.input)?;
            let user = AppUserId::parse(input.user_id)
                .map_err(|_| auth_http::invalid(request, "application user id is invalid"))?;
            let update = AdminUpdateUserMetadataRequest {
                trusted_metadata: TrustedAppMetadata::new(input.trusted_metadata)
                    .map_err(|_| auth_http::invalid(request, "trusted metadata is invalid"))?,
                profile_metadata: UserProfileMetadata::new(input.profile_metadata)
                    .map_err(|_| auth_http::invalid(request, "profile metadata is invalid"))?,
            };
            serde_json::to_vec(
                &service
                    .update_metadata(&context, &user, update)
                    .await
                    .map_err(|error| map_admin_error(request, error))?,
            )
        }
        IdentityAdminOperation::DisableUser
        | IdentityAdminOperation::RestoreUser
        | IdentityAdminOperation::RevokeAllSessions
        | IdentityAdminOperation::DeleteUser => {
            let user = user_id_input(request, &command.input)?;
            let view = match command.operation {
                IdentityAdminOperation::DisableUser => service.disable(&context, &user).await,
                IdentityAdminOperation::RestoreUser => service.restore(&context, &user).await,
                IdentityAdminOperation::RevokeAllSessions => {
                    service.revoke_all_sessions(&context, &user).await
                }
                IdentityAdminOperation::DeleteUser => service.delete(&context, &user).await,
                _ => unreachable!(),
            }
            .map_err(|error| map_admin_error(request, error))?;
            serde_json::to_vec(&view)
        }
        IdentityAdminOperation::RevokeSession => {
            let input: RevokeSessionWire = parse_input(request, &command.input)?;
            let user = AppUserId::parse(input.user_id)
                .map_err(|_| auth_http::invalid(request, "application user id is invalid"))?;
            let session = SessionId::parse(input.session_id)
                .map_err(|_| auth_http::invalid(request, "application session id is invalid"))?;
            serde_json::to_vec(
                &service
                    .revoke_session(&context, &user, &session)
                    .await
                    .map_err(|error| map_admin_error(request, error))?,
            )
        }
        _ => unreachable!("user dispatcher restricts operations"),
    }
    .map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "identity response serialization failed",
        )
    })?;
    if !invitations.healthy() {
        return Err(auth_http::unavailable(
            request,
            "invitation audit is unavailable",
        ));
    }
    if matches!(
        command.operation,
        IdentityAdminOperation::UpdateUserMetadata
            | IdentityAdminOperation::DisableUser
            | IdentityAdminOperation::RestoreUser
            | IdentityAdminOperation::DeleteUser
    ) {
        graph
            .explorer_authorization_store(tenant, tenant)
            .map_err(|_| {
                auth_http::unavailable(request, "explorer authorization state is unavailable")
            })?
            .advance_all_epochs()
            .await
            .map_err(|_| {
                auth_http::unavailable(request, "explorer authorization could not be invalidated")
            })?;
    }
    Ok(body)
}

async fn execute_credential_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    let (permission, action) = match command.operation {
        IdentityAdminOperation::ListProjectCredentials => (
            IdentityAdminPermission::ReadProjectCredentials,
            "project_credential_list",
        ),
        IdentityAdminOperation::InspectProjectCredential => (
            IdentityAdminPermission::ReadProjectCredentials,
            "project_credential_read",
        ),
        IdentityAdminOperation::RetireProjectCredential => (
            IdentityAdminPermission::ManageProjectCredentials,
            "project_credential_retire",
        ),
        _ => unreachable!(),
    };
    let id = if command.operation == IdentityAdminOperation::ListProjectCredentials {
        None
    } else {
        let input: CredentialIdWire = parse_input(request, &command.input)?;
        Some(
            ProjectCredentialId::parse(input.id)
                .map_err(|_| auth_http::invalid(request, "project credential id is invalid"))?,
        )
    };
    require_permission(
        graph,
        request,
        tenant,
        command,
        permission,
        action,
        id.as_ref()
            .map_or("project-credentials", ProjectCredentialId::as_str),
        now,
    )
    .await?;
    let store = graph
        .identity_store(tenant, tenant)
        .map_err(|_| auth_http::unavailable(request, "identity authority is unavailable"))?;
    let body = match command.operation {
        IdentityAdminOperation::ListProjectCredentials => {
            let metadata = store
                .list_project_credential_metadata(
                    NonZeroUsize::new(100).expect("credential list limit is non-zero"),
                )
                .await
                .map_err(|error| map_identity_error(request, error))?;
            serde_json::to_vec(&metadata)
        }
        IdentityAdminOperation::InspectProjectCredential => {
            let id = id.as_ref().expect("inspect parsed a credential id");
            let metadata = store
                .project_credential_metadata(id)
                .await
                .map_err(|error| map_identity_error(request, error))?
                .ok_or_else(|| not_found(request, "project credential was not found"))?;
            serde_json::to_vec(&metadata)
        }
        IdentityAdminOperation::RetireProjectCredential => {
            let id = id.as_ref().expect("retire parsed a credential id");
            store
                .retire_project_credential(id, now)
                .await
                .map_err(|error| map_identity_error(request, error))?;
            serde_json::to_vec(&MutationAcceptedWire { accepted: true })
        }
        _ => unreachable!(),
    }
    .map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "identity response serialization failed",
        )
    })?;
    append_admin_audit(
        graph,
        tenant,
        command,
        action,
        id.as_ref()
            .map_or("project-credentials", ProjectCredentialId::as_str),
        AuditOutcome::Allowed,
        "allowed",
        request.request_id(),
        now,
    )
    .await?;
    Ok(body)
}

async fn execute_signing_operation(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    now: u64,
) -> Result<Vec<u8>, HttpApiError> {
    let (permission, action) = if command.operation == IdentityAdminOperation::ListSigningKeys {
        (IdentityAdminPermission::ReadSigningKeys, "signing_key_read")
    } else {
        (
            IdentityAdminPermission::ManageSigningKeys,
            "signing_key_rotate",
        )
    };
    require_permission(
        graph,
        request,
        tenant,
        command,
        permission,
        action,
        "signing-keys",
        now,
    )
    .await?;
    let body = match command.operation {
        IdentityAdminOperation::InitializeSigningKey => {
            let record = graph
                .initialize_signing_key(tenant, now)
                .await
                .map_err(|_| conflict(request, "signing key could not be initialized"))?;
            serde_json::to_vec(&SigningKeyViewWire::from(&record))
        }
        IdentityAdminOperation::RotateSigningKey => {
            let input: RotateSigningKeyWire = parse_input(request, &command.input)?;
            let record = graph
                .rotate_signing_key(tenant, now, input.overlap_seconds)
                .await
                .map_err(|_| conflict(request, "signing key could not be rotated"))?;
            serde_json::to_vec(&SigningKeyViewWire::from(&record))
        }
        IdentityAdminOperation::ListSigningKeys => {
            let ring = graph
                .signing_key_ring(tenant)
                .await
                .map_err(|_| auth_http::unavailable(request, "signing keys are unavailable"))?;
            let keys = ring
                .records()
                .iter()
                .map(SigningKeyViewWire::from)
                .collect::<Vec<_>>();
            serde_json::to_vec(&SigningKeysWire { keys })
        }
        _ => unreachable!(),
    }
    .map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "identity response serialization failed",
        )
    })?;
    append_admin_audit(
        graph,
        tenant,
        command,
        action,
        "signing-keys",
        AuditOutcome::Allowed,
        "allowed",
        request.request_id(),
        now,
    )
    .await?;
    Ok(body)
}

fn user_permissions(command: &IdentityAdminCommand) -> Vec<AdminUserPermission> {
    let mut permissions = Vec::new();
    if command
        .permissions
        .contains(&IdentityAdminPermission::ReadApplicationUsers)
    {
        permissions.push(AdminUserPermission::Read);
    }
    if command
        .permissions
        .contains(&IdentityAdminPermission::ManageApplicationUsers)
    {
        permissions.extend([
            AdminUserPermission::Create,
            AdminUserPermission::UpdateMetadata,
            AdminUserPermission::ManageLifecycle,
            AdminUserPermission::RevokeSessions,
            AdminUserPermission::Delete,
        ]);
    }
    permissions
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn require_permission(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    permission: IdentityAdminPermission,
    action: &str,
    target: &str,
    now: u64,
) -> Result<(), HttpApiError> {
    if command.permissions.contains(&permission) {
        return Ok(());
    }
    append_admin_audit(
        graph,
        tenant,
        command,
        action,
        target,
        AuditOutcome::Denied,
        "permission_denied",
        request.request_id(),
        now,
    )
    .await?;
    Err(forbidden(request, "identity administration is forbidden"))
}

#[allow(clippy::too_many_arguments)]
async fn append_admin_audit(
    graph: &DataPlaneGraph,
    tenant: &mako_api::TenantScope,
    command: &IdentityAdminCommand,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    reason: &str,
    request_id: &str,
    now: u64,
) -> Result<(), HttpApiError> {
    auth_http::append_audit(
        graph,
        tenant,
        AuditCategory::Control,
        ActorIdentity::Developer {
            actor_id: command.actor_id.clone(),
        },
        "identity_admin",
        target,
        action,
        outcome,
        reason,
        request_id,
        now,
    )
    .await
}

struct DataPlaneIdentityAudit {
    graph: Arc<DataPlaneGraph>,
}

impl AdminUserAuditSink for DataPlaneIdentityAudit {
    fn record(&self, event: &AdminUserAuditEvent) -> Result<(), AdminUserAuditError> {
        let action = match event.action() {
            AdminUserAction::Search => "application_user_search",
            AdminUserAction::Inspect => "application_user_inspect",
            AdminUserAction::Create => "application_user_create",
            AdminUserAction::Invite => "application_user_invite",
            AdminUserAction::UpdateMetadata => "application_user_update_metadata",
            AdminUserAction::Disable => "application_user_disable",
            AdminUserAction::Restore => "application_user_restore",
            AdminUserAction::RevokeSession => "application_user_revoke_session",
            AdminUserAction::RevokeAllSessions => "application_user_revoke_all_sessions",
            AdminUserAction::Delete => "application_user_delete",
        };
        let (outcome, reason) = match event.outcome() {
            AdminAuditOutcome::Succeeded => (AuditOutcome::Allowed, "allowed"),
            AdminAuditOutcome::Denied => (AuditOutcome::Denied, "permission_denied"),
            AdminAuditOutcome::Failed => (AuditOutcome::Failed, "operation_failed"),
        };
        let graph = Arc::clone(&self.graph);
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(auth_http::append_audit(
                        &graph,
                        event.tenant(),
                        AuditCategory::Control,
                        ActorIdentity::Developer {
                            actor_id: event.actor_id().to_owned(),
                        },
                        "application_user",
                        event.target_id(),
                        action,
                        outcome,
                        reason,
                        event.request_id(),
                        event.occurred_at_unix_seconds(),
                    ))
                })
                .join()
        })
        .map_err(|_| AdminUserAuditError::new("identity audit worker failed"))?
        .map_err(|_| AdminUserAuditError::new("identity audit is unavailable"))
    }
}

struct PersistentInvitationAudit {
    graph: Arc<DataPlaneGraph>,
    actor_id: String,
    request_id: String,
    occurred_at_unix_seconds: u64,
    healthy: AtomicBool,
}

impl PersistentInvitationAudit {
    fn new(
        graph: Arc<DataPlaneGraph>,
        actor_id: String,
        request_id: String,
        occurred_at_unix_seconds: u64,
    ) -> Self {
        Self {
            graph,
            actor_id,
            request_id,
            occurred_at_unix_seconds,
            healthy: AtomicBool::new(true),
        }
    }

    fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

impl ApplicationUserInvitationSink for PersistentInvitationAudit {
    fn enqueue(&self, invitation: ApplicationUserInvitation) {
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    block_on(auth_http::append_audit(
                        &self.graph,
                        invitation.tenant(),
                        AuditCategory::Authentication,
                        ActorIdentity::Developer {
                            actor_id: self.actor_id.clone(),
                        },
                        "application_user",
                        invitation.user_id().as_str(),
                        "application_user_invitation_queued",
                        AuditOutcome::Allowed,
                        "pending_delivery",
                        &self.request_id,
                        self.occurred_at_unix_seconds,
                    ))
                })
                .join()
        });
        if !matches!(result, Ok(Ok(_))) {
            self.healthy.store(false, Ordering::Release);
        }
    }
}

struct DataPlaneMetadataInvalidation {
    graph: Arc<DataPlaneGraph>,
}

#[async_trait]
impl TrustedMetadataInvalidationSink for DataPlaneMetadataInvalidation {
    async fn trusted_metadata_changed(
        &self,
        tenant: &mako_api::TenantScope,
        user_id: &AppUserId,
    ) -> Result<(), TrustedMetadataInvalidationError> {
        let subject = SubjectId::parse(user_id.as_str()).map_err(|_| {
            TrustedMetadataInvalidationError::new("application user subject is invalid")
        })?;
        self.graph
            .authorization_epoch_store(tenant, tenant)
            .map_err(|_| {
                TrustedMetadataInvalidationError::new("authorization epoch store is unavailable")
            })?
            .trusted_claims_changed(&subject)
            .await
            .map_err(|_| {
                TrustedMetadataInvalidationError::new("authorization epoch could not be advanced")
            })?;
        Ok(())
    }
}

fn issue_body(
    issue: &IssuedProjectCredential,
    request: &HttpRequest,
) -> Result<Vec<u8>, HttpApiError> {
    serde_json::to_vec(&IssuedProjectCredentialWire {
        metadata: &issue.metadata,
        credential: issue.credential.expose_once(),
    })
    .map_err(|_| {
        auth_http::internal_from_id(
            request.request_id(),
            "credential response serialization failed",
        )
    })
}

fn user_id_input(request: &HttpRequest, input: &Value) -> Result<AppUserId, HttpApiError> {
    let input: UserIdWire = parse_input(request, input)?;
    AppUserId::parse(input.user_id)
        .map_err(|_| auth_http::invalid(request, "application user id is invalid"))
}

fn create_user_request(
    request: &HttpRequest,
    input: CreateUserWire,
) -> Result<AdminCreateUserRequest, HttpApiError> {
    Ok(AdminCreateUserRequest {
        email: NormalizedEmail::parse(input.email)
            .map_err(|_| auth_http::invalid(request, "application user email is invalid"))?,
        trusted_metadata: TrustedAppMetadata::new(input.trusted_metadata)
            .map_err(|_| auth_http::invalid(request, "trusted metadata is invalid"))?,
        profile_metadata: UserProfileMetadata::new(input.profile_metadata)
            .map_err(|_| auth_http::invalid(request, "profile metadata is invalid"))?,
    })
}

pub(crate) fn parse_input<T: for<'de> Deserialize<'de>>(
    request: &HttpRequest,
    input: &Value,
) -> Result<T, HttpApiError> {
    serde_json::from_value(input.clone())
        .map_err(|_| auth_http::invalid(request, "identity operation input is invalid"))
}

fn map_guard_error(request: &HttpRequest, error: RocksInternalReplayGuardError) -> HttpApiError {
    match error {
        RocksInternalReplayGuardError::IdempotencyMismatch => conflict(
            request,
            "idempotency key was reused for another identity operation",
        ),
        RocksInternalReplayGuardError::Replay => {
            conflict(request, "internal request nonce was already used")
        }
        RocksInternalReplayGuardError::TenantMismatch | RocksInternalReplayGuardError::Scope(_) => {
            auth_http::unauthenticated(request, "internal request tenant is invalid")
        }
        _ => auth_http::unavailable(request, "identity replay protection is unavailable"),
    }
}

fn map_admin_error(request: &HttpRequest, error: AdminUserApiError) -> HttpApiError {
    match error {
        AdminUserApiError::InvalidRequest => {
            auth_http::invalid(request, "application user request is invalid")
        }
        AdminUserApiError::PermissionDenied => {
            forbidden(request, "application user operation is forbidden")
        }
        AdminUserApiError::Store(error) => map_identity_error(request, error),
        AdminUserApiError::Audit(_) | AdminUserApiError::Invalidation(_) => {
            auth_http::unavailable(request, "application user administration is unavailable")
        }
    }
}

fn map_identity_error(request: &HttpRequest, error: IdentityStoreError) -> HttpApiError {
    match error {
        IdentityStoreError::UserNotFound
        | IdentityStoreError::SessionNotFound
        | IdentityStoreError::ProjectCredentialNotFound => {
            not_found(request, "identity resource was not found")
        }
        IdentityStoreError::RecordAlreadyExists | IdentityStoreError::EmailAlreadyExists => {
            conflict(request, "identity resource already exists")
        }
        IdentityStoreError::InvalidAdminQuery
        | IdentityStoreError::InvalidProjectCredential
        | IdentityStoreError::InvalidUserStatusTransition
        | IdentityStoreError::SessionOwnerMismatch
        | IdentityStoreError::Record(_) => {
            auth_http::invalid(request, "identity operation is invalid")
        }
        IdentityStoreError::ConcurrentCredentialChange
        | IdentityStoreError::ConcurrentIdentityChange
        | IdentityStoreError::ConcurrentProjectCredentialChange => {
            conflict(request, "identity resource changed concurrently")
        }
        _ => auth_http::unavailable(request, "identity authority is unavailable"),
    }
}

fn json_bytes(body: Vec<u8>) -> HttpResponse {
    HttpResponse::bytes(200, "application/json; charset=utf-8", body)
}

const fn operation_name(operation: IdentityAdminOperation) -> &'static str {
    match operation {
        IdentityAdminOperation::InstallCollection => "install_collection",
        IdentityAdminOperation::InstallPolicy => "install_policy",
        IdentityAdminOperation::InstallBucket => "install_bucket",
        IdentityAdminOperation::RemoveBucket => "remove_bucket",
        IdentityAdminOperation::ListBuckets => "list_buckets",
        IdentityAdminOperation::InstallAuthProviders => "install_auth_providers",
        IdentityAdminOperation::InspectAuthProviders => "inspect_auth_providers",
        IdentityAdminOperation::InspectBucket => "inspect_bucket",
        IdentityAdminOperation::ListBucketObjects => "list_bucket_objects",
        IdentityAdminOperation::DeleteBucketObject => "delete_bucket_object",
        IdentityAdminOperation::InstallIndex => "install_index",
        IdentityAdminOperation::InstallQuotaPolicy => "install_quota_policy",
        IdentityAdminOperation::InstallCustomDomains => "install_custom_domains",
        IdentityAdminOperation::InspectIndex => "inspect_index",
        IdentityAdminOperation::SearchUsers => "search_users",
        IdentityAdminOperation::InspectUser => "inspect_user",
        IdentityAdminOperation::CreateUser => "create_user",
        IdentityAdminOperation::InviteUser => "invite_user",
        IdentityAdminOperation::UpdateUserMetadata => "update_user_metadata",
        IdentityAdminOperation::DisableUser => "disable_user",
        IdentityAdminOperation::RestoreUser => "restore_user",
        IdentityAdminOperation::RevokeSession => "revoke_session",
        IdentityAdminOperation::RevokeAllSessions => "revoke_all_sessions",
        IdentityAdminOperation::DeleteUser => "delete_user",
        IdentityAdminOperation::CreateProjectCredential => "create_project_credential",
        IdentityAdminOperation::ListProjectCredentials => "list_project_credentials",
        IdentityAdminOperation::InspectProjectCredential => "inspect_project_credential",
        IdentityAdminOperation::RotateProjectCredential => "rotate_project_credential",
        IdentityAdminOperation::RetireProjectCredential => "retire_project_credential",
        IdentityAdminOperation::InitializeSigningKey => "initialize_signing_key",
        IdentityAdminOperation::RotateSigningKey => "rotate_signing_key",
        IdentityAdminOperation::ListSigningKeys => "list_signing_keys",
        IdentityAdminOperation::IssueExplorerGrant => "issue_explorer_grant",
        IdentityAdminOperation::RevokeExplorerGrant => "revoke_explorer_grant",
        IdentityAdminOperation::AdvanceExplorerEpoch => "advance_explorer_epoch",
        IdentityAdminOperation::ImportDataJobBatch => "import_data_job_batch",
        IdentityAdminOperation::ExportDataJobPage => "export_data_job_page",
        IdentityAdminOperation::ReadChangeFeed => "read_change_feed",
    }
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

fn forbidden(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        403,
        ErrorCode::PermissionDenied,
        message,
        request.request_id(),
        RetryAdvice::Never,
    )
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SearchUsersWire {
    query: Option<String>,
    limit: usize,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SearchUsersResponseWire {
    users: Vec<mako_identity::AdminUserSummary>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UserIdWire {
    user_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateUserWire {
    email: String,
    trusted_metadata: Value,
    profile_metadata: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateUserWire {
    user_id: String,
    trusted_metadata: Value,
    profile_metadata: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeSessionWire {
    user_id: String,
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CreateProjectCredentialWire {
    id: String,
    kind: ProjectCredentialKind,
    service_scope: Option<ServiceCredentialScopeWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ServiceCredentialScopeWire {
    collections: Vec<String>,
    operations: Vec<ServiceCredentialOperation>,
}

impl ServiceCredentialScopeWire {
    fn build(self, request: &HttpRequest) -> Result<ServiceCredentialScope, HttpApiError> {
        ServiceCredentialScope::new(self.collections, self.operations)
            .map_err(|_| auth_http::invalid(request, "service credential scope is invalid"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RotateProjectCredentialWire {
    current_id: String,
    replacement_id: String,
    overlap_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CredentialIdWire {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RotateSigningKeyWire {
    overlap_seconds: u64,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct IssuedProjectCredentialWire<'a> {
    metadata: &'a ProjectCredentialMetadata,
    credential: &'a str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MutationAcceptedWire {
    accepted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExplorerGrantRevocationWire {
    nonce: String,
    developer_identity_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExplorerEpochAdvanceWire {
    #[serde(default)]
    developer_identity_id: Option<String>,
    #[serde(default)]
    all_developers: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExplorerEpochWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    advanced: usize,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SigningKeyViewWire {
    key_id: String,
    state: mako_identity::SigningKeyState,
    created_at_unix_seconds: u64,
    retire_at_unix_seconds: Option<u64>,
}

impl From<&ProjectSigningKeyRecord> for SigningKeyViewWire {
    fn from(record: &ProjectSigningKeyRecord) -> Self {
        Self {
            key_id: record.key_id().to_owned(),
            state: record.state(),
            created_at_unix_seconds: record.created_at_unix_seconds(),
            retire_at_unix_seconds: record.retire_at_unix_seconds(),
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SigningKeysWire {
    keys: Vec<SigningKeyViewWire>,
}

#[cfg(test)]
mod change_feed_tests {
    use std::{num::NonZeroU64, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_documents::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, MutationCommitOutcome, MutationId, MutationInput,
        PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion, ScopedCollectionEngine,
    };
    use mako_internal_rpc::ChangeFeedEvent;
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::{ChangeFeedFailure, read_change_feed};

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_feedtest0001").expect("project"),
            EnvironmentId::parse("env_feedtest0001").expect("environment"),
        )
    }

    fn metadata() -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "title": {"type": "string"},
                    "secret": {"type": "string"}
                },
                "required": ["id", "title"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
    }

    /// Three commits on one document -- create, update, delete -- plus one
    /// create on another, each finalized the way the data plane's write path
    /// finalizes them, so the high water reflects all four.
    async fn seed(engine: &DocumentEngine, scoped: &ScopedCollectionEngine) {
        let tenant = tenant();
        scoped
            .install_collection_metadata(&metadata(), Durability::Memory)
            .await
            .expect("metadata installed");
        let validator = DocumentValidator::compile(&metadata()).expect("validator");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Memory)
            .expect("sequencer");
        let mut lease = sequencer
            .lease(NonZeroU64::new(4).expect("non-zero"))
            .await
            .expect("lease");
        let input = |position: u64, id: &str, title: &str| MutationInput {
            mutation_id: MutationId::parse(format!("feed-{position}")).expect("mutation id"),
            commit_position: CommitPosition::new(position).expect("position"),
            document: validator
                .validate_create(json!({"id": id, "title": title, "secret": "hunter2"}))
                .expect("validated document"),
            durability: Durability::Memory,
        };
        let first = lease.issue().expect("position 1");
        let MutationCommitOutcome::Applied(created) = scoped
            .create_document(input(first, "todo-1", "created"))
            .await
            .expect("create")
        else {
            panic!("create must apply");
        };
        sequencer.mark_committed(first).await.expect("committed");
        let second = lease.issue().expect("position 2");
        let MutationCommitOutcome::Applied(updated) = scoped
            .update_document(created.revision, input(second, "todo-1", "updated"))
            .await
            .expect("update")
        else {
            panic!("update must apply");
        };
        sequencer.mark_committed(second).await.expect("committed");
        let third = lease.issue().expect("position 3");
        let MutationCommitOutcome::Applied(_) = scoped
            .delete_document(updated.revision, input(third, "todo-1", "deleted"))
            .await
            .expect("delete")
        else {
            panic!("delete must apply");
        };
        sequencer.mark_committed(third).await.expect("committed");
        let fourth = lease.issue().expect("position 4");
        let MutationCommitOutcome::Applied(_) = scoped
            .create_document(input(fourth, "todo-2", "other"))
            .await
            .expect("create")
        else {
            panic!("create must apply");
        };
        sequencer.mark_committed(fourth).await.expect("committed");
        assert_eq!(sequencer.recover_high_water().await.expect("high water"), 4);
    }

    #[test]
    fn the_change_feed_pages_events_in_commit_order_without_document_fields() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter);
            let tenant = tenant();
            let scoped = engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("scoped");
            seed(&engine, &scoped).await;

            // Before anything is read: the high water is the cursor a new
            // endpoint starts from, so nothing older is ever delivered.
            let probe = read_change_feed(&scoped, 0, 1).await.expect("probe");
            assert_eq!(probe.high_water, 4);
            assert_eq!(probe.changes.len(), 1);
            assert!(!probe.exhausted);

            let first = read_change_feed(&scoped, 0, 2).await.expect("first page");
            assert_eq!(first.scanned_through, 2);
            assert_eq!(first.high_water, 4);
            assert!(!first.exhausted);
            assert_eq!(first.changes[0].document_id, "todo-1");
            assert_eq!(first.changes[0].event, ChangeFeedEvent::Insert);
            assert_eq!(first.changes[0].commit_position, 1);
            assert!(first.changes[0].previous_revision.is_none());
            assert_eq!(first.changes[1].event, ChangeFeedEvent::Update);
            assert_eq!(
                first.changes[1].previous_revision.as_deref(),
                Some(first.changes[0].revision.as_str())
            );

            let second = read_change_feed(&scoped, first.scanned_through, 500)
                .await
                .expect("second page");
            assert_eq!(second.scanned_through, 4);
            assert!(second.exhausted);
            assert_eq!(second.changes.len(), 2);
            assert_eq!(second.changes[0].event, ChangeFeedEvent::Delete);
            assert_eq!(second.changes[0].document_id, "todo-1");
            assert_eq!(second.changes[1].event, ChangeFeedEvent::Insert);
            assert_eq!(second.changes[1].document_id, "todo-2");

            // The wire carries identifiers and revisions only.
            let encoded = serde_json::to_string(&second).expect("json");
            assert!(!encoded.contains("hunter2"));
            assert!(!encoded.contains("title"));
            assert!(encoded.contains("\"documentId\""));

            // Reading past the high water is exhausted, not an error.
            let beyond = read_change_feed(&scoped, 4, 10).await.expect("beyond");
            assert!(beyond.exhausted);
            assert!(beyond.changes.is_empty());
            assert_eq!(beyond.scanned_through, 4);
        });
    }

    #[test]
    fn a_collection_without_metadata_is_not_found() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter);
            let tenant = tenant();
            let scoped = engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("missing").expect("collection"),
                    ),
                )
                .expect("scoped");
            assert_eq!(
                read_change_feed(&scoped, 0, 10).await.expect_err("missing"),
                ChangeFeedFailure::CollectionNotFound
            );
        });
    }
}
