use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures::executor::block_on;
use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_audit::{
    ActorIdentity, AttributeValue, AuditCategory, AuditEvent, AuditOutcome, AuditStore,
    CorrelationId, RequestId, ResourceReference, SafeAttributes, SignalContext, SignalScope,
    TelemetryRedactor,
};
use mako_control_plane::{
    ActivityRecord, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    OperatorAuditEvent, OperatorAuditOutcome, OperatorAuditSink, OperatorAuthenticationAuditAction,
    OperatorAuthenticationAuditEvent, OperatorAuthenticationAuditOutcome,
    OperatorAuthenticationAuditSink, team_audit_tenant,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, WriteBatch,
};

thread_local! {
    static ACTING_TOKEN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs a request made with an automation token so that everything it audits
/// names the token as well as the developer it acts for. A request is handled
/// start to finish on one thread (`with_developer` blocks on it), so the
/// marker covers exactly that request and is cleared when it ends.
pub(crate) fn acting_as_token<T>(token_id: &str, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<String>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let previous = self.0.take();
            ACTING_TOKEN.with(|cell| *cell.borrow_mut() = previous);
        }
    }
    let previous = ACTING_TOKEN.with(|cell| cell.replace(Some(token_id.to_owned())));
    let _reset = Reset(previous);
    run()
}

fn acting_token() -> Option<String> {
    ACTING_TOKEN.with(|cell| cell.borrow().clone())
}

/// Synchronous domain audit facade backed by the append-only control-plane store.
/// Domain APIs cannot accidentally proceed while an asynchronous audit write is
/// still outstanding, and readiness fails closed after any persistence failure.
pub(crate) struct PersistentControlAudit {
    store: AuditStore,
    redactor: TelemetryRedactor,
    projection_adapter: Arc<dyn KvAdapter>,
    healthy: AtomicBool,
}

impl PersistentControlAudit {
    pub(crate) fn new(
        store: AuditStore,
        redactor: TelemetryRedactor,
        projection_adapter: Arc<dyn KvAdapter>,
    ) -> Self {
        Self {
            store,
            redactor,
            projection_adapter,
            healthy: AtomicBool::new(true),
        }
    }

    pub(crate) fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    fn append(&self, tenant: &TenantScope, category: AuditCategory, event: AuditEvent) -> bool {
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| block_on(self.store.append(tenant, category, event, &self.redactor)))
                .join()
        });
        let succeeded = matches!(result, Ok(Ok(_)));
        if !succeeded {
            self.healthy.store(false, Ordering::Release);
        }
        succeeded
    }

    #[allow(clippy::too_many_arguments)]
    fn project_activity(
        &self,
        source_event_id: &str,
        actor_id: &str,
        action: &str,
        target: &str,
        project_id: Option<ProjectId>,
        outcome: &str,
        at_unix_seconds: u64,
    ) {
        let digest = event_digest(&[
            source_event_id,
            actor_id,
            action,
            target,
            outcome,
            &at_unix_seconds.to_string(),
        ]);
        let id = format!("evt_{}", &digest[..24]);
        let mut record = ActivityRecord {
            id: id.clone(),
            source_event_id: source_event_id.to_owned(),
            actor_id: actor_id.to_owned(),
            action: action.to_owned(),
            target: target.to_owned(),
            project_id,
            case_reference: None,
            outcome: outcome.to_owned(),
            at_unix_seconds,
            integrity: String::new(),
        };
        record.integrity = record.computed_integrity();
        let Ok(key) = ControlKeyspace::operator_activity_key(
            at_unix_seconds.max(1).saturating_mul(1_000),
            &id,
        ) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let Ok(value) = serde_json::to_vec(&record) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let mut batch = WriteBatch::new();
        batch.put(&key, value);
        let write = AtomicWrite {
            conditions: vec![KeyCondition::Missing { key }],
            batch,
            durability: Durability::Sync,
        };
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| block_on(self.projection_adapter.compare_and_write(write)))
                .join()
        });
        if !matches!(
            result,
            Ok(Ok(
                CompareAndWriteResult::Applied | CompareAndWriteResult::Conflict { .. }
            ))
        ) {
            self.healthy.store(false, Ordering::Release);
        }
    }
}

