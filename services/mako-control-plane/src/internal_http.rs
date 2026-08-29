use std::{collections::BTreeSet, sync::Arc};

use futures::executor::block_on;
use mako_api::{ErrorCode, RetryAdvice};
use mako_control_plane::{
    BootstrapDeveloperAdmissionRepairInput, OperatorBootstrapInput, OperatorEntitlementAdminInput,
    OperatorEntitlementChangeKind, OperatorPasswordAuthenticationError, OperatorPermission,
};
use mako_internal_rpc::{
    FunctionSecretResolutionRequest, FunctionSecretResolutionResponse, GuardDecision,
    InternalReplayGuard, InternalRoute, OPERATOR_ADMIN_ENVIRONMENT_ID, OPERATOR_ADMIN_PROJECT_ID,
    OperatorEntitlementApplyResponse, OperatorEntitlementCommand, OperatorEntitlementOperation,
    OperatorEntitlementPlanResponse, ResolvedFunctionSecret, RocksInternalReplayGuardError,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};

use crate::{ControlPlaneGraph, FunctionResolutionError};

pub fn add_internal_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let resolution_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        InternalRoute::FunctionSecretsResolve.path(),
        move |request| handle_resolution(&resolution_graph, &request),
    )?;
    let ask_graph = Arc::clone(&graph);
    router.add_route(HttpMethod::Get, CUSTOM_DOMAIN_ASK_PATH, move |request| {
        handle_custom_domain_ask(&ask_graph, &request)
    })?;
    let plan_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        InternalRoute::OperatorEntitlementPlan.path(),
        move |request| handle_operator_entitlement(&plan_graph, &request, false),
    )?;
    router.add_route(
        HttpMethod::Post,
        InternalRoute::OperatorEntitlementApply.path(),
        move |request| handle_operator_entitlement(&graph, &request, true),
    )
}

/// Caddy's on-demand TLS `ask` endpoint, and the edge gateway's hostname
/// lookup: `200` with the environment for a verified custom domain, `404`
/// for anything else. Unauthenticated, so it answers loopback peers only
/// and says nothing a public caller could not learn by connecting.
pub(crate) const CUSTOM_DOMAIN_ASK_PATH: &str = "/_internal/v1/custom-domains/ask";

fn handle_custom_domain_ask(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    if !request
        .remote_address()
        .is_some_and(|address| address.ip().is_loopback())
    {
        return Err(HttpApiError::new(
            403,
            ErrorCode::PermissionDenied,
            "custom domain lookup is available only over loopback",
            request.request_id(),
            RetryAdvice::Never,
        ));
    }
    if !request.body().is_empty() {
        return Err(invalid(request, "request body is not supported"));
    }
    let mut domains = request
        .query()
        .iter()
        .filter(|(name, _)| name == "domain")
        .map(|(_, value)| value.as_str());
    let domain = domains
        .next()
        .ok_or_else(|| invalid(request, "domain query is required"))?;
    if domains.next().is_some() || request.query().len() != 1 {
        return Err(invalid(request, "domain query is invalid"));
    }
    let not_found = || {
        HttpApiError::new(
            404,
            ErrorCode::NotFound,
            "custom domain is not verified",
            request.request_id(),
            RetryAdvice::Never,
        )
    };
    if domain.is_empty() || domain.len() > 253 || domain.chars().any(char::is_control) {
        return Err(not_found());
    }
    let verified = block_on(graph.custom_domain_service().lookup_verified(domain))
        .map_err(|_| unavailable(request, "custom domain lookup is unavailable"))?
        .ok_or_else(not_found)?;
    HttpResponse::json(
        200,
        &serde_json::json!({
            "hostname": verified.hostname(),
            "projectId": verified.project_id().as_str(),
            "environmentId": verified.environment_id().as_str(),
        }),
    )
    .map_err(|_| unavailable(request, "custom domain lookup response is unavailable"))
}

