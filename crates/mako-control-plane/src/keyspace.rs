use std::{error::Error, fmt};

use mako_api::{EnvironmentId, ProjectId};
use mako_storage::{KeyCodecError, KeyRange, TenantKeyspace};

use crate::{AutomationTokenId, DeveloperIdentityId, InvitationId, OrganizationId};

const DEVELOPERS: &[u8] = b"control/developers";
const DEVELOPER_EMAILS: &[u8] = b"control/developer-email-index";
const AUTHENTICATION_IDENTITIES: &[u8] = b"control/authentication-identities/v1";
const DEVELOPER_ROLES: &[u8] = b"control/developer-roles/v1";
const DEVELOPER_TOKENS: &[u8] = b"control/developer-auth-tokens";
const DEVELOPER_IDENTITY_TOKENS: &[u8] = b"control/developer-auth-token-by-identity";
const DEVELOPER_SESSIONS: &[u8] = b"control/developer-refresh-sessions";
const DEVELOPER_DECISIONS: &[u8] = b"control/developer-review-decisions";
const DEVELOPER_RATE_LIMITS: &[u8] = b"control/developer-rate-limits";
const DEVELOPER_MAIL_OUTBOX: &[u8] = b"control/developer-mail-outbox";
const DEVELOPER_MIGRATIONS: &[u8] = b"control/developer-migrations";
const APPLICATION_MAIL_OUTBOX: &[u8] = b"control/application-mail-outbox/v1";
const OPERATOR_ENTITLEMENTS: &[u8] = b"control/operator-auth/v1/entitlements";
const OPERATOR_SESSIONS: &[u8] = b"control/operator-auth/v1/sessions";
const OPERATOR_ATTEMPTS: &[u8] = b"control/operator-auth/v1/attempts";
const OPERATOR_IDEMPOTENCY: &[u8] = b"control/operator-auth/v1/idempotency";
const OPERATOR_MIGRATIONS: &[u8] = b"control/operator-auth/migrations";
const ORGANIZATIONS: &[u8] = b"control/organizations";
const INVITATIONS: &[u8] = b"control/invitations";
const PROJECTS: &[u8] = b"control/projects";
const QUOTAS: &[u8] = b"control/quotas";
const AUTOMATION_TOKENS: &[u8] = b"control/automation-tokens";
const OPERATOR_INCIDENTS: &[u8] = b"control/operator-control-center/v1/incidents";
const OPERATOR_RECOVERY_JOBS: &[u8] = b"control/operator-control-center/v1/recovery-jobs";
const OPERATOR_ACTIVITY: &[u8] = b"control/operator-control-center/v1/activity";
const OPERATOR_ACTIVITY_EXPORTS: &[u8] = b"control/operator-control-center/v1/activity-exports";
const OPERATOR_PROJECTIONS: &[u8] = b"control/operator-control-center/v1/projections";
const OPERATOR_TENANT_SEARCH: &[u8] = b"control/operator-control-center/v1/tenant-search";
const DATA_JOBS: &[u8] = b"control/developer-workspace/v1/data-jobs";
const DEVELOPER_RESTORES: &[u8] = b"control/developer-workspace/v1/restore-requests";
const WEBHOOK_ENDPOINTS: &str = "control/webhooks/v1/endpoints";
const WEBHOOK_ENDPOINT_INDEX: &[u8] = b"control/webhooks/v1/endpoint-index";
const WEBHOOK_CURSORS: &str = "control/webhooks/v1/cursors";
const WEBHOOK_DELIVERIES: &str = "control/webhooks/v1/deliveries";
const WEBHOOK_PENDING: &str = "control/webhooks/v1/pending";
const WEBHOOK_DELIVERY_INDEX: &str = "control/webhooks/v1/delivery-index";
const WEBHOOK_DUE: &[u8] = b"control/webhooks/v1/due";

/// Encodes every management record below the storage adapter's reserved system namespace.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlKeyspace;

