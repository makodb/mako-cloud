//! The service-credential route through which an application's own trusted
//! code sets a user's administrator-controlled app metadata.
//!
//! The route is the identity-side twin of the service document routes: the
//! same explicit `X-Mako-Service-Key`, the same audited privileged bypass
//! established before anything is written, and the same refusal of every
//! other credential. What it writes is trusted metadata only -- the claims a
//! policy may rely on -- so a change also advances the user's authorization
//! epoch, which is what carries it into the user's next token: the token in
//! hand stops verifying, and the refresh that follows re-reads the user.

use std::sync::Arc;

use futures::executor::block_on;
use mako_api::{CollectionId, TenantScope};
use mako_audit::{ActorIdentity, AttributeValue, AuditCategory, AuditOutcome, SafeAttributes};
use mako_gateway::{
    PresentedServiceCredential, ServiceBypassGateway, ServiceBypassGatewayError,
    ServiceBypassGatewayRequest,
};
use mako_identity::{AppUserId, IdentityStoreError, TrustedAppMetadata};
use mako_policy::{
    AuditRequestId, DocumentOperation, PrivilegedAuditWriteError, PrivilegedBypassAuditEvent,
    PrivilegedBypassAuditSink, PrivilegedBypassAuthorizer, PrivilegedBypassReason, SubjectId,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    DataPlaneGraph,
    auth_http::{
        CUSTOM_DOMAIN_HEADER, append_audit_with_details, charge_auth, invalid, json, not_found,
        now_unix_seconds, parse_json, require_json, tenant_for, unauthenticated, unavailable,
    },
    document_http::{conflict, permission_denied, require_presented_request_id},
};

pub(crate) const APP_METADATA_ROUTE: &str =
    "/v1/projects/{projectId}/environments/{environmentId}/service/users/{userId}/app-metadata";

/// The scope target a service credential must list, together with the
/// `update` operation, before it may write app metadata. Document scopes name
/// collections; this reserved name is the identity surface, checked by the
/// same gateway in the same way.
pub(crate) const USERS_SCOPE_TARGET: &str = "users";

/// The audited action of a verified bypass on this route.
pub(crate) const AUDIT_ACTION: &str = "service_user_app_metadata_update";
/// The audited action of a read. Reading another user's trusted claims is a
/// privileged bypass like writing them, and is recorded as one.
pub(crate) const READ_AUDIT_ACTION: &str = "service_user_app_metadata_read";

const SERVICE_KEY_HEADER: &str = "x-mako-service-key";
const AUTHORIZATION_HEADER: &str = "authorization";
const PUBLIC_KEY_HEADER: &str = "x-mako-key";

pub fn add_service_user_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let reader = Arc::clone(&graph);
    router.add_route(HttpMethod::Get, APP_METADATA_ROUTE, move |request| {
        handle_get_app_metadata(&reader, &request)
    })?;
    router.add_route(HttpMethod::Post, APP_METADATA_ROUTE, move |request| {
        handle_set_app_metadata(&graph, &request)
    })
}

/// Reads a user's administrator-controlled metadata under a service
/// credential.
///
/// A function that manages membership has to compose the claim it writes out
/// of the one that is there -- the patch replaces a key whole, so adding one
/// household without this means reconstructing every other from whatever
/// projection the application happens to keep. The epoch comes back with the
/// metadata so the write that follows can say which version it read.
fn handle_get_app_metadata(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    refuse_application_credentials(request)?;
    refuse_public_hostname(request)?;
    let tenant = tenant_for(graph, request)?;
    let user_id = user_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let reason = PrivilegedBypassReason::parse(
            request
                .header("x-mako-bypass-reason")
                .ok_or_else(|| invalid(request, "service bypass reason is required"))?
                .to_owned(),
        )
        .map_err(|_| invalid(request, "service bypass reason is invalid"))?;
        let _bypass = establish_bypass(
            graph,
            request,
            &tenant,
            &user_id,
            reason,
            DocumentOperation::Read,
            READ_AUDIT_ACTION,
            now,
        )
        .await?;
        charge_auth(graph, &tenant, request, now).await?;
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let user = store
            .user_by_id(&user_id)
            .await
            .map_err(|error| map_store_error(request, &error))?
            .ok_or_else(|| not_found(request, "application user was not found"))?;
        let subject = SubjectId::parse(user_id.as_str())
            .map_err(|_| invalid(request, "application user id is invalid"))?;
        let snapshot = graph
            .authorization_epoch_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?
            .epochs_for(&subject)
            .await
            .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?;
        json(
            request,
            200,
            &AppMetadataResultWire {
                user_id: user.id().as_str().to_owned(),
                app_metadata: user.trusted_metadata().values().clone(),
                authorization_epoch: snapshot.user().get(),
            },
        )
    })
}

