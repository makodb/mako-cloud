use std::{collections::BTreeMap, sync::Arc};

use mako_api::{CollectionId, CollectionScope, TenantScope};
use mako_control_plane::{
    ActivePolicyView, DeveloperPrincipal, NewPolicyDraft, PolicyAdminError, PolicyExampleResult,
    PolicyValidationView,
};
use mako_internal_rpc::IdentityAdminOperation;
use mako_policy::{
    DiagnosticSeverity, DocumentOperation, PolicyEffect, PolicyEvaluationContext, PolicyRule,
    PolicyRuleId, PolicySet, PolicyStoreError, SafeRequestMetadata, SubjectId, VerifiedEmail,
    VerifiedIdentity, VerifiedRole,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json as json_value};

use crate::{
    ControlPlaneGraph, explorer_invalidation,
    identity_admin_http::administer,
    management_http::{
        conflict, forbidden, invalid, json, no_payload, no_query, not_found, parse_json,
        require_idempotency, require_json, unavailable, with_developer,
    },
};

pub(crate) fn add_policy_routes(
    router: &mut HttpRouter,
    graph: Arc<ControlPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    for (method, path, handler) in [
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies",
            handle_active_policy as Handler,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies",
            handle_create_draft,
        ),
        (
            HttpMethod::Get,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}",
            handle_get_policy,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/validate",
            handle_validate_policy,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/test",
            handle_test_policy,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/activate",
            handle_activate_policy,
        ),
        (
            HttpMethod::Post,
            "/v1/projects/{projectId}/environments/{environmentId}/collections/{collectionId}/policies/{policyVersion}/actions/rollback",
            handle_rollback_policy,
        ),
    ] {
        let graph = Arc::clone(&graph);
        router.add_route(method, path, move |request| handler(&graph, &request))?;
    }
    Ok(())
}

type Handler = fn(&Arc<ControlPlaneGraph>, &HttpRequest) -> Result<HttpResponse, HttpApiError>;

fn handle_active_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    with_developer(graph, request, |actor, now| async move {
        let view = graph
            .policy_service()
            .active_policy(&actor, &tenant, &collection_id, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        json(request, 200, &active_policy_wire(&view))
    })
}

fn handle_create_draft(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    require_idempotency(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    let body: CreatePolicyDraftWire = parse_json(request)?;
    let rules = body
        .rules
        .iter()
        .map(policy_rule)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid(request, "policy rules are invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let input = NewPolicyDraft {
            tenant: tenant.clone(),
            collection_id: collection_id.clone(),
            version: body.version,
            rules: rules.clone(),
            now_unix_seconds: now,
        };
        let policy = match graph.policy_service().create_draft(&actor, input).await {
            Ok(policy) => policy,
            Err(PolicyAdminError::Store(PolicyStoreError::VersionAlreadyExists)) => {
                let existing = graph
                    .policy_service()
                    .get_policy(&actor, &tenant, &collection_id, body.version, now)
                    .await
                    .map_err(|error| policy_error(request, error))?;
                if existing.rules() == rules {
                    existing
                } else {
                    return Err(conflict(request, "policy idempotency conflict"));
                }
            }
            Err(error) => return Err(policy_error(request, error)),
        };
        json(request, 201, &policy_wire(&policy))
    })
}

fn handle_get_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    let version = policy_version(request)?;
    with_developer(graph, request, |actor, now| async move {
        let policy = graph
            .policy_service()
            .get_policy(&actor, &tenant, &collection_id, version, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        json(request, 200, &policy_wire(&policy))
    })
}

fn handle_validate_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    let version = policy_version(request)?;
    with_developer(graph, request, |actor, now| async move {
        let view = graph
            .policy_service()
            .validate(&actor, &tenant, &collection_id, version, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        json(request, 200, &validation_wire(&view))
    })
}