impl ControlKeyspace {
    pub fn developer_restore_key(
        project_id: &ProjectId,
        request_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!(
                "{}/{}",
                String::from_utf8_lossy(DEVELOPER_RESTORES),
                project_id.as_str()
            )
            .into_bytes(),
            request_id,
        )
    }

    pub fn developer_restores_range(
        project_id: &ProjectId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(
            format!(
                "{}/{}",
                String::from_utf8_lossy(DEVELOPER_RESTORES),
                project_id.as_str()
            )
            .into_bytes(),
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn all_data_jobs_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DATA_JOBS).map_err(ControlKeyspaceError)
    }

    pub fn data_job_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        job_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!(
                "{}/{}/{}",
                String::from_utf8_lossy(DATA_JOBS),
                project_id.as_str(),
                environment_id.as_str()
            )
            .into_bytes(),
            job_id,
        )
    }

    pub fn data_jobs_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(
            format!(
                "{}/{}/{}",
                String::from_utf8_lossy(DATA_JOBS),
                project_id.as_str(),
                environment_id.as_str()
            )
            .into_bytes(),
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn data_job_index_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        index: &str,
        value: &str,
        created_at_unix_seconds: u64,
        job_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!(
                "{}/{}/{}/index/{index}/{value}",
                String::from_utf8_lossy(DATA_JOBS),
                project_id.as_str(),
                environment_id.as_str()
            )
            .into_bytes(),
            &format!("{created_at_unix_seconds:020}/{job_id}"),
        )
    }

    pub fn authentication_identity_key(
        id: &DeveloperIdentityId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(AUTHENTICATION_IDENTITIES, id.as_str())
    }

    pub fn authentication_identities_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(AUTHENTICATION_IDENTITIES).map_err(ControlKeyspaceError)
    }

    pub fn developer_role_key(id: &DeveloperIdentityId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_ROLES, id.as_str())
    }

    pub fn developer_roles_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_ROLES).map_err(ControlKeyspaceError)
    }

    /// Legacy composite account key retained for rollback and migration reads.
    pub fn developer_key(id: &DeveloperIdentityId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPERS, id.as_str())
    }

    pub fn developers_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPERS).map_err(ControlKeyspaceError)
    }

    pub fn developer_email_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_EMAILS, digest)
    }

    pub fn developer_status_key(
        status: crate::DeveloperIdentityStatus,
        created_at_unix_seconds: u64,
        id: &DeveloperIdentityId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &developer_status_domain(status),
            &format!("{created_at_unix_seconds:020}/{}", id.as_str()),
        )
    }

    pub fn developer_status_range(
        status: crate::DeveloperIdentityStatus,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(developer_status_domain(status))
            .map_err(ControlKeyspaceError)
    }

    pub fn developer_token_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_TOKENS, digest)
    }

    pub fn developer_tokens_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_TOKENS).map_err(ControlKeyspaceError)
    }

    pub fn developer_identity_token_key(
        identity_id: &DeveloperIdentityId,
        purpose: crate::DeveloperAuthTokenPurpose,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        let purpose = match purpose {
            crate::DeveloperAuthTokenPurpose::EmailVerification => "email-verification",
            crate::DeveloperAuthTokenPurpose::PasswordRecovery => "password-recovery",
        };
        system_key(
            &format!(
                "{}/{}",
                String::from_utf8_lossy(DEVELOPER_IDENTITY_TOKENS),
                identity_id.as_str()
            )
            .into_bytes(),
            purpose,
        )
    }

    pub fn developer_session_key(session_id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_SESSIONS, session_id)
    }

    pub fn developer_sessions_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_SESSIONS).map_err(ControlKeyspaceError)
    }

    pub fn developer_decision_key(
        idempotency_digest: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_DECISIONS, idempotency_digest)
    }

    pub fn developer_decisions_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_DECISIONS).map_err(ControlKeyspaceError)
    }

    pub fn developer_rate_limit_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_RATE_LIMITS, digest)
    }

    pub fn developer_rate_limits_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_RATE_LIMITS).map_err(ControlKeyspaceError)
    }

    pub fn developer_mail_outbox_key(id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_MAIL_OUTBOX, id)
    }

    pub fn developer_mail_outbox_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(DEVELOPER_MAIL_OUTBOX).map_err(ControlKeyspaceError)
    }

    pub fn developer_migration_key(version: u32) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(DEVELOPER_MIGRATIONS, &format!("{version:010}"))
    }

    /// One application mail the data plane asked for, keyed by its intent id
    /// so a re-drained intent lands on the record it already produced.
    pub fn application_mail_outbox_key(id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(APPLICATION_MAIL_OUTBOX, id)
    }

    pub fn application_mail_outbox_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(APPLICATION_MAIL_OUTBOX).map_err(ControlKeyspaceError)
    }

    /// An environment's customized email template for one kind of mail; the
    /// absence of a record means the built-in default applies.
    pub fn email_template_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        kind: crate::EmailTemplateKind,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &email_template_domain(project_id, environment_id),
            kind.as_str(),
        )
    }

    pub fn email_templates_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(email_template_domain(project_id, environment_id))
            .map_err(ControlKeyspaceError)
    }

    pub fn operator_entitlement_key(
        identity_id: &DeveloperIdentityId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_ENTITLEMENTS, identity_id.as_str())
    }

    pub fn operator_entitlements_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_ENTITLEMENTS).map_err(ControlKeyspaceError)
    }

    pub fn operator_session_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_SESSIONS, digest)
    }

    pub fn operator_sessions_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_SESSIONS).map_err(ControlKeyspaceError)
    }

    pub fn operator_attempt_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_ATTEMPTS, digest)
    }

    pub fn operator_attempts_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_ATTEMPTS).map_err(ControlKeyspaceError)
    }

    pub fn operator_idempotency_key(digest: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_IDEMPOTENCY, digest)
    }

    pub fn operator_idempotency_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_IDEMPOTENCY).map_err(ControlKeyspaceError)
    }

    pub fn operator_migration_key(version: u32) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_MIGRATIONS, &format!("{version:010}"))
    }

    pub fn organization_key(id: &OrganizationId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(ORGANIZATIONS, id.as_str())
    }

    /// One operator-granted credit. Keyed by its id, so granting is
    /// exactly-once under a conditional create.
    pub fn organization_credit_key(
        id: &OrganizationId,
        credit_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        TenantKeyspace::system_key(credit_domain(id), credit_id).map_err(ControlKeyspaceError)
    }

    /// Every credit an organization holds.
    pub fn organization_credits_range(
        id: &OrganizationId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(credit_domain(id)).map_err(ControlKeyspaceError)
    }

    /// One closed billing period. Keyed by the period's start, so closing is
    /// exactly-once under a conditional create and listing walks the months
    /// in order.
    pub fn organization_invoice_key(
        id: &OrganizationId,
        period_start_unix_milliseconds: u64,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        TenantKeyspace::system_key(
            invoice_domain(id),
            format!("{period_start_unix_milliseconds:020}"),
        )
        .map_err(ControlKeyspaceError)
    }

    /// Every closed period an organization has.
    pub fn organization_invoices_range(
        id: &OrganizationId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(invoice_domain(id)).map_err(ControlKeyspaceError)
    }

    /// The operator-recorded exceptions to an organization's plan.
    pub fn organization_plan_exceptions_key(
        id: &OrganizationId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(b"control/organization-plan-exceptions", id.as_str())
    }

    pub fn developer_organization_key(
        developer_id: &DeveloperIdentityId,
        organization_id: &OrganizationId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &developer_organization_domain(developer_id),
            organization_id.as_str(),
        )
    }

    pub fn developer_organizations_range(
        developer_id: &DeveloperIdentityId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(developer_organization_domain(developer_id))
            .map_err(ControlKeyspaceError)
    }

    pub fn membership_key(
        organization_id: &OrganizationId,
        developer_id: &DeveloperIdentityId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(&membership_domain(organization_id), developer_id.as_str())
    }

    pub fn memberships_range(
        organization_id: &OrganizationId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(membership_domain(organization_id))
            .map_err(ControlKeyspaceError)
    }

    pub fn invitation_key(id: &InvitationId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(INVITATIONS, id.as_str())
    }

    pub fn project_key(id: &ProjectId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(PROJECTS, id.as_str())
    }

    pub fn projects_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(PROJECTS).map_err(ControlKeyspaceError)
    }

    pub fn organization_project_key(
        organization_id: &OrganizationId,
        project_id: &ProjectId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &organization_project_domain(organization_id),
            project_id.as_str(),
        )
    }

    pub fn organization_projects_range(
        organization_id: &OrganizationId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(organization_project_domain(organization_id))
            .map_err(ControlKeyspaceError)
    }

    pub fn environment_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(&environment_domain(project_id), environment_id.as_str())
    }

    pub fn environments_range(project_id: &ProjectId) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(environment_domain(project_id))
            .map_err(ControlKeyspaceError)
    }

    pub fn quota_key(project_id: &ProjectId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(QUOTAS, project_id.as_str())
    }

    pub fn operator_quota_override_key(
        project_id: &ProjectId,
        override_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!("control/operator/quota-overrides/{}", project_id.as_str()).into_bytes(),
            override_id,
        )
    }

    pub fn operator_quota_overrides_range(
        project_id: &ProjectId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(
            format!("control/operator/quota-overrides/{}", project_id.as_str()).into_bytes(),
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn operator_abuse_response_key(
        project_id: &ProjectId,
        response_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!("control/operator/abuse-responses/{}", project_id.as_str()).into_bytes(),
            response_id,
        )
    }

    pub fn operator_abuse_responses_range(
        project_id: &ProjectId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(
            format!("control/operator/abuse-responses/{}", project_id.as_str()).into_bytes(),
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn operator_support_session_key(
        project_id: &ProjectId,
        session_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!("control/operator/support-sessions/{}", project_id.as_str()).into_bytes(),
            session_id,
        )
    }

    pub fn operator_support_sessions_range(
        project_id: &ProjectId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(
            format!("control/operator/support-sessions/{}", project_id.as_str()).into_bytes(),
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn operator_all_support_sessions_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(b"control/operator/support-sessions")
            .map_err(ControlKeyspaceError)
    }

    pub fn operator_incident_key(id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_INCIDENTS, id)
    }

    pub fn operator_incidents_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_INCIDENTS).map_err(ControlKeyspaceError)
    }

    pub fn operator_recovery_job_key(id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_RECOVERY_JOBS, id)
    }

    pub fn operator_recovery_jobs_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_RECOVERY_JOBS).map_err(ControlKeyspaceError)
    }

    pub fn operator_activity_key(
        at_unix_milliseconds: u64,
        id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            OPERATOR_ACTIVITY,
            &format!("{at_unix_milliseconds:020}/{id}"),
        )
    }

    pub fn operator_activity_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_ACTIVITY).map_err(ControlKeyspaceError)
    }

    pub fn operator_activity_export_key(id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_ACTIVITY_EXPORTS, id)
    }

    pub fn operator_activity_exports_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_ACTIVITY_EXPORTS).map_err(ControlKeyspaceError)
    }

    pub fn operator_projection_key(name: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_PROJECTIONS, name)
    }

    pub fn operator_tenant_search_key(
        project_id: &ProjectId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(OPERATOR_TENANT_SEARCH, project_id.as_str())
    }

    pub fn operator_tenant_search_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(OPERATOR_TENANT_SEARCH).map_err(ControlKeyspaceError)
    }

    pub fn deletion_workflow_key(scope_key: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(b"control/deletion-workflows", scope_key)
    }

    pub fn automation_token_key(id: &AutomationTokenId) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(AUTOMATION_TOKENS, id.as_str())
    }

    pub fn signing_key_ring_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            b"control/signing-key-rings",
            &format!("{}/{}", project_id.as_str(), environment_id.as_str()),
        )
    }

    pub fn function_secret_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &crate::FunctionSecretName,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!(
                "control/function-secrets/{}/{}",
                project_id.as_str(),
                environment_id.as_str()
            )
            .into_bytes(),
            name.as_str(),
        )
    }

    pub fn function_secret_version_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &crate::FunctionSecretName,
        version: u64,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &format!(
                "control/function-secret-versions/{}/{}/{}",
                project_id.as_str(),
                environment_id.as_str(),
                name.as_str()
            )
            .into_bytes(),
            &format!("{version:020}"),
        )
    }

    pub fn function_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &crate::FunctionName,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(&function_domain(project_id, environment_id), name.as_str())
    }

    pub fn functions_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(function_domain(project_id, environment_id))
            .map_err(ControlKeyspaceError)
    }

    /// Where the function-log collector keeps its high-water mark for one
    /// function, so a restarted collector does not re-emit what it already
    /// shipped.
    pub fn function_log_state_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        TenantKeyspace::system_key(
            format!(
                "control/function-log-state/{}/{}",
                project_id.as_str(),
                environment_id.as_str()
            ),
            name,
        )
        .map_err(ControlKeyspaceError)
    }

    pub fn function_version_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &crate::FunctionName,
        version: u64,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &function_version_domain(project_id, environment_id, name),
            &format!("{version:020}"),
        )
    }

    pub fn function_versions_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        name: &crate::FunctionName,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(function_version_domain(
            project_id,
            environment_id,
            name,
        ))
        .map_err(ControlKeyspaceError)
    }

    pub fn function_bundle_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        digest: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(&function_bundle_domain(project_id, environment_id), digest)
    }

    pub fn function_bundle_content_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        digest: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &function_bundle_content_domain(project_id, environment_id),
            digest,
        )
    }

    pub fn schema_migration_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        collection_id: &mako_api::CollectionId,
        migration_id: &crate::SchemaMigrationId,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &schema_migration_domain(project_id, environment_id, collection_id),
            migration_id.as_str(),
        )
    }

    pub fn organizations_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(ORGANIZATIONS).map_err(ControlKeyspaceError)
    }

    /// A webhook endpoint registered for an environment.
    pub fn webhook_endpoint_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &webhook_tenant_domain(WEBHOOK_ENDPOINTS, project_id, environment_id),
            endpoint_id,
        )
    }

    pub fn webhook_endpoints_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(webhook_tenant_domain(
            WEBHOOK_ENDPOINTS,
            project_id,
            environment_id,
        ))
        .map_err(ControlKeyspaceError)
    }

    /// The node-wide registry of webhook endpoints the worker walks; the
    /// value names the tenant the endpoint record lives under.
    pub fn webhook_endpoint_index_key(endpoint_id: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(WEBHOOK_ENDPOINT_INDEX, endpoint_id)
    }

    pub fn webhook_endpoint_index_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(WEBHOOK_ENDPOINT_INDEX).map_err(ControlKeyspaceError)
    }

    /// The change-log position one endpoint has consumed for one collection.
    pub fn webhook_cursor_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
        collection_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &webhook_endpoint_domain(WEBHOOK_CURSORS, project_id, environment_id, endpoint_id),
            collection_id,
        )
    }

    pub fn webhook_cursors_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(webhook_endpoint_domain(
            WEBHOOK_CURSORS,
            project_id,
            environment_id,
            endpoint_id,
        ))
        .map_err(ControlKeyspaceError)
    }

    /// One delivery in an endpoint's log. `item` is the reverse-timestamped
    /// name the webhook module assigns, so a forward scan reads newest first.
    pub fn webhook_delivery_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
        item: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &webhook_endpoint_domain(WEBHOOK_DELIVERIES, project_id, environment_id, endpoint_id),
            item,
        )
    }

    pub fn webhook_deliveries_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(webhook_endpoint_domain(
            WEBHOOK_DELIVERIES,
            project_id,
            environment_id,
            endpoint_id,
        ))
        .map_err(ControlKeyspaceError)
    }

    /// Delivery id to log entry name, so a redelivery can find its original
    /// without walking the log.
    pub fn webhook_delivery_index_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
        delivery_id: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &webhook_endpoint_domain(
                WEBHOOK_DELIVERY_INDEX,
                project_id,
                environment_id,
                endpoint_id,
            ),
            delivery_id,
        )
    }

    pub fn webhook_delivery_index_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(webhook_endpoint_domain(
            WEBHOOK_DELIVERY_INDEX,
            project_id,
            environment_id,
            endpoint_id,
        ))
        .map_err(ControlKeyspaceError)
    }

    /// An endpoint's pending deliveries in creation order, so the delivery
    /// loop can keep one document's deliveries in sequence.
    pub fn webhook_pending_key(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
        item: &str,
    ) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(
            &webhook_endpoint_domain(WEBHOOK_PENDING, project_id, environment_id, endpoint_id),
            item,
        )
    }

    pub fn webhook_pending_range(
        project_id: &ProjectId,
        environment_id: &EnvironmentId,
        endpoint_id: &str,
    ) -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(webhook_endpoint_domain(
            WEBHOOK_PENDING,
            project_id,
            environment_id,
            endpoint_id,
        ))
        .map_err(ControlKeyspaceError)
    }

    /// The node-wide index of pending deliveries by their next attempt time.
    pub fn webhook_due_key(item: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
        system_key(WEBHOOK_DUE, item)
    }

    pub fn webhook_due_range() -> Result<KeyRange, ControlKeyspaceError> {
        TenantKeyspace::system_domain_range(WEBHOOK_DUE).map_err(ControlKeyspaceError)
    }
}