fn handle_operator_entitlement(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    apply: bool,
) -> Result<HttpResponse, HttpApiError> {
    if !request
        .remote_address()
        .is_some_and(|address| address.ip().is_loopback())
    {
        return Err(HttpApiError::new(
            403,
            ErrorCode::PermissionDenied,
            "operator administration is available only over loopback",
            request.request_id(),
            RetryAdvice::Never,
        ));
    }
    let route = if apply {
        InternalRoute::OperatorEntitlementApply
    } else {
        InternalRoute::OperatorEntitlementPlan
    };
    let now = now_unix_seconds(request.request_id())?;
    let verified = graph
        .operator_admin_authenticator()
        .verify(route, request, now)
        .map_err(|error| error.to_http_error(request.request_id()))?;
    if verified.tenant.project_id().as_str() != OPERATOR_ADMIN_PROJECT_ID
        || verified.tenant.environment_id().as_str() != OPERATOR_ADMIN_ENVIRONMENT_ID
    {
        return Err(operator_admin_forbidden(request));
    }
    let command: OperatorEntitlementCommand = serde_json::from_slice(request.body())
        .map_err(|_| invalid(request, "operator administration input is invalid"))?;
    if command.environment_binding.trim_end_matches('/') != graph.public_origin()
        || verified.idempotency_key != command.idempotency_key
        || (apply != command.typed_confirmation.is_some())
    {
        return Err(operator_admin_forbidden(request));
    }
    let permissions = parse_operator_permissions(&command.permissions)
        .ok_or_else(|| invalid(request, "operator administration input is invalid"))?;

    block_on(async {
        if command.operation == OperatorEntitlementOperation::RepairBootstrapDeveloperAdmission {
            if command.activate_waitlisted || !permissions.is_empty() {
                return Err(invalid(
                    request,
                    "developer-admission repair input is invalid",
                ));
            }
            let input = BootstrapDeveloperAdmissionRepairInput {
                target_email: command.target_email,
                private_reason: command.private_reason,
                environment_binding: command.environment_binding,
                idempotency_key: command.idempotency_key,
                prior_bootstrap_idempotency_key: command
                    .prior_bootstrap_idempotency_key
                    .ok_or_else(|| {
                        invalid(request, "developer-admission repair provenance is required")
                    })?,
            };
            let service = graph
                .operator_password_authentication()
                .bootstrap_developer_admission_repair_service();
            if apply {
                let confirmation = command
                    .typed_confirmation
                    .as_deref()
                    .ok_or_else(|| operator_admin_forbidden(request))?;
                let result = service.apply(&input, confirmation, now).await;
                graph
                    .developer_metrics()
                    .observe_developer_role_repair(result.is_ok());
                let result = result.map_err(|error| map_operator_admin_error(request, error))?;
                return HttpResponse::json(
                    200,
                    &OperatorEntitlementApplyResponse {
                        environment_binding: result.environment_binding,
                        developer_identity_id: result.developer_identity_id.as_str().to_owned(),
                        permission_digest: result.permission_digest,
                        operation_digest: result.operation_digest,
                        developer_status_before: Some(developer_status_name(
                            result.developer_status_before,
                        )),
                        developer_status_after: Some(developer_status_name(
                            result.developer_status_after,
                        )),
                        operator_status_before: result.operator_status_before.to_owned(),
                        operator_status_after: result.operator_status_after.to_owned(),
                        lifecycle_result: "developer_repaired_to_waitlisted".to_owned(),
                        operator_epoch: result.operator_epoch,
                        replayed: result.replayed,
                    },
                )
                .map_err(|_| unavailable(request, "operator administration response failed"));
            }
            let plan = service
                .plan(&input)
                .await
                .map_err(|error| map_operator_admin_error(request, error))?;
            return HttpResponse::json(
                200,
                &OperatorEntitlementPlanResponse {
                    environment_binding: plan.environment_binding,
                    developer_identity_id: plan.developer_identity_id.as_str().to_owned(),
                    email_digest: plan.email_digest,
                    permission_digest: plan.permission_digest,
                    request_digest: plan.request_digest,
                    operation_digest: plan.operation_digest,
                    developer_status_before: Some(developer_status_name(
                        plan.developer_status_before,
                    )),
                    developer_status_after: Some(developer_status_name(
                        plan.developer_status_after,
                    )),
                    operator_status_before: plan.operator_status_before.to_owned(),
                    operator_status_after: plan.operator_status_after.to_owned(),
                    lifecycle_result: "developer_repaired_to_waitlisted".to_owned(),
                    typed_confirmation: plan.typed_confirmation,
                    replayed: plan.replayed,
                },
            )
            .map_err(|_| unavailable(request, "operator administration response failed"));
        }

        if command.prior_bootstrap_idempotency_key.is_some() {
            return Err(invalid(
                request,
                "bootstrap provenance is valid only for developer-admission repair",
            ));
        }
        if command.operation == OperatorEntitlementOperation::InitialBootstrap {
            let input = OperatorBootstrapInput {
                target_email: command.target_email,
                permissions,
                private_reason: command.private_reason,
                environment_binding: command.environment_binding,
                idempotency_key: command.idempotency_key,
                activate_waitlisted: command.activate_waitlisted,
            };
            let service = graph.operator_password_authentication().bootstrap_service();
            if apply {
                let confirmation = command
                    .typed_confirmation
                    .as_deref()
                    .ok_or_else(|| operator_admin_forbidden(request))?;
                let result = service.apply(&input, confirmation, now).await;
                graph
                    .developer_metrics()
                    .observe_operator_bootstrap(result.is_ok());
                let result = result.map_err(|error| map_operator_admin_error(request, error))?;
                return HttpResponse::json(
                    200,
                    &OperatorEntitlementApplyResponse {
                        environment_binding: result.environment_binding,
                        developer_identity_id: result.developer_identity_id.as_str().to_owned(),
                        permission_digest: result.permission_digest,
                        operation_digest: result.operation_digest,
                        developer_status_before: result
                            .developer_status_before
                            .map(developer_status_name),
                        developer_status_after: result
                            .developer_status_after
                            .map(developer_status_name),
                        operator_status_before: result.operator_status_before.to_owned(),
                        operator_status_after: result.operator_status_after.to_owned(),
                        lifecycle_result: result.lifecycle_result.to_owned(),
                        operator_epoch: result.operator_epoch,
                        replayed: result.replayed,
                    },
                )
                .map_err(|_| unavailable(request, "operator administration response failed"));
            }
            let plan = service
                .plan(&input)
                .await
                .map_err(|error| map_operator_admin_error(request, error))?;
            return HttpResponse::json(
                200,
                &OperatorEntitlementPlanResponse {
                    environment_binding: plan.environment_binding,
                    developer_identity_id: plan.developer_identity_id.as_str().to_owned(),
                    email_digest: plan.email_digest,
                    permission_digest: plan.permission_digest,
                    request_digest: plan.request_digest,
                    operation_digest: plan.operation_digest,
                    developer_status_before: plan
                        .developer_status_before
                        .map(developer_status_name),
                    developer_status_after: plan.developer_status_after.map(developer_status_name),
                    operator_status_before: plan.operator_status_before.to_owned(),
                    operator_status_after: plan.operator_status_after.to_owned(),
                    lifecycle_result: plan.lifecycle_result.to_owned(),
                    typed_confirmation: plan.typed_confirmation,
                    replayed: plan.replayed,
                },
            )
            .map_err(|_| unavailable(request, "operator administration response failed"));
        }

        if command.activate_waitlisted {
            return Err(invalid(
                request,
                "wait-list activation is valid only for initial bootstrap",
            ));
        }
        let kind = match command.operation {
            OperatorEntitlementOperation::Grant => OperatorEntitlementChangeKind::Grant,
            OperatorEntitlementOperation::Replace => OperatorEntitlementChangeKind::Replace,
            OperatorEntitlementOperation::Revoke => OperatorEntitlementChangeKind::Revoke,
            OperatorEntitlementOperation::InitialBootstrap => unreachable!(),
            OperatorEntitlementOperation::RepairBootstrapDeveloperAdmission => unreachable!(),
        };
        let input = OperatorEntitlementAdminInput {
            kind,
            target_email: command.target_email,
            permissions,
            private_reason: command.private_reason,
            environment_binding: command.environment_binding,
            idempotency_key: command.idempotency_key,
        };
        let service = graph
            .operator_password_authentication()
            .entitlement_admin_service();
        if apply {
            let confirmation = command
                .typed_confirmation
                .as_deref()
                .ok_or_else(|| operator_admin_forbidden(request))?;
            let result = service
                .apply(&input, confirmation, request.request_id(), now)
                .await
                .map_err(|error| map_operator_admin_error(request, error))?;
            HttpResponse::json(
                200,
                &OperatorEntitlementApplyResponse {
                    environment_binding: result.environment_binding,
                    developer_identity_id: result.developer_identity_id.as_str().to_owned(),
                    permission_digest: result.permission_digest,
                    operation_digest: result.operation_digest,
                    developer_status_before: result
                        .developer_status_before
                        .map(developer_status_name),
                    developer_status_after: result
                        .developer_status_after
                        .map(developer_status_name),
                    operator_status_before: result.operator_status_before.to_owned(),
                    operator_status_after: result.operator_status_after.to_owned(),
                    lifecycle_result: "unchanged".to_owned(),
                    operator_epoch: result.operator_epoch,
                    replayed: result.replayed,
                },
            )
            .map_err(|_| unavailable(request, "operator administration response failed"))
        } else {
            let plan = service
                .plan(&input)
                .await
                .map_err(|error| map_operator_admin_error(request, error))?;
            HttpResponse::json(
                200,
                &OperatorEntitlementPlanResponse {
                    environment_binding: plan.environment_binding,
                    developer_identity_id: plan.developer_identity_id.as_str().to_owned(),
                    email_digest: plan.email_digest,
                    permission_digest: plan.permission_digest,
                    request_digest: plan.request_digest,
                    operation_digest: plan.operation_digest,
                    developer_status_before: plan
                        .developer_status_before
                        .map(developer_status_name),
                    developer_status_after: plan.developer_status_after.map(developer_status_name),
                    operator_status_before: plan.operator_status_before.to_owned(),
                    operator_status_after: plan.operator_status_after.to_owned(),
                    lifecycle_result: "unchanged".to_owned(),
                    typed_confirmation: plan.typed_confirmation,
                    replayed: plan.replayed,
                },
            )
            .map_err(|_| unavailable(request, "operator administration response failed"))
        }
    })
}