fn handle_test_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    no_query(request)?;
    require_json(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    let version = policy_version(request)?;
    let body: PolicyExamplesWire = parse_json(request)?;
    let scope = CollectionScope::new(tenant.clone(), collection_id.clone());
    let examples = body
        .examples
        .into_iter()
        .map(|example| policy_example(scope.clone(), example))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid(request, "policy examples are invalid"))?;
    with_developer(graph, request, |actor, now| async move {
        let results = graph
            .policy_service()
            .test_examples(&actor, &tenant, &collection_id, version, &examples, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        let results = results.iter().map(example_result_wire).collect();
        json(request, 200, &PolicyResultsWire { results })
    })
}

fn handle_activate_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    policy_lifecycle(graph, request, false)
}

fn handle_rollback_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    policy_lifecycle(graph, request, true)
}

/// Install the active policy in the data plane that enforces it.
async fn propagate_policy(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    actor: &DeveloperPrincipal,
    tenant: &TenantScope,
    collection_id: &CollectionId,
    policy: &PolicySet,
) -> Result<(), HttpApiError> {
    let encoded = policy
        .encode()
        .map_err(|_| unavailable(request, "policy could not be encoded"))?;
    let value: Value = serde_json::from_slice(&encoded)
        .map_err(|_| unavailable(request, "policy could not be encoded"))?;
    let _: Value = administer(
        graph,
        request,
        actor,
        tenant,
        IdentityAdminOperation::InstallPolicy,
        json_value!({
            "collectionId": collection_id.as_str(),
            "version": policy.version().get(),
            "policy": value,
        }),
        true,
    )
    .await?;
    Ok(())
}

fn policy_lifecycle(
    graph: &Arc<ControlPlaneGraph>,
    request: &HttpRequest,
    rollback: bool,
) -> Result<HttpResponse, HttpApiError> {
    no_payload(request)?;
    require_idempotency(request)?;
    let (tenant, collection_id) = collection_scope(request)?;
    let version = policy_version(request)?;
    with_developer(graph, request, |actor, now| async move {
        let current = graph
            .policy_service()
            .active_policy(&actor, &tenant, &collection_id, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        // Document authorization is enforced by the data plane against its own
        // store. A policy that is only recorded here leaves replication pull
        // returning an empty batch and every push denied, so the version is not
        // enforced until the data plane holds it.
        //
        // The data plane is granted the version before it is committed here,
        // deliberately. Committing first would leave every management surface
        // reporting a version active that the data plane never received and is
        // not enforcing — a developer would read their new rules as live while
        // traffic was still evaluated against the old ones. Failing before the
        // local commit keeps the previously granted version in effect on both
        // sides. Both writes are idempotent, so a retry converges.
        let target = graph
            .policy_service()
            .get_policy(&actor, &tenant, &collection_id, version, now)
            .await
            .map_err(|error| policy_error(request, error))?;
        propagate_policy(graph, request, &actor, &tenant, &collection_id, &target).await?;
        let view = if current
            .policy
            .as_ref()
            .is_some_and(|policy| policy.version().get() == version)
        {
            current
        } else if rollback {
            graph
                .policy_service()
                .rollback(&actor, &tenant, &collection_id, version, now)
                .await
                .map_err(|error| policy_error(request, error))?
        } else {
            graph
                .policy_service()
                .activate(&actor, &tenant, &collection_id, version, now)
                .await
                .map_err(|error| policy_error(request, error))?
        };
        explorer_invalidation::advance_tenant(
            graph,
            request,
            &tenant,
            actor.identity_id().as_str(),
            "policy-lifecycle",
        )?;
        json(request, 200, &active_policy_wire(&view))
    })
}

fn collection_scope(request: &HttpRequest) -> Result<(TenantScope, CollectionId), HttpApiError> {
    let tenant = TenantScope::require(
        request.path_parameter("projectId"),
        request.path_parameter("environmentId"),
    )
    .map_err(|_| invalid(request, "tenant path is invalid"))?;
    let collection_id =
        CollectionId::parse(request.path_parameter("collectionId").unwrap_or_default())
            .map_err(|_| invalid(request, "collection path is invalid"))?;
    Ok((tenant, collection_id))
}

fn policy_version(request: &HttpRequest) -> Result<u64, HttpApiError> {
    request
        .path_parameter("policyVersion")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(request, "policy version path is invalid"))
}