impl ControlAuditSink for PersistentControlAudit {
    fn record(&self, event: ControlAuditEvent) {
        let digest = event_digest(&[
            event.organization_id.as_str(),
            event.actor_id.as_str(),
            event.action.as_str(),
            &event.target,
            &event.at_unix_seconds.to_string(),
        ]);
        let Some(tenant) = tenant_from_control_target(&event.target)
            .or_else(|| team_audit_tenant(&event.organization_id))
        else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let Some(mut audit) = build_event(
            &tenant,
            event.organization_id.as_str(),
            ActorIdentity::Developer {
                actor_id: event.actor_id.as_str().to_owned(),
            },
            "control_resource",
            &digest,
            event.action.as_str(),
            match event.outcome {
                ControlAuditOutcome::Allowed => AuditOutcome::Allowed,
                ControlAuditOutcome::Denied => AuditOutcome::Denied,
            },
            event.at_unix_seconds,
        ) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        // The stored resource is a digest; the target itself (the member,
        // invitation, or token acted on) is kept alongside when it is a plain
        // identifier, so the team's activity can say what was changed.
        audit.details = event_details(&event.target, acting_token().as_deref());
        let source_event_id = audit.event_id.clone();
        if self.append(&tenant, AuditCategory::Control, audit) {
            self.project_activity(
                &source_event_id,
                event.actor_id.as_str(),
                event.action.as_str(),
                &event.target,
                Some(tenant.project_id().clone()),
                match event.outcome {
                    ControlAuditOutcome::Allowed => "allowed",
                    ControlAuditOutcome::Denied => "denied",
                },
                event.at_unix_seconds,
            );
        }
    }
}

impl OperatorAuditSink for PersistentControlAudit {
    fn record(&self, event: OperatorAuditEvent) {
        let digest = event_digest(&[
            event.operator_id.as_str(),
            event.action.as_str(),
            event.project_id.as_str(),
            &event.target,
            &event.at_unix_seconds.to_string(),
        ]);
        let Some(tenant) = project_audit_tenant(event.project_id.clone(), "operatoraudit") else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let Some(audit) = build_event(
            &tenant,
            event.project_id.as_str(),
            ActorIdentity::Operator {
                actor_id: event.operator_id.as_str().to_owned(),
                support_session_id: None,
            },
            "operator_resource",
            &digest,
            event.action.as_str(),
            match event.outcome {
                OperatorAuditOutcome::Allowed => AuditOutcome::Allowed,
                OperatorAuditOutcome::Denied => AuditOutcome::Denied,
            },
            event.at_unix_seconds,
        ) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let source_event_id = audit.event_id.clone();
        if self.append(&tenant, AuditCategory::Operator, audit) {
            self.project_activity(
                &source_event_id,
                event.operator_id.as_str(),
                event.action.as_str(),
                &event.target,
                Some(event.project_id),
                match event.outcome {
                    OperatorAuditOutcome::Allowed => "allowed",
                    OperatorAuditOutcome::Denied => "denied",
                },
                event.at_unix_seconds,
            );
        }
    }
}