fn developer_status_name(status: mako_control_plane::DeveloperIdentityStatus) -> String {
    match status {
        mako_control_plane::DeveloperIdentityStatus::Unverified => "unverified",
        mako_control_plane::DeveloperIdentityStatus::Waitlisted => "waitlisted",
        mako_control_plane::DeveloperIdentityStatus::Active => "active",
        mako_control_plane::DeveloperIdentityStatus::Rejected => "rejected",
        mako_control_plane::DeveloperIdentityStatus::Disabled => "disabled",
        mako_control_plane::DeveloperIdentityStatus::Deleted => "deleted",
    }
    .to_owned()
}

fn parse_operator_permissions(values: &BTreeSet<String>) -> Option<BTreeSet<OperatorPermission>> {
    values
        .iter()
        .map(|value| match value.as_str() {
            "tenant_read" => Some(OperatorPermission::TenantRead),
            "overview_read" => Some(OperatorPermission::OverviewRead),
            "operations_read" => Some(OperatorPermission::OperationsRead),
            "incident_read" => Some(OperatorPermission::IncidentRead),
            "incident_manage" => Some(OperatorPermission::IncidentManage),
            "backup_read" => Some(OperatorPermission::BackupRead),
            "recovery_manage" => Some(OperatorPermission::RecoveryManage),
            "fleet_read" => Some(OperatorPermission::FleetRead),
            "security_read" => Some(OperatorPermission::SecurityRead),
            "security_manage" => Some(OperatorPermission::SecurityManage),
            "activity_read" => Some(OperatorPermission::ActivityRead),
            "activity_export" => Some(OperatorPermission::ActivityExport),
            "provisioning_repair" => Some(OperatorPermission::ProvisioningRepair),
            "quota_override" => Some(OperatorPermission::QuotaOverride),
            "abuse_response" => Some(OperatorPermission::AbuseResponse),
            "support_access" => Some(OperatorPermission::SupportAccess),
            "waitlist_review" => Some(OperatorPermission::WaitlistReview),
            _ => None,
        })
        .collect()
}