fn policy_rule(wire: &PolicyRuleInputWire) -> Result<PolicyRule, mako_policy::PolicyModelError> {
    PolicyRule::new(
        PolicyRuleId::parse(wire.id.clone())?,
        wire.effect,
        wire.operations.iter().copied(),
        wire.expression.clone(),
    )
}

fn policy_example(
    scope: CollectionScope,
    wire: PolicyExampleWire,
) -> Result<PolicyEvaluationContext, mako_policy::PolicyContextError> {
    let identity = match wire.identity.user_id {
        Some(user_id) => VerifiedIdentity::user(
            SubjectId::parse(user_id)?,
            VerifiedRole::parse(wire.identity.role)?,
            wire.identity
                .email
                .map(|address| VerifiedEmail::parse(address, wire.identity.email_verified))
                .transpose()?,
            wire.identity.trusted_claims,
        )?,
        None if wire.identity.role == "anonymous" => {
            VerifiedIdentity::anonymous(wire.identity.trusted_claims)?
        }
        None => {
            return Err(mako_policy::PolicyContextError::InvalidToken(
                "identity.role",
            ));
        }
    };
    PolicyEvaluationContext::new(
        scope,
        wire.operation,
        identity,
        wire.old_document,
        wire.new_document,
        SafeRequestMetadata::new(wire.request_metadata)?,
    )
}

fn policy_error(request: &HttpRequest, error: PolicyAdminError) -> HttpApiError {
    match error {
        PolicyAdminError::NotFound | PolicyAdminError::Store(PolicyStoreError::VersionNotFound) => {
            not_found(request, "policy resource was not found")
        }
        PolicyAdminError::Forbidden => forbidden(request, "policy action is forbidden"),
        PolicyAdminError::InvalidExamples
        | PolicyAdminError::DocumentScope(_)
        | PolicyAdminError::Collection(_)
        | PolicyAdminError::Model(_)
        | PolicyAdminError::Context(_) => invalid(request, "policy input is invalid"),
        PolicyAdminError::Store(
            PolicyStoreError::VersionAlreadyExists
            | PolicyStoreError::ConcurrentLifecycleChange
            | PolicyStoreError::ValidationFailed(_)
            | PolicyStoreError::NewVersionMustBeDraft,
        ) => conflict(request, "policy lifecycle or version conflict"),
        _ => unavailable(request, "policy administration is unavailable"),
    }
}

fn policy_wire(policy: &PolicySet) -> PolicySetWire {
    PolicySetWire {
        version: policy.version().get(),
        state: policy.state(),
        rules: policy
            .rules()
            .iter()
            .map(|rule| PolicyRuleOutputWire {
                id: rule.id().as_str().to_owned(),
                effect: rule.effect(),
                operations: rule.operations().iter().copied().collect(),
                expression: rule.expression().to_owned(),
            })
            .collect(),
        diagnostics: policy
            .diagnostics()
            .iter()
            .map(|diagnostic| PolicyDiagnosticWire {
                severity: diagnostic.severity(),
                code: diagnostic.code().to_owned(),
                message: diagnostic.message().to_owned(),
                span: diagnostic.span().map(|span| SourceSpanWire {
                    line: span.line(),
                    column: span.column(),
                    length: span.length(),
                }),
            })
            .collect(),
    }
}

fn active_policy_wire(view: &ActivePolicyView) -> ActivePolicyWire {
    ActivePolicyWire {
        default_deny: view.default_deny,
        authorization_epoch: view.authorization_epoch,
        policy: view.policy.as_ref().map(policy_wire),
    }
}

fn validation_wire(view: &PolicyValidationView) -> PolicyValidationWire {
    PolicyValidationWire {
        valid: view.valid,
        policy: policy_wire(&view.policy),
    }
}