#[derive(Debug)]
pub struct ControlKeyspaceError(KeyCodecError);

impl fmt::Display for ControlKeyspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "control key cannot be encoded: {}", self.0)
    }
}

impl Error for ControlKeyspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

fn system_key(domain: &[u8], item: &str) -> Result<Vec<u8>, ControlKeyspaceError> {
    TenantKeyspace::system_key(domain, item).map_err(ControlKeyspaceError)
}

fn credit_domain(organization_id: &OrganizationId) -> Vec<u8> {
    format!("control/organization-credits/{}", organization_id.as_str()).into_bytes()
}

fn invoice_domain(organization_id: &OrganizationId) -> Vec<u8> {
    format!("control/organization-invoices/{}", organization_id.as_str()).into_bytes()
}

fn membership_domain(organization_id: &OrganizationId) -> Vec<u8> {
    format!("control/memberships/{}", organization_id.as_str()).into_bytes()
}

fn developer_status_domain(status: crate::DeveloperIdentityStatus) -> Vec<u8> {
    let status = match status {
        crate::DeveloperIdentityStatus::Unverified => "unverified",
        crate::DeveloperIdentityStatus::Waitlisted => "waitlisted",
        crate::DeveloperIdentityStatus::Active => "active",
        crate::DeveloperIdentityStatus::Rejected => "rejected",
        crate::DeveloperIdentityStatus::Disabled => "disabled",
        crate::DeveloperIdentityStatus::Deleted => "deleted",
    };
    format!("control/developers-by-status/{status}").into_bytes()
}