fn map_operator_admin_error(
    request: &HttpRequest,
    error: OperatorPasswordAuthenticationError,
) -> HttpApiError {
    match error {
        OperatorPasswordAuthenticationError::NotFound
        | OperatorPasswordAuthenticationError::Ineligible => HttpApiError::new(
            404,
            ErrorCode::NotFound,
            "operator entitlement target is unavailable",
            request.request_id(),
            RetryAdvice::Never,
        ),
        OperatorPasswordAuthenticationError::Conflict
        | OperatorPasswordAuthenticationError::ConfirmationMismatch
        | OperatorPasswordAuthenticationError::LastRecoverableAdministrator => {
            conflict(request, "operator entitlement plan is stale or mismatched")
        }
        OperatorPasswordAuthenticationError::InvalidConfiguration
        | OperatorPasswordAuthenticationError::InvalidCredentials
        | OperatorPasswordAuthenticationError::Unauthenticated
        | OperatorPasswordAuthenticationError::StepUpRequired
        | OperatorPasswordAuthenticationError::RateLimited { .. } => {
            invalid(request, "operator administration input is invalid")
        }
        OperatorPasswordAuthenticationError::Unavailable
        | OperatorPasswordAuthenticationError::Developer(_)
        | OperatorPasswordAuthenticationError::Store(_) => {
            unavailable(request, "operator administration dependency is unavailable")
        }
    }
}