fn handle_set_app_metadata(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    refuse_application_credentials(request)?;
    refuse_public_hostname(request)?;
    require_json(request)?;
    let tenant = tenant_for(graph, request)?;
    let user_id = user_id(request)?;
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let body: AppMetadataUpdateWire = parse_json(request)?;
        let reason = PrivilegedBypassReason::parse(body.reason)
            .map_err(|_| invalid(request, "service bypass reason is invalid"))?;
        let Value::Object(patch) = body.app_metadata else {
            return Err(invalid(request, "appMetadata must be a JSON object"));
        };
        // The patch is bounded like the metadata it patches, and that much is
        // known before any credential is verified or any record written; the
        // merged result is bounded again below.
        TrustedAppMetadata::new(Value::Object(patch.clone()))
            .map_err(|_| invalid(request, "app metadata exceeds its bounds"))?;
        // Establishing the bypass verifies the credential and appends the
        // audit record; nothing below runs unless both succeeded. The
        // authorizer itself guards a document scope, and this route writes
        // no document, so it is evidence rather than an input.
        let _bypass = establish_bypass(
            graph,
            request,
            &tenant,
            &user_id,
            reason,
            DocumentOperation::Update,
            AUDIT_ACTION,
            now,
        )
        .await?;
        charge_auth(graph, &tenant, request, now).await?;
        let store = graph
            .identity_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "identity authority is unavailable"))?;
        let update = store
            .begin_user_metadata_update(&user_id)
            .await
            .map_err(|error| map_store_error(request, &error))?;
        if let Some(expected) = body.expected_authorization_epoch {
            let subject = SubjectId::parse(user_id.as_str())
                .map_err(|_| invalid(request, "application user id is invalid"))?;
            let current = graph
                .authorization_epoch_store(&tenant, &tenant)
                .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?
                .epochs_for(&subject)
                .await
                .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?;
            if current.user().get() != expected {
                return Err(conflict(
                    request,
                    "app metadata changed since the expected authorization epoch",
                ));
            }
        }
        let merged = update
            .user()
            .trusted_metadata()
            .merge_patch(&patch)
            .map_err(|_| invalid(request, "app metadata exceeds its bounds"))?;
        let profile = update.user().profile_metadata().clone();
        let changed = &merged != update.user().trusted_metadata();
        let subject = SubjectId::parse(user_id.as_str())
            .map_err(|_| invalid(request, "application user id is invalid"))?;
        let epochs = graph
            .authorization_epoch_store(&tenant, &tenant)
            .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?;
        if changed {
            // As on the management path: the user's authorization epoch
            // advances before the write, so no token issued against the old
            // claims keeps verifying once the new ones are stored.
            epochs
                .trusted_claims_changed(&subject)
                .await
                .map_err(|_| unavailable(request, "authorization epoch could not be advanced"))?;
        }
        let user = update
            .commit(merged, profile, now)
            .await
            .map_err(|error| map_store_error(request, &error))?;
        if changed {
            graph
                .explorer_authorization_store(&tenant, &tenant)
                .map_err(|_| unavailable(request, "explorer authorization state is unavailable"))?
                .advance_all_epochs()
                .await
                .map_err(|_| {
                    unavailable(request, "explorer authorization could not be invalidated")
                })?;
        }
        let snapshot = epochs
            .epochs_for(&subject)
            .await
            .map_err(|_| unavailable(request, "authorization epoch store is unavailable"))?;
        json(
            request,
            200,
            &AppMetadataResultWire {
                user_id: user.id().as_str().to_owned(),
                app_metadata: user.trusted_metadata().values().clone(),
                authorization_epoch: snapshot.user().get(),
            },
        )
    })
}

/// A service route accepts the service credential and nothing else: a bearer
/// token or a public project key on it is a misuse to refuse outright, never
/// a fallback to verify.
fn refuse_application_credentials(request: &HttpRequest) -> Result<(), HttpApiError> {
    if !request.header_values(AUTHORIZATION_HEADER).is_empty()
        || !request.header_values(PUBLIC_KEY_HEADER).is_empty()
    {
        return Err(unauthenticated(
            request,
            "service route accepts only a service project key",
        ));
    }
    Ok(())
}