fn developer_organization_domain(developer_id: &DeveloperIdentityId) -> Vec<u8> {
    format!("control/developer-organizations/{}", developer_id.as_str()).into_bytes()
}

fn environment_domain(project_id: &ProjectId) -> Vec<u8> {
    format!("control/environments/{}", project_id.as_str()).into_bytes()
}

fn email_template_domain(project_id: &ProjectId, environment_id: &EnvironmentId) -> Vec<u8> {
    format!(
        "control/email-templates/v1/{}/{}",
        project_id.as_str(),
        environment_id.as_str()
    )
    .into_bytes()
}

fn organization_project_domain(organization_id: &OrganizationId) -> Vec<u8> {
    format!(
        "control/projects-by-organization/{}",
        organization_id.as_str()
    )
    .into_bytes()
}

fn schema_migration_domain(
    project_id: &ProjectId,
    environment_id: &EnvironmentId,
    collection_id: &mako_api::CollectionId,
) -> Vec<u8> {
    format!(
        "control/schema-migrations/{}/{}/{}",
        project_id.as_str(),
        environment_id.as_str(),
        collection_id.as_str()
    )
    .into_bytes()
}

fn function_domain(project_id: &ProjectId, environment_id: &EnvironmentId) -> Vec<u8> {
    format!(
        "control/functions/{}/{}",
        project_id.as_str(),
        environment_id.as_str()
    )
    .into_bytes()
}