fn operator_admin_forbidden(request: &HttpRequest) -> HttpApiError {
    HttpApiError::new(
        403,
        ErrorCode::PermissionDenied,
        "operator administration binding was not accepted",
        request.request_id(),
        RetryAdvice::Never,
    )
}

fn handle_resolution(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = now_unix_seconds(request.request_id())?;
    block_on(async {
        let verified = graph
            .internal_authenticator()
            .verify(InternalRoute::FunctionSecretsResolve, request, now)
            .map_err(|error| error.to_http_error(request.request_id()))?;
        let input: FunctionSecretResolutionRequest = serde_json::from_slice(request.body())
            .map_err(|_| invalid(request, "function resolution input is invalid"))?;
        let canonical_ref = format!(
            "{}--{}",
            verified.tenant.project_id(),
            verified.tenant.environment_id()
        );
        if input.project_ref != canonical_ref {
            return Err(HttpApiError::new(
                403,
                ErrorCode::PermissionDenied,
                "function project reference does not match the signed tenant",
                request.request_id(),
                RetryAdvice::Never,
            ));
        }
        let guard = graph.internal_replay_guard(&verified.tenant).map_err(|_| {
            unavailable(
                request,
                "function resolution replay protection is unavailable",
            )
        })?;
        if guard
            .claim(&verified, now)
            .await
            .map_err(|error| map_guard_error(request, error))?
            == GuardDecision::Duplicate
        {
            return Err(conflict(
                request,
                "internal function request was already used",
            ));
        }
        let resolved = graph
            .function_resolution()
            .resolve(&verified.tenant, &input.function_name, &input.region)
            .await
            .map_err(|error| map_resolution_error(request, error))?;
        let project = graph
            .project_store()
            .get_project(verified.tenant.project_id())
            .await
            .map_err(|_| unavailable(request, "function organization resolution is unavailable"))?
            .ok_or_else(|| {
                unavailable(request, "function organization resolution is unavailable")
            })?;
        let custom_domains = graph
            .custom_domain_service()
            .verified_hostnames(&verified.tenant)
            .await
            .map_err(|_| {
                unavailable(request, "function custom domain resolution is unavailable")
            })?;
        let response = FunctionSecretResolutionResponse {
            organization_id: project.organization_id().as_str().to_owned(),
            function_name: resolved.function_name.as_str().to_owned(),
            version: resolved.version,
            bundle_digest: resolved.bundle_digest,
            entrypoint: resolved.entrypoint,
            runtime_version: resolved.runtime_version,
            verify_jwt: resolved.configuration.verify_jwt,
            selected_regions: resolved.configuration.regions,
            request_limit_bytes: resolved.configuration.limits.request_bytes,
            response_limit_bytes: resolved.configuration.limits.response_bytes,
            secrets: resolved
                .secrets
                .into_iter()
                .map(|secret| ResolvedFunctionSecret {
                    name: secret.name.as_str().to_owned(),
                    version: secret.version,
                    value: secret.expose_to_runtime().to_owned(),
                })
                .collect(),
            custom_domains,
        };
        HttpResponse::json(200, &response)
            .map_err(|_| unavailable(request, "function resolution response is unavailable"))
    })
}