fn example_result_wire(result: &PolicyExampleResult) -> PolicyExampleResultWire {
    PolicyExampleResultWire {
        allowed: result.allowed,
        code: result.code.clone(),
        matched_rule_ids: result.matched_rule_ids.clone(),
        evaluated_rules: result.evaluated_rules,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePolicyDraftWire {
    version: u64,
    rules: Vec<PolicyRuleInputWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyRuleInputWire {
    id: String,
    effect: PolicyEffect,
    operations: Vec<DocumentOperation>,
    expression: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyExamplesWire {
    examples: Vec<PolicyExampleWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyExampleWire {
    operation: DocumentOperation,
    identity: PolicyIdentityWire,
    old_document: Option<Value>,
    new_document: Option<Value>,
    #[serde(default)]
    request_metadata: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyIdentityWire {
    user_id: Option<String>,
    role: String,
    /// What `identity.email` and `identity.email_verified` read for the
    /// simulated caller. A simulation that omitted them could not exercise a
    /// rule that scopes a document to an address.
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    trusted_claims: Value,
}

#[derive(Serialize)]
struct PolicySetWire {
    version: u64,
    state: mako_policy::PolicyState,
    rules: Vec<PolicyRuleOutputWire>,
    diagnostics: Vec<PolicyDiagnosticWire>,
}

#[derive(Serialize)]
struct PolicyRuleOutputWire {
    id: String,
    effect: PolicyEffect,
    operations: Vec<DocumentOperation>,
    expression: String,
}

#[derive(Serialize)]
struct PolicyDiagnosticWire {
    severity: DiagnosticSeverity,
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<SourceSpanWire>,
}

#[derive(Serialize)]
struct SourceSpanWire {
    line: u32,
    column: u32,
    length: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivePolicyWire {
    default_deny: bool,
    authorization_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<PolicySetWire>,
}

#[derive(Serialize)]
struct PolicyValidationWire {
    valid: bool,
    policy: PolicySetWire,
}

#[derive(Serialize)]
struct PolicyResultsWire {
    results: Vec<PolicyExampleResultWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyExampleResultWire {
    allowed: bool,
    code: String,
    matched_rule_ids: Vec<String>,
    evaluated_rules: usize,
}

#[cfg(test)]
mod tests {
    use mako_api::{EnvironmentId, ProjectId};
    use serde_json::json;

    use super::*;

    #[test]
    fn policy_examples_validate_identity_and_document_states() {
        let scope = CollectionScope::new(
            TenantScope::new(
                ProjectId::parse("prj_example00").expect("project"),
                EnvironmentId::parse("env_example00").expect("environment"),
            ),
            CollectionId::parse("todos").expect("collection"),
        );
        let valid = policy_example(
            scope.clone(),
            PolicyExampleWire {
                operation: DocumentOperation::Create,
                identity: PolicyIdentityWire {
                    user_id: Some("user-1".to_owned()),
                    role: "member".to_owned(),
                    email: None,
                    email_verified: false,
                    trusted_claims: json!({"team": "blue"}),
                },
                old_document: None,
                new_document: Some(json!({"id": "todo-1"})),
                request_metadata: BTreeMap::new(),
            },
        );
        assert!(valid.is_ok());

        let invalid = policy_example(
            scope,
            PolicyExampleWire {
                operation: DocumentOperation::Read,
                identity: PolicyIdentityWire {
                    user_id: None,
                    role: "anonymous".to_owned(),
                    email: None,
                    email_verified: false,
                    trusted_claims: json!({}),
                },
                old_document: None,
                new_document: None,
                request_metadata: BTreeMap::new(),
            },
        );
        assert!(invalid.is_err());
    }

    #[test]
    fn result_wire_uses_public_camel_case_fields() {
        let encoded = serde_json::to_value(PolicyExampleResultWire {
            allowed: true,
            code: "policy_allowed".to_owned(),
            matched_rule_ids: vec!["owner".to_owned()],
            evaluated_rules: 1,
        })
        .expect("result wire");
        assert_eq!(encoded["matchedRuleIds"], json!(["owner"]));
        assert_eq!(encoded["evaluatedRules"], 1);
    }
}