fn function_version_domain(
    project_id: &ProjectId,
    environment_id: &EnvironmentId,
    name: &crate::FunctionName,
) -> Vec<u8> {
    format!(
        "control/function-versions/{}/{}/{}",
        project_id.as_str(),
        environment_id.as_str(),
        name.as_str()
    )
    .into_bytes()
}

fn function_bundle_domain(project_id: &ProjectId, environment_id: &EnvironmentId) -> Vec<u8> {
    format!(
        "control/function-bundles/{}/{}",
        project_id.as_str(),
        environment_id.as_str()
    )
    .into_bytes()
}

fn webhook_tenant_domain(
    root: &str,
    project_id: &ProjectId,
    environment_id: &EnvironmentId,
) -> Vec<u8> {
    format!("{root}/{}/{}", project_id.as_str(), environment_id.as_str()).into_bytes()
}

fn webhook_endpoint_domain(
    root: &str,
    project_id: &ProjectId,
    environment_id: &EnvironmentId,
    endpoint_id: &str,
) -> Vec<u8> {
    format!(
        "{root}/{}/{}/{endpoint_id}",
        project_id.as_str(),
        environment_id.as_str()
    )
    .into_bytes()
}

fn function_bundle_content_domain(
    project_id: &ProjectId,
    environment_id: &EnvironmentId,
) -> Vec<u8> {
    format!(
        "control/function-bundle-content/{}/{}",
        project_id.as_str(),
        environment_id.as_str()
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_storage::TenantKeyspace;
    use proptest::prelude::*;

    #[test]
    fn every_control_record_is_inside_the_reserved_system_range() {
        let system = TenantKeyspace::system_range().expect("system range");
        let project = ProjectId::parse("prj_abcdefgh").expect("project id");
        let environment = EnvironmentId::parse("env_abcdefgh").expect("environment id");
        let organization = OrganizationId::parse("org_abcdefgh").expect("organization id");
        let developer = DeveloperIdentityId::parse("dev_abcdefgh").expect("developer id");
        let invitation = InvitationId::parse("inv_abcdefgh").expect("invitation id");
        let keys = [
            ControlKeyspace::developer_key(&developer).expect("developer key"),
            ControlKeyspace::organization_key(&organization).expect("organization key"),
            ControlKeyspace::membership_key(&organization, &developer).expect("membership key"),
            ControlKeyspace::invitation_key(&invitation).expect("invitation key"),
            ControlKeyspace::project_key(&project).expect("project key"),
            ControlKeyspace::environment_key(&project, &environment).expect("environment key"),
            ControlKeyspace::quota_key(&project).expect("quota key"),
        ];
        assert!(keys.iter().all(|key| system.contains(key)));

        let tenant = TenantKeyspace::new("prj_abcdefgh", "env_abcdefgh").expect("tenant");
        let tenant_range = tenant.environment_range().expect("tenant range");
        assert!(keys.iter().all(|key| !tenant_range.contains(key)));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn arbitrary_management_tenants_have_disjoint_function_ranges(
            project_a in "[a-z0-9_-]{8,24}",
            environment_a in "[a-z0-9_-]{8,24}",
            project_b in "[a-z0-9_-]{8,24}",
            environment_b in "[a-z0-9_-]{8,24}",
            function in "[a-z][a-z0-9-]{0,30}",
        ) {
            prop_assume!(project_a != project_b || environment_a != environment_b);
            let project_a = ProjectId::parse(format!("prj_{project_a}")).expect("project A");
            let environment_a = EnvironmentId::parse(format!("env_{environment_a}")).expect("environment A");
            let project_b = ProjectId::parse(format!("prj_{project_b}")).expect("project B");
            let environment_b = EnvironmentId::parse(format!("env_{environment_b}")).expect("environment B");
            let name = crate::FunctionName::parse(function).expect("function");
            let key_a = ControlKeyspace::function_key(&project_a, &environment_a, &name).expect("key A");
            let key_b = ControlKeyspace::function_key(&project_b, &environment_b, &name).expect("key B");
            let range_a = ControlKeyspace::functions_range(&project_a, &environment_a).expect("range A");
            let range_b = ControlKeyspace::functions_range(&project_b, &environment_b).expect("range B");
            prop_assert_ne!(&key_a, &key_b);
            prop_assert!(range_a.contains(&key_a));
            prop_assert!(!range_a.contains(&key_b));
            prop_assert!(range_b.contains(&key_b));
            prop_assert!(!range_b.contains(&key_a));
        }
    }
}