fn now_unix_seconds(request_id: &str) -> Result<u64, HttpApiError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| {
            HttpApiError::new(
                500,
                ErrorCode::Internal,
                "service clock is unavailable",
                request_id,
                RetryAdvice::AfterDelay { after_ms: 1_000 },
            )
        })
}

fn map_resolution_error(request: &HttpRequest, error: FunctionResolutionError) -> HttpApiError {
    match error {
        FunctionResolutionError::InvalidRequest
        | FunctionResolutionError::Function(_)
        | FunctionResolutionError::Credential(_)
        | FunctionResolutionError::Keyspace(_) => {
            invalid(request, "function resolution input is invalid")
        }
        FunctionResolutionError::NotFound | FunctionResolutionError::Inactive => HttpApiError::new(
            404,
            ErrorCode::NotFound,
            "function deployment was not found",
            request.request_id(),
            RetryAdvice::Never,
        ),
        FunctionResolutionError::RegionUnavailable => HttpApiError::new(
            503,
            ErrorCode::Unavailable,
            "function is not available in this region",
            request.request_id(),
            RetryAdvice::AfterDelay { after_ms: 1_000 },
        ),
        _ => unavailable(request, "function resolution is unavailable"),
    }
}

fn map_guard_error(request: &HttpRequest, error: RocksInternalReplayGuardError) -> HttpApiError {
    match error {
        RocksInternalReplayGuardError::IdempotencyMismatch => {
            conflict(request, "function resolution idempotency key conflicts")
        }
        RocksInternalReplayGuardError::Replay => {
            conflict(request, "internal function request nonce was already used")
        }
        _ => unavailable(
            request,
            "function resolution replay protection is unavailable",
        ),
    }
}

fn invalid(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        400,
        ErrorCode::InvalidRequest,
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

fn unavailable(request: &HttpRequest, message: &'static str) -> HttpApiError {
    HttpApiError::new(
        503,
        ErrorCode::Unavailable,
        message,
        request.request_id(),
        RetryAdvice::AfterDelay { after_ms: 1_000 },
    )
}