impl OperatorAuthenticationAuditSink for PersistentControlAudit {
    fn record(&self, event: OperatorAuthenticationAuditEvent) {
        let operator_id = event
            .operator_id
            .as_ref()
            .map_or("opr_anonymousauth", mako_control_plane::OperatorId::as_str);
        let action = match event.action {
            OperatorAuthenticationAuditAction::SignIn => "operator_password_signin",
            OperatorAuthenticationAuditAction::SignOut => "operator_password_signout",
            OperatorAuthenticationAuditAction::PasswordStepUp => "operator_password_step_up",
            OperatorAuthenticationAuditAction::SessionRevocation => {
                "operator_password_session_revocation"
            }
            OperatorAuthenticationAuditAction::EntitlementChange => "operator_entitlement_change",
            OperatorAuthenticationAuditAction::DeveloperRoleRepair => "developer_role_repair",
        };
        let digest = event_digest(&[
            operator_id,
            action,
            event.classification,
            &event.at_unix_seconds.to_string(),
        ]);
        let Some(tenant) = synthetic_tenant(operator_id, "operatorauth") else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let Some(audit) = build_event(
            &tenant,
            operator_id,
            ActorIdentity::Operator {
                actor_id: operator_id.to_owned(),
                support_session_id: None,
            },
            "operator_authentication",
            &digest,
            action,
            match event.outcome {
                OperatorAuthenticationAuditOutcome::Allowed => AuditOutcome::Allowed,
                OperatorAuthenticationAuditOutcome::Denied
                | OperatorAuthenticationAuditOutcome::Throttled => AuditOutcome::Denied,
            },
            event.at_unix_seconds,
        ) else {
            self.healthy.store(false, Ordering::Release);
            return;
        };
        let source_event_id = audit.event_id.clone();
        if self.append(&tenant, AuditCategory::Operator, audit) {
            self.project_activity(
                &source_event_id,
                operator_id,
                action,
                event.classification,
                None,
                match event.outcome {
                    OperatorAuthenticationAuditOutcome::Allowed => "allowed",
                    OperatorAuthenticationAuditOutcome::Denied => "denied",
                    OperatorAuthenticationAuditOutcome::Throttled => "throttled",
                },
                event.at_unix_seconds,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_event(
    tenant: &TenantScope,
    organization_id: &str,
    actor: ActorIdentity,
    resource_kind: &str,
    digest: &str,
    action: &str,
    outcome: AuditOutcome,
    at_unix_seconds: u64,
) -> Option<AuditEvent> {
    let context = SignalContext::new(
        SignalScope::Tenant {
            tenant: tenant.clone(),
            organization_id: Some(organization_id.to_owned()),
        },
        actor,
        ResourceReference::new(resource_kind, format!("res_{}", &digest[..32])).ok()?,
        RequestId::parse(format!("req_{}", &digest[..32])).ok()?,
        CorrelationId::parse(format!("cor_{}", &digest[..32])).ok()?,
        None,
    )
    .ok()?;
    Some(AuditEvent {
        context,
        event_id: format!("evt_{}", &digest[16..48]),
        occurred_at_unix_milliseconds: at_unix_seconds.max(1).saturating_mul(1_000),
        action: action.to_owned(),
        outcome,
        reason_code: match outcome {
            AuditOutcome::Allowed => "allowed",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Failed => "failed",
        }
        .to_owned(),
        details: SafeAttributes::default(),
    })
}

fn synthetic_tenant(scope: &str, environment: &str) -> Option<TenantScope> {
    let scope_digest = blake3::hash(scope.as_bytes()).to_hex().to_string();
    Some(TenantScope::new(
        ProjectId::parse(format!("prj_{}", &scope_digest[..16])).ok()?,
        EnvironmentId::parse(format!("env_{environment}")).ok()?,
    ))
}

fn tenant_from_control_target(target: &str) -> Option<TenantScope> {
    let mut segments = target.split('/');
    let project = ProjectId::parse(segments.next()?.to_owned()).ok()?;
    let environment = EnvironmentId::parse(segments.next()?.to_owned()).ok()?;
    Some(TenantScope::new(project, environment))
}

fn project_audit_tenant(project: ProjectId, environment: &str) -> Option<TenantScope> {
    Some(TenantScope::new(
        project,
        EnvironmentId::parse(format!("env_{environment}")).ok()?,
    ))
}

/// `{"target": ...}` for an identifier-shaped target; nothing for anything
/// else, an email address above all, which a refused invitation records.
fn plain_identifier(value: &str) -> bool {
    (1..=200).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/' | b'.'))
}

/// `target=` names what was acted on when it is a plain identifier, and
/// `via=` the automation token a request was made with: without it a CI
/// deploy read as the token's creator acting in person.
fn event_details(target: &str, via: Option<&str>) -> SafeAttributes {
    let mut pairs = Vec::with_capacity(2);
    if plain_identifier(target) {
        pairs.push(("target".to_owned(), AttributeValue::Text(target.to_owned())));
    }
    if let Some(token) = via.filter(|token| plain_identifier(token)) {
        pairs.push(("via".to_owned(), AttributeValue::Text(token.to_owned())));
    }
    SafeAttributes::try_from_iter(pairs).unwrap_or_default()
}

fn event_digest(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("mako/control-plane/audit-event-id/v1");
    for part in parts {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

impl std::fmt::Debug for PersistentControlAudit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistentControlAudit")
            .field("healthy", &self.healthy())
            .finish_non_exhaustive()
    }
}

pub(crate) type SharedControlAudit = Arc<PersistentControlAudit>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_events_stay_under_the_tenant_they_were_always_stored_under() {
        // Events written before the tenant had a shared definition must stay
        // readable as the team's activity.
        let team = mako_control_plane::OrganizationId::parse("org_websiteteam").expect("team");
        assert_eq!(
            team_audit_tenant(&team),
            synthetic_tenant(team.as_str(), "controlaudit")
        );
    }

    #[test]
    fn identifier_targets_are_kept_and_email_addresses_are_not() {
        let target_details = |target: &str| event_details(target, None);
        let kept = target_details("dev_jordanteam");
        assert_eq!(
            kept.get("target"),
            Some(&AttributeValue::Text("dev_jordanteam".to_owned()))
        );
        assert!(
            target_details("prj_example/env_example/Audit")
                .get("target")
                .is_some()
        );
        assert!(target_details("jordan@example.test").is_empty());
        assert!(target_details("").is_empty());
        assert!(target_details(&"a".repeat(201)).is_empty());
    }

    #[test]
    fn a_request_made_with_a_token_names_it_in_what_it_audits() {
        assert_eq!(acting_token(), None);
        let seen = acting_as_token("atm_ci000001", || {
            event_details(
                "prj_example/env_example/todo-stats",
                acting_token().as_deref(),
            )
        });
        assert_eq!(
            seen.get("via"),
            Some(&AttributeValue::Text("atm_ci000001".to_owned()))
        );
        assert!(seen.get("target").is_some());
        // The marker ends with the request, even one that panics.
        assert_eq!(acting_token(), None);
        let _ = std::panic::catch_unwind(|| {
            acting_as_token("atm_ci000002", || panic!("request failed"))
        });
        assert_eq!(acting_token(), None);
        assert!(event_details("dev_jordanteam", None).get("via").is_none());
    }

    #[test]
    fn tenant_scoped_control_targets_retain_the_real_project_environment() {
        let tenant =
            tenant_from_control_target("prj_example00/env_example00/functions").expect("tenant");
        assert_eq!(tenant.project_id().as_str(), "prj_example00");
        assert_eq!(tenant.environment_id().as_str(), "env_example00");
        assert!(tenant_from_control_target("org_example00/invitations").is_none());
    }
}