/// The reverse proxy never forwards `/service/` on a custom domain, and the
/// data plane refuses on its own too: a request that arrived on a public
/// application hostname is answered as the proxy would.
fn refuse_public_hostname(request: &HttpRequest) -> Result<(), HttpApiError> {
    if request.header_values(CUSTOM_DOMAIN_HEADER).is_empty() {
        Ok(())
    } else {
        Err(not_found(
            request,
            "resource was not found on this hostname",
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn establish_bypass(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
    tenant: &TenantScope,
    user_id: &AppUserId,
    reason: PrivilegedBypassReason,
    operation: DocumentOperation,
    audit_action: &'static str,
    now: u64,
) -> Result<PrivilegedBypassAuthorizer, HttpApiError> {
    require_presented_request_id(request)?;
    let credential = request
        .header(SERVICE_KEY_HEADER)
        .ok_or_else(|| unauthenticated(request, "service project key is required"))?;
    let request_id = AuditRequestId::parse(request.request_id())
        .map_err(|_| invalid(request, "service request id is invalid"))?;
    let audit = AppMetadataBypassAudit {
        graph: Arc::clone(graph),
        user_id: user_id.clone(),
        action: audit_action,
    };
    ServiceBypassGateway
        .authorize(
            ServiceBypassGatewayRequest {
                tenant: tenant.clone(),
                collection_id: CollectionId::parse(USERS_SCOPE_TARGET)
                    .expect("the reserved identity scope target is a collection identifier"),
                operation,
                credential: PresentedServiceCredential::parse(credential.to_owned())
                    .map_err(|_| unauthenticated(request, "service project key is invalid"))?,
                request_id,
                reason,
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
            ServiceBypassGatewayError::Forbidden => permission_denied(
                request,
                "service credential does not permit app metadata writes",
            ),
            _ => unavailable(request, "service authorization is unavailable"),
        })
}

/// Writes the privileged-bypass record for one target user. The gateway
/// establishes the bypass only after this returns `Ok`, so an audit trail that
/// cannot be written refuses the write it would have described.
struct AppMetadataBypassAudit {
    graph: Arc<DataPlaneGraph>,
    user_id: AppUserId,
    action: &'static str,
}

impl PrivilegedBypassAuditSink for AppMetadataBypassAudit {
    fn record(&self, event: &PrivilegedBypassAuditEvent) -> Result<(), PrivilegedAuditWriteError> {
        let now = now_unix_seconds(event.request_id().as_str())
            .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit is unavailable"))?;
        let details = SafeAttributes::try_from_iter([(
            "bypass_reason".to_owned(),
            AttributeValue::Text(event.reason().as_str().to_owned()),
        )])
        .map_err(|_| PrivilegedAuditWriteError::new("service bypass audit is invalid"))?;
        // The sink is synchronous inside an executor that is already driving
        // the handler; the append runs on its own thread so no executor is
        // entered twice.
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
                        "application_user",
                        self.user_id.as_str(),
                        self.action,
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

fn user_id(request: &HttpRequest) -> Result<AppUserId, HttpApiError> {
    AppUserId::parse(
        request
            .path_parameter("userId")
            .ok_or_else(|| invalid(request, "application user id is invalid"))?,
    )
    .map_err(|_| invalid(request, "application user id is invalid"))
}

fn map_store_error(request: &HttpRequest, error: &IdentityStoreError) -> HttpApiError {
    match error {
        IdentityStoreError::UserNotFound => not_found(request, "application user was not found"),
        IdentityStoreError::ConcurrentIdentityChange => {
            conflict(request, "application user changed concurrently")
        }
        IdentityStoreError::Record(_) => invalid(request, "app metadata is invalid"),
        _ => unavailable(request, "identity authority is unavailable"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AppMetadataUpdateWire {
    reason: String,
    app_metadata: Value,
    /// The user authorization epoch the caller composed this patch against.
    /// A read-modify-write over a claim map is only correct if it can say
    /// which version it read; without it two concurrent membership changes
    /// silently keep whichever wrote last.
    #[serde(default)]
    expected_authorization_epoch: Option<u64>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AppMetadataResultWire {
    user_id: String,
    app_metadata: Map<String, Value>,
    authorization_epoch: u64,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, num::NonZeroUsize, sync::Arc};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use futures::executor::block_on;
    use mako_api::{ErrorCode, TenantScope};
    use mako_audit::{ActorIdentity, AttributeValue, AuditCategory, AuditFilter, AuditOutcome};
    use mako_identity::{
        AccessTokenClaims, AppUserId, AppUserRecord, AppUserStatus, IdentityProvider,
        NormalizedEmail, ProjectCredentialId, ServiceCredentialOperation, ServiceCredentialScope,
        TrustedAppMetadata, UserIdentityId, UserIdentityRecord, UserProfileMetadata,
    };
    use mako_service_runtime::{HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{APP_METADATA_ROUTE, AUDIT_ACTION};
    use crate::{
        DataPlaneGraph, DataPlaneRefreshOutcome,
        graph::test_support::{DeploymentEnvironment, config_for, local_tempdir, tenant},
        service_user_http::READ_AUDIT_ACTION,
    };

    /// What `HttpRequest::for_test` stamps on every request; the route
    /// requires it to be presented explicitly, as the service document routes
    /// do. Because the audit event id is derived from the action and the
    /// request id, one graph can establish exactly one audited bypass under
    /// it -- so each audited case below opens its own graph, and the refusals,
    /// which are answered before any bypass, share the first.
    const REQUEST_ID: &str = "req_test_support";
    const USER_ID: &str = "usr_household_owner";
    const SERVICE_ID: &str = "service_households";
    const NARROW_ID: &str = "service_readonly";

    struct Fixture {
        _directory: TempDir,
        graph: Arc<DataPlaneGraph>,
        router: HttpRouter,
        tenant: TenantScope,
        user_id: AppUserId,
        service_key: String,
        narrow_key: String,
    }

    impl Fixture {
        /// A graph with one active user holding `trusted` app metadata and
        /// a displayName profile, one credential scoped to `users` with
        /// `update` (and a document collection alongside, as an application's
        /// credential would be), and one read-only document credential.
        fn open(prefix: &str, trusted: Value) -> Self {
            let directory = local_tempdir(prefix);
            let config = config_for(directory.path(), DeploymentEnvironment::Local);
            let graph = Arc::new(DataPlaneGraph::open(&config).expect("graph"));
            let router = crate::data_plane_router(Arc::clone(&graph)).expect("router");
            let tenant = tenant();
            let identity = graph
                .identity_store(&tenant, &tenant)
                .expect("identity store");
            let user_id = AppUserId::parse(USER_ID).expect("user id");
            let email = NormalizedEmail::parse("owner@example.test").expect("email");
            let user = AppUserRecord::new(
                tenant.clone(),
                user_id.clone(),
                AppUserStatus::Active,
                TrustedAppMetadata::new(trusted).expect("trusted"),
                UserProfileMetadata::new(json!({"displayName": "Owner"})).expect("profile"),
                1,
            );
            let identity_record = UserIdentityRecord::new(
                tenant.clone(),
                UserIdentityId::parse("idn_household_owner").expect("identity id"),
                user_id.clone(),
                IdentityProvider::Email,
                email.as_str(),
                1,
            )
            .expect("identity record");
            block_on(identity.create_email_user(&user, &identity_record, &email)).expect("user");
            let service_key = block_on(
                identity.create_service_credential(
                    ProjectCredentialId::parse(SERVICE_ID).expect("credential id"),
                    ServiceCredentialScope::new(
                        ["memberships".to_owned(), "users".to_owned()],
                        [
                            ServiceCredentialOperation::Create,
                            ServiceCredentialOperation::Read,
                            ServiceCredentialOperation::Update,
                        ],
                    )
                    .expect("scope"),
                    2,
                ),
            )
            .expect("service credential")
            .credential
            .expose_once()
            .to_owned();
            let narrow_key = block_on(
                identity.create_service_credential(
                    ProjectCredentialId::parse(NARROW_ID).expect("credential id"),
                    ServiceCredentialScope::new(
                        ["transactions".to_owned()],
                        [ServiceCredentialOperation::Read],
                    )
                    .expect("scope"),
                    2,
                ),
            )
            .expect("narrow credential")
            .credential
            .expose_once()
            .to_owned();
            Self {
                _directory: directory,
                graph,
                router,
                tenant,
                user_id,
                service_key,
                narrow_key,
            }
        }

        fn close(self) {
            let Self { graph, router, .. } = self;
            drop(router);
            let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
            block_on(graph.shutdown()).expect("shutdown");
        }

        fn path(&self, user_id: &str) -> String {
            format!(
                "/v1/projects/{}/environments/{}/service/users/{user_id}/app-metadata",
                self.tenant.project_id().as_str(),
                self.tenant.environment_id().as_str(),
            )
        }

        fn service_headers(&self, key: &str) -> Vec<(String, String)> {
            vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("X-Mako-Request-Id".to_owned(), REQUEST_ID.to_owned()),
                ("X-Mako-Service-Key".to_owned(), key.to_owned()),
            ]
        }

        fn send(
            &self,
            user_id: &str,
            headers: Vec<(String, String)>,
            body: &Value,
        ) -> Result<HttpResponse, HttpApiError> {
            self.router
                .dispatch_for_test(HttpRequest::for_test(
                    HttpMethod::Post,
                    self.path(user_id),
                    headers,
                    serde_json::to_vec(body).expect("body"),
                    None,
                ))
                .expect("the app-metadata route is registered")
        }

        fn read(
            &self,
            user_id: &str,
            headers: Vec<(String, String)>,
        ) -> Result<HttpResponse, HttpApiError> {
            self.router
                .dispatch_for_test(HttpRequest::for_test(
                    HttpMethod::Get,
                    self.path(user_id),
                    headers,
                    Vec::new(),
                    None,
                ))
                .expect("the app-metadata route is registered")
        }

        fn stored_user(&self) -> AppUserRecord {
            block_on(
                self.graph
                    .identity_store(&self.tenant, &self.tenant)
                    .expect("identity store")
                    .user_by_id(&self.user_id),
            )
            .expect("lookup")
            .expect("user")
        }

        fn bypass_audit_records(&self) -> Vec<mako_audit::AuditRecord> {
            self.bypass_audit_records_for(AUDIT_ACTION)
        }

        fn bypass_audit_records_for(&self, action: &str) -> Vec<mako_audit::AuditRecord> {
            let now_ms = crate::auth_http::now_unix_seconds(REQUEST_ID).expect("clock") * 1_000;
            block_on(self.graph.audit_store().query(
                &self.tenant,
                &AuditFilter {
                    categories: BTreeSet::from([AuditCategory::ServiceBypass]),
                    action: Some(action.to_owned()),
                    ..AuditFilter::default()
                },
                now_ms.saturating_sub(3_600_000),
                None,
                NonZeroUsize::new(100).expect("non-zero"),
                None,
                now_ms,
            ))
            .expect("audit query")
            .records
        }
    }

    fn body(reason: &str, patch: Value) -> Value {
        json!({ "reason": reason, "appMetadata": patch })
    }

    fn body_expecting(reason: &str, patch: Value, epoch: u64) -> Value {
        json!({ "reason": reason, "appMetadata": patch, "expectedAuthorizationEpoch": epoch })
    }

    fn code(result: Result<HttpResponse, HttpApiError>) -> ErrorCode {
        result
            .expect_err("the request is refused")
            .envelope()
            .error
            .code
    }

    fn response_json(response: HttpResponse) -> Value {
        serde_json::from_slice(response.body_for_test().expect("a body")).expect("JSON")
    }

    fn claims_of(token: &str) -> AccessTokenClaims {
        let encoded = token.split('.').nth(1).expect("JWT payload");
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).expect("base64")).expect("claims")
    }

    #[test]
    fn the_route_is_registered_only_for_a_service_read_and_write() {
        let directory = local_tempdir("data-plane-app-metadata-route");
        let config = config_for(directory.path(), DeploymentEnvironment::Local);
        let graph = Arc::new(DataPlaneGraph::open(&config).expect("graph"));
        let router = crate::data_plane_router(Arc::clone(&graph)).expect("router");
        let path = APP_METADATA_ROUTE
            .replace("{projectId}", "prj_example00")
            .replace("{environmentId}", "env_example00")
            .replace("{userId}", USER_ID);
        assert!(router.permits(HttpMethod::Post, &path));
        // A function composes the claim it writes out of the one that is
        // there, so the read is part of the same surface.
        assert!(router.permits(HttpMethod::Get, &path));
        assert!(!router.permits(HttpMethod::Patch, &path));
        assert!(!router.permits(HttpMethod::Delete, &path));
        for method in [HttpMethod::Get, HttpMethod::Post] {
            assert!(
                !router.permits(method, &path.replace("/service/users/", "/users/")),
                "app metadata has no application-credential route"
            );
        }
        drop(router);
        let graph = Arc::try_unwrap(graph).unwrap_or_else(|_| panic!("route owners dropped"));
        block_on(graph.shutdown()).expect("shutdown");
    }

    /// Every refusal is answered before a bypass is established -- so none of
    /// them writes an audit record or touches the user -- and the one valid
    /// request merges the patch, advances the user's authorization epoch,
    /// invalidates the token in hand, and puts the new claims on the token
    /// the refresh returns, with one bypass record naming credential, reason,
    /// and user.
    #[test]
    fn service_credential_writes_trusted_metadata_that_reaches_the_next_token() {
        let fixture = Fixture::open(
            "data-plane-app-metadata",
            json!({"role": "authenticated", "plan": "free"}),
        );
        let patch = body(
            "invitation accepted",
            json!({"households": {"hh_one": "owner"}, "role": "editor"}),
        );
        let session = block_on(fixture.graph.create_application_session(
            &fixture.tenant,
            &fixture.user_id,
            3,
        ))
        .expect("session");
        let old_token = session
            .access_token
            .expose_for_authorization_header()
            .to_owned();
        assert_eq!(claims_of(&old_token).user_authorization_epoch, 0);

        // Without the service credential: nothing else is accepted in its
        // place, and nothing else is accepted beside it.
        let mut headers = fixture.service_headers(&fixture.service_key);
        headers.retain(|(name, _)| name != "X-Mako-Service-Key");
        assert_eq!(
            code(fixture.send(USER_ID, headers.clone(), &patch)),
            ErrorCode::Unauthenticated
        );
        let mut bearer = headers.clone();
        bearer.push(("Authorization".to_owned(), format!("Bearer {old_token}")));
        assert_eq!(
            code(fixture.send(USER_ID, bearer, &patch)),
            ErrorCode::Unauthenticated,
            "an application bearer never reaches the identity write"
        );
        let mut with_bearer = fixture.service_headers(&fixture.service_key);
        with_bearer.push(("Authorization".to_owned(), format!("Bearer {old_token}")));
        assert_eq!(
            code(fixture.send(USER_ID, with_bearer, &patch)),
            ErrorCode::Unauthenticated,
            "a bearer beside the service key is a misuse, not a fallback"
        );
        let mut public = fixture.service_headers(&fixture.service_key);
        public.push((
            "X-Mako-Key".to_owned(),
            "mako_pk.public_edge.value".to_owned(),
        ));
        assert_eq!(
            code(fixture.send(USER_ID, public, &patch)),
            ErrorCode::Unauthenticated
        );
        let mut custom_domain = fixture.service_headers(&fixture.service_key);
        custom_domain.push((
            "X-Mako-Custom-Domain".to_owned(),
            "api.example.com".to_owned(),
        ));
        assert_eq!(
            code(fixture.send(USER_ID, custom_domain, &patch)),
            ErrorCode::NotFound,
            "a request that arrived on a public application hostname is refused"
        );
        let mut malformed = fixture.service_headers(&fixture.service_key);
        malformed.retain(|(name, _)| name != "X-Mako-Service-Key");
        malformed.push((
            "X-Mako-Service-Key".to_owned(),
            "mako_pk.not.a.service.key".to_owned(),
        ));
        assert_eq!(
            code(fixture.send(USER_ID, malformed, &patch)),
            ErrorCode::Unauthenticated
        );
        let mut unknown_key = fixture.service_headers(&fixture.service_key);
        unknown_key.retain(|(name, _)| name != "X-Mako-Service-Key");
        unknown_key.push((
            "X-Mako-Service-Key".to_owned(),
            format!("mako_sk.service_unknown.{}", "a".repeat(64)),
        ));
        assert_eq!(
            code(fixture.send(USER_ID, unknown_key, &patch)),
            ErrorCode::Unauthenticated
        );
        assert_eq!(
            code(fixture.send(
                USER_ID,
                fixture.service_headers(&fixture.narrow_key),
                &patch
            )),
            ErrorCode::PermissionDenied,
            "a credential without the users scope target cannot write claims"
        );
        let mut no_request_id = fixture.service_headers(&fixture.service_key);
        no_request_id.retain(|(name, _)| name != "X-Mako-Request-Id");
        assert_eq!(
            code(fixture.send(USER_ID, no_request_id, &patch)),
            ErrorCode::InvalidRequest
        );
        let mut not_json = fixture.service_headers(&fixture.service_key);
        not_json.retain(|(name, _)| name != "Content-Type");
        assert_eq!(
            code(fixture.send(USER_ID, not_json, &patch)),
            ErrorCode::InvalidRequest
        );
        for invalid_body in [
            json!({"appMetadata": {"role": "editor"}}),
            body("", json!({"role": "editor"})),
            body(" padded ", json!({"role": "editor"})),
            body(&"r".repeat(513), json!({"role": "editor"})),
            body("reason", json!(["not", "an", "object"])),
            body("reason", json!("string")),
            json!({"reason": "reason", "appMetadata": {}, "profileMetadata": {"x": 1}}),
            body("reason", json!({"blob": "x".repeat(64 * 1024)})),
        ] {
            assert_eq!(
                code(fixture.send(
                    USER_ID,
                    fixture.service_headers(&fixture.service_key),
                    &invalid_body
                )),
                ErrorCode::InvalidRequest,
                "{invalid_body}"
            );
        }
        assert_eq!(
            code(fixture.send(
                &format!("usr_{}", "a".repeat(130)),
                fixture.service_headers(&fixture.service_key),
                &patch
            )),
            ErrorCode::InvalidRequest,
            "a malformed user id is refused before any credential is read"
        );
        assert!(
            fixture.bypass_audit_records().is_empty(),
            "no refusal establishes a bypass"
        );
        assert_eq!(
            fixture.stored_user().trusted_metadata().values(),
            json!({"role": "authenticated", "plan": "free"})
                .as_object()
                .expect("object"),
            "no refusal touches the user"
        );
        assert!(
            block_on(
                fixture
                    .graph
                    .verify_access_token(&old_token, &fixture.tenant, 4)
            )
            .is_ok(),
            "no refusal advances the user's epoch"
        );

        // The valid request.
        let response = fixture
            .send(
                USER_ID,
                fixture.service_headers(&fixture.service_key),
                &patch,
            )
            .expect("metadata written");
        assert_eq!(
            response_json(response),
            json!({
                "userId": USER_ID,
                "appMetadata": {
                    "role": "editor",
                    "plan": "free",
                    "households": {"hh_one": "owner"}
                },
                "authorizationEpoch": 1
            })
        );
        let stored = fixture.stored_user();
        assert_eq!(
            stored.trusted_metadata().values(),
            json!({"role": "editor", "plan": "free", "households": {"hh_one": "owner"}})
                .as_object()
                .expect("object")
        );
        assert_eq!(
            stored.profile_metadata().values(),
            json!({"displayName": "Owner"}).as_object().expect("object"),
            "user-editable metadata is untouched"
        );
        assert_eq!(stored.status(), AppUserStatus::Active);

        // The token in hand was issued at epoch 0 and no longer verifies; the
        // refresh re-reads the user and issues the new claims at epoch 1.
        assert!(
            block_on(
                fixture
                    .graph
                    .verify_access_token(&old_token, &fixture.tenant, 5)
            )
            .is_err()
        );
        let DataPlaneRefreshOutcome::Rotated(refreshed) =
            block_on(fixture.graph.refresh_application_session(
                &fixture.tenant,
                session.refresh_credential.expose_for_token_response(),
                6,
            ))
            .expect("refresh")
        else {
            panic!("the refresh credential is still valid");
        };
        let claims = claims_of(refreshed.access_token.expose_for_authorization_header());
        assert_eq!(claims.user_authorization_epoch, 1);
        assert_eq!(claims.role, "editor");
        assert_eq!(
            claims.trusted_claims,
            *json!({"role": "editor", "plan": "free", "households": {"hh_one": "owner"}})
                .as_object()
                .expect("object")
        );
        assert_eq!(refreshed.authorization_epoch, 1);
        assert!(
            block_on(fixture.graph.verify_access_token(
                refreshed.access_token.expose_for_authorization_header(),
                &fixture.tenant,
                7
            ))
            .is_ok()
        );

        // One privileged-bypass record: the credential as actor, the reason,
        // and the user as the resource.
        let records = fixture.bypass_audit_records();
        assert_eq!(records.len(), 1);
        let event = &records[0].event;
        assert_eq!(records[0].category, AuditCategory::ServiceBypass);
        assert_eq!(
            event.context.actor(),
            &ActorIdentity::Service {
                actor_id: SERVICE_ID.to_owned()
            }
        );
        assert_eq!(event.context.resource().kind(), "application_user");
        assert_eq!(event.context.resource().id(), USER_ID);
        assert_eq!(event.action, AUDIT_ACTION);
        assert_eq!(event.outcome, AuditOutcome::Allowed);
        assert_eq!(event.reason_code, "service_bypass_verified");
        assert_eq!(
            event.details.get("bypass_reason"),
            Some(&AttributeValue::Text("invitation accepted".to_owned()))
        );
        fixture.close();
    }

    /// A function that manages one member of a claim map has to compose the
    /// next value out of the current one, because the patch replaces a key
    /// whole. Reading it is that first half; naming the epoch it read is what
    /// keeps two concurrent changes from silently keeping whichever wrote
    /// last, which is the failure this pair exists to prevent.
    #[test]
    fn a_read_composes_the_next_claim_and_the_epoch_it_read_guards_the_write() {
        let fixture = Fixture::open(
            "data-plane-app-metadata-read",
            json!({"households": {"hh_one": "owner"}}),
        );
        let mut reading = fixture.service_headers(&fixture.service_key);
        reading.push((
            "X-Mako-Bypass-Reason".to_owned(),
            "compose the next households claim".to_owned(),
        ));
        let read = fixture
            .read(USER_ID, reading.clone())
            .expect("metadata read");
        assert_eq!(
            response_json(read),
            json!({
                "userId": USER_ID,
                "appMetadata": {"households": {"hh_one": "owner"}},
                "authorizationEpoch": 0
            })
        );
        // Reading somebody's trusted claims is a privileged bypass, and is
        // recorded as one under its own action.
        assert_eq!(fixture.bypass_audit_records_for(READ_AUDIT_ACTION).len(), 1);
        assert!(fixture.bypass_audit_records().is_empty());
        // The reason is required: a read of somebody's claims is not a thing
        // the audit trail should have to describe as "unspecified".
        assert_eq!(
            code(fixture.read(USER_ID, fixture.service_headers(&fixture.service_key))),
            ErrorCode::InvalidRequest
        );

        // The write names what it read, and adds a household without
        // disturbing the one that was there.
        let response = fixture
            .send(
                USER_ID,
                fixture.service_headers(&fixture.service_key),
                &body_expecting(
                    "accepted an invitation",
                    json!({"households": {"hh_one": "owner", "hh_two": "editor"}}),
                    0,
                ),
            )
            .expect("metadata written");
        assert_eq!(
            response_json(response),
            json!({
                "userId": USER_ID,
                "appMetadata": {"households": {"hh_one": "owner", "hh_two": "editor"}},
                "authorizationEpoch": 1
            })
        );

        fixture.close();
    }

    /// The other half: a write still holding an epoch somebody else has
    /// already moved past is refused, rather than writing the map it composed
    /// from what it read and dropping whatever they added.
    #[test]
    fn a_write_naming_a_stale_epoch_is_refused_and_changes_nothing() {
        let fixture = Fixture::open(
            "data-plane-app-metadata-stale",
            json!({"households": {"hh_one": "owner", "hh_two": "editor"}}),
        );
        let stale = fixture.send(
            USER_ID,
            fixture.service_headers(&fixture.service_key),
            &body_expecting(
                "accepted an invitation",
                json!({"households": {"hh_one": "owner", "hh_three": "viewer"}}),
                4,
            ),
        );
        assert_eq!(code(stale), ErrorCode::Conflict);
        assert_eq!(
            fixture.stored_user().trusted_metadata().values(),
            json!({"households": {"hh_one": "owner", "hh_two": "editor"}})
                .as_object()
                .expect("object"),
            "the refused write changed nothing"
        );
        // The bypass is established and audited before the precondition is
        // read, so the refusal is on the record like any other privileged
        // attempt.
        assert_eq!(fixture.bypass_audit_records().len(), 1);
        fixture.close();
    }

    #[test]
    fn a_null_in_the_patch_removes_the_key_and_the_rest_survives() {
        let fixture = Fixture::open(
            "data-plane-app-metadata-null",
            json!({"role": "editor", "households": {"hh_one": "owner", "hh_two": "viewer"}}),
        );
        let response = fixture
            .send(
                USER_ID,
                fixture.service_headers(&fixture.service_key),
                &body(
                    "member removed",
                    json!({"households": {"hh_two": "viewer"}, "role": null}),
                ),
            )
            .expect("metadata written");
        assert_eq!(
            response_json(response),
            json!({
                "userId": USER_ID,
                "appMetadata": {"households": {"hh_two": "viewer"}},
                "authorizationEpoch": 1
            }),
            "a key set to null is removed and a nested object is replaced whole"
        );
        assert_eq!(fixture.bypass_audit_records().len(), 1);
        fixture.close();
    }

    #[test]
    fn a_patch_that_changes_nothing_is_audited_but_advances_no_epoch() {
        let fixture = Fixture::open(
            "data-plane-app-metadata-noop",
            json!({"households": {"hh_one": "owner"}}),
        );
        let session = block_on(fixture.graph.create_application_session(
            &fixture.tenant,
            &fixture.user_id,
            3,
        ))
        .expect("session");
        let response = fixture
            .send(
                USER_ID,
                fixture.service_headers(&fixture.service_key),
                &body(
                    "reconcile",
                    json!({"households": {"hh_one": "owner"}, "absent": null}),
                ),
            )
            .expect("accepted");
        assert_eq!(
            response_json(response),
            json!({
                "userId": USER_ID,
                "appMetadata": {"households": {"hh_one": "owner"}},
                "authorizationEpoch": 0
            })
        );
        assert!(
            block_on(fixture.graph.verify_access_token(
                session.access_token.expose_for_authorization_header(),
                &fixture.tenant,
                4
            ))
            .is_ok(),
            "unchanged claims leave the token in hand valid"
        );
        assert_eq!(fixture.bypass_audit_records().len(), 1);
        fixture.close();
    }

    #[test]
    fn an_unknown_user_is_not_found_after_the_bypass_is_audited() {
        let fixture = Fixture::open("data-plane-app-metadata-unknown", json!({}));
        assert_eq!(
            code(fixture.send(
                "usr_nobody_here",
                fixture.service_headers(&fixture.service_key),
                &body(
                    "invitation accepted",
                    json!({"households": {"hh_one": "viewer"}})
                ),
            )),
            ErrorCode::NotFound
        );
        let records = fixture.bypass_audit_records();
        assert_eq!(records.len(), 1, "the verified bypass is on record");
        assert_eq!(records[0].event.context.resource().id(), "usr_nobody_here");
        fixture.close();
    }
}
