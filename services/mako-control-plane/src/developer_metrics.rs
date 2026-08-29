use std::sync::atomic::{AtomicU64, Ordering};

use mako_api::ErrorCode;
use mako_control_plane::{
    ApplicationMailWorkerReport, DeveloperOutboxWorkerReport, DeveloperRegistrationHealthSnapshot,
    OperatorAuthenticationHealthSnapshot,
};
use mako_service_runtime::{HttpApiError, HttpResponse};
use mako_storage::SqliteHealthSignals;

#[derive(Clone, Copy)]
pub(crate) enum DeveloperMetricOperation {
    Registration,
    Verification,
    VerificationResend,
    SignIn,
    Refresh,
    SignOut,
    RecoveryRequest,
    RecoveryCompletion,
    WaitlistStatus,
    ReviewList,
    ReviewDetail,
    Approve,
    Reject,
}

#[derive(Clone, Copy)]
pub(crate) enum OperatorAuthMetricOperation {
    SignIn,
    Inspect,
    SignOut,
    StepUp,
}

pub(crate) struct DeveloperMetricsRenderContext<'a> {
    pub(crate) health: &'a DeveloperRegistrationHealthSnapshot,
    pub(crate) registration_enabled: bool,
    pub(crate) mail_ready: bool,
    pub(crate) operator_health: &'a OperatorAuthenticationHealthSnapshot,
    pub(crate) operator_password_enabled: bool,
    pub(crate) operator_break_glass_enabled: bool,
    pub(crate) control_storage: Option<&'a SqliteHealthSignals>,
}

#[derive(Debug, Default)]
pub(crate) struct DeveloperMetrics {
    registration_requests: AtomicU64,
    verification_requests: AtomicU64,
    sign_in_requests: AtomicU64,
    recovery_requests: AtomicU64,
    waitlist_status_requests: AtomicU64,
    review_requests: AtomicU64,
    approvals: AtomicU64,
    rejections: AtomicU64,
    self_reviews: AtomicU64,
    throttles: AtomicU64,
    failures: AtomicU64,
    mail_delivered: AtomicU64,
    mail_retried: AtomicU64,
    mail_dead_lettered: AtomicU64,
    mail_worker_failures: AtomicU64,
    application_mail_stored: AtomicU64,
    application_mail_delivered: AtomicU64,
    application_mail_retried: AtomicU64,
    application_mail_dead_lettered: AtomicU64,
    application_mail_worker_failures: AtomicU64,
    operator_sign_in_successes: AtomicU64,
    operator_sign_in_failures: AtomicU64,
    operator_throttles: AtomicU64,
    operator_session_revocations: AtomicU64,
    operator_step_up_successes: AtomicU64,
    operator_step_up_requirements: AtomicU64,
    operator_bootstrap_successes: AtomicU64,
    operator_bootstrap_failures: AtomicU64,
    developer_role_repair_successes: AtomicU64,
    developer_role_repair_failures: AtomicU64,
    operator_control_center_requests: AtomicU64,
    operator_control_center_failures: AtomicU64,
    operator_control_center_unavailable: AtomicU64,
    operator_control_center_latency_milliseconds: AtomicU64,
}

impl DeveloperMetrics {
    pub(crate) fn observe_http(
        &self,
        operation: DeveloperMetricOperation,
        result: &Result<HttpResponse, HttpApiError>,
    ) {
        let counter = match operation {
            DeveloperMetricOperation::Registration => Some(&self.registration_requests),
            DeveloperMetricOperation::Verification
            | DeveloperMetricOperation::VerificationResend => Some(&self.verification_requests),
            DeveloperMetricOperation::SignIn
            | DeveloperMetricOperation::Refresh
            | DeveloperMetricOperation::SignOut => Some(&self.sign_in_requests),
            DeveloperMetricOperation::RecoveryRequest
            | DeveloperMetricOperation::RecoveryCompletion => Some(&self.recovery_requests),
            DeveloperMetricOperation::WaitlistStatus => Some(&self.waitlist_status_requests),
            DeveloperMetricOperation::ReviewList | DeveloperMetricOperation::ReviewDetail => {
                Some(&self.review_requests)
            }
            DeveloperMetricOperation::Approve | DeveloperMetricOperation::Reject => None,
        };
        if let Some(counter) = counter {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        if result.is_ok() {
            match operation {
                DeveloperMetricOperation::Approve => {
                    self.approvals.fetch_add(1, Ordering::Relaxed);
                }
                DeveloperMetricOperation::Reject => {
                    self.rejections.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
        if let Err(error) = result {
            if error.envelope().error.code == ErrorCode::RateLimited {
                self.throttles.fetch_add(1, Ordering::Relaxed);
            } else {
                self.failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn observe_mail(&self, report: &DeveloperOutboxWorkerReport) {
        self.mail_delivered
            .fetch_add(report.delivered as u64, Ordering::Relaxed);
        self.mail_retried
            .fetch_add(report.retried as u64, Ordering::Relaxed);
        self.mail_dead_lettered
            .fetch_add(report.dead_lettered as u64, Ordering::Relaxed);
    }

    pub(crate) fn observe_mail_worker_failure(&self) {
        self.mail_worker_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_application_mail(&self, report: &ApplicationMailWorkerReport) {
        self.application_mail_stored
            .fetch_add(report.stored as u64, Ordering::Relaxed);
        self.application_mail_delivered
            .fetch_add(report.delivered as u64, Ordering::Relaxed);
        self.application_mail_retried
            .fetch_add(report.retried as u64, Ordering::Relaxed);
        self.application_mail_dead_lettered.fetch_add(
            (report.dead_lettered + report.refused_at_intake) as u64,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn observe_application_mail_worker_failure(&self) {
        self.application_mail_worker_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_operator_auth(
        &self,
        operation: OperatorAuthMetricOperation,
        result: &Result<HttpResponse, HttpApiError>,
    ) {
        match (operation, result) {
            (OperatorAuthMetricOperation::SignIn, Ok(_)) => {
                self.operator_sign_in_successes
                    .fetch_add(1, Ordering::Relaxed);
            }
            (OperatorAuthMetricOperation::SignIn, Err(error))
                if error.envelope().error.code == ErrorCode::RateLimited =>
            {
                self.operator_throttles.fetch_add(1, Ordering::Relaxed);
            }
            (OperatorAuthMetricOperation::SignIn, Err(_)) => {
                self.operator_sign_in_failures
                    .fetch_add(1, Ordering::Relaxed);
            }
            (OperatorAuthMetricOperation::SignOut, Ok(_)) => {
                self.operator_session_revocations
                    .fetch_add(1, Ordering::Relaxed);
            }
            (OperatorAuthMetricOperation::StepUp, Ok(_)) => {
                self.operator_step_up_successes
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub(crate) fn observe_operator_step_up_required(&self) {
        self.operator_step_up_requirements
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_operator_bootstrap(&self, succeeded: bool) {
        if succeeded {
            self.operator_bootstrap_successes
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.operator_bootstrap_failures
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn observe_self_review(&self) {
        self.self_reviews.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe_developer_role_repair(&self, succeeded: bool) {
        if succeeded {
            self.developer_role_repair_successes
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.developer_role_repair_failures
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn observe_operator_control_center(
        &self,
        result: &Result<HttpResponse, HttpApiError>,
        elapsed_milliseconds: u64,
    ) {
        self.operator_control_center_requests
            .fetch_add(1, Ordering::Relaxed);
        self.operator_control_center_latency_milliseconds
            .fetch_add(elapsed_milliseconds, Ordering::Relaxed);
        if let Err(error) = result {
            self.operator_control_center_failures
                .fetch_add(1, Ordering::Relaxed);
            if error.envelope().error.retry != mako_api::RetryAdvice::Never {
                self.operator_control_center_unavailable
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn render(&self, context: DeveloperMetricsRenderContext<'_>) -> String {
        let DeveloperMetricsRenderContext {
            health,
            registration_enabled,
            mail_ready,
            operator_health,
            operator_password_enabled,
            operator_break_glass_enabled,
            control_storage,
        } = context;
        let mut output = String::with_capacity(4_096);
        metric(
            &mut output,
            "mako_developer_registration_requests_total",
            self.registration_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_verification_requests_total",
            self.verification_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_sign_in_requests_total",
            self.sign_in_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_recovery_requests_total",
            self.recovery_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_waitlist_status_requests_total",
            self.waitlist_status_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_review_requests_total",
            self.review_requests.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_approvals_total",
            self.approvals.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_rejections_total",
            self.rejections.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_throttles_total",
            self.throttles.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_failures_total",
            self.failures.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_mail_delivered_total",
            self.mail_delivered.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_mail_retried_total",
            self.mail_retried.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_mail_dead_lettered_total",
            self.mail_dead_lettered.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_mail_worker_failures_total",
            self.mail_worker_failures.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_application_mail_stored_total",
            self.application_mail_stored.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_application_mail_delivered_total",
            self.application_mail_delivered.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_application_mail_retried_total",
            self.application_mail_retried.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_application_mail_dead_lettered_total",
            self.application_mail_dead_lettered.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_application_mail_worker_failures_total",
            self.application_mail_worker_failures
                .load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_registration_enabled",
            u64::from(registration_enabled),
        );
        metric(
            &mut output,
            "mako_developer_mail_ready",
            u64::from(mail_ready),
        );
        for (state, count) in [
            ("unverified", health.unverified_identities),
            ("waitlisted", health.waitlisted_identities),
            ("active", health.active_identities),
            ("rejected", health.rejected_identities),
            ("disabled", health.disabled_identities),
            ("deleted", health.deleted_identities),
        ] {
            output.push_str(&format!(
                "mako_developer_identities{{state=\"{state}\"}} {count}\n"
            ));
        }
        metric(
            &mut output,
            "mako_developer_waitlist_oldest_age_seconds",
            health.oldest_waitlisted_age_seconds,
        );
        metric(
            &mut output,
            "mako_developer_outbox_pending",
            health.pending_outbox as u64,
        );
        metric(
            &mut output,
            "mako_developer_outbox_dead_letters",
            health.dead_letter_outbox as u64,
        );
        metric(
            &mut output,
            "mako_developer_outbox_oldest_age_seconds",
            health.oldest_pending_outbox_age_seconds,
        );
        metric(
            &mut output,
            "mako_developer_refresh_sessions",
            health.refresh_sessions as u64,
        );
        metric(
            &mut output,
            "mako_developer_auth_tokens",
            health.auth_tokens as u64,
        );
        metric(
            &mut output,
            "mako_developer_decisions",
            health.decisions as u64,
        );
        metric(
            &mut output,
            "mako_developer_self_reviews_total",
            self.self_reviews.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_sign_in_successes_total",
            self.operator_sign_in_successes.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_sign_in_failures_total",
            self.operator_sign_in_failures.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_throttles_total",
            self.operator_throttles.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_session_revocations_total",
            self.operator_session_revocations.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_step_up_successes_total",
            self.operator_step_up_successes.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_step_up_required_total",
            self.operator_step_up_requirements.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_bootstrap_successes_total",
            self.operator_bootstrap_successes.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_bootstrap_failures_total",
            self.operator_bootstrap_failures.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_role_repair_successes_total",
            self.developer_role_repair_successes.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_developer_role_repair_failures_total",
            self.developer_role_repair_failures.load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_active_sessions",
            operator_health.active_sessions as u64,
        );
        metric(
            &mut output,
            "mako_operator_revoked_sessions",
            operator_health.revoked_sessions as u64,
        );
        metric(
            &mut output,
            "mako_operator_entitlements",
            operator_health.entitlements as u64,
        );
        metric(
            &mut output,
            "mako_operator_attempt_records",
            operator_health.attempt_records as u64,
        );
        metric(
            &mut output,
            "mako_operator_password_auth_enabled",
            u64::from(operator_password_enabled),
        );
        metric(
            &mut output,
            "mako_operator_break_glass_bearer_enabled",
            u64::from(operator_break_glass_enabled),
        );
        metric(
            &mut output,
            "mako_operator_control_center_requests_total",
            self.operator_control_center_requests
                .load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_control_center_failures_total",
            self.operator_control_center_failures
                .load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_control_center_unavailable_total",
            self.operator_control_center_unavailable
                .load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_operator_control_center_latency_milliseconds_total",
            self.operator_control_center_latency_milliseconds
                .load(Ordering::Relaxed),
        );
        metric(
            &mut output,
            "mako_control_sqlite_ready",
            u64::from(control_storage.is_some()),
        );
        if let Some(storage) = control_storage {
            metric(
                &mut output,
                "mako_control_sqlite_integrity_verified",
                u64::from(storage.integrity_verified),
            );
            metric(
                &mut output,
                "mako_control_sqlite_schema_version",
                u64::from(storage.schema_version),
            );
            metric(
                &mut output,
                "mako_control_sqlite_database_bytes",
                storage.database_bytes,
            );
            metric(
                &mut output,
                "mako_control_sqlite_wal_bytes",
                storage.wal_bytes,
            );
            metric(
                &mut output,
                "mako_control_sqlite_available_bytes",
                storage.available_bytes,
            );
            metric(
                &mut output,
                "mako_control_sqlite_active_operations",
                storage.active_operations as u64,
            );
            metric(
                &mut output,
                "mako_control_sqlite_active_transactions",
                storage.active_transactions as u64,
            );
            metric(
                &mut output,
                "mako_control_sqlite_transaction_oldest_seconds",
                storage.oldest_transaction_seconds,
            );
            metric(
                &mut output,
                "mako_control_sqlite_disk_warning_free_bytes",
                storage.disk_warning_free_bytes,
            );
            metric(
                &mut output,
                "mako_control_sqlite_disk_critical_free_bytes",
                storage.disk_critical_free_bytes,
            );
            metric(
                &mut output,
                "mako_control_sqlite_busy_total",
                storage.busy_failures,
            );
            metric(
                &mut output,
                "mako_control_sqlite_checkpoint_failures_total",
                storage.checkpoint_failures,
            );
        }
        output
    }
}

fn metric(output: &mut String, name: &str, value: u64) {
    output.push_str(name);
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_metrics_have_only_bounded_aggregate_labels() {
        let metrics = DeveloperMetrics::default();
        let health = DeveloperRegistrationHealthSnapshot::default();
        let operator_health = OperatorAuthenticationHealthSnapshot::default();
        let rendered = metrics.render(DeveloperMetricsRenderContext {
            health: &health,
            registration_enabled: true,
            mail_ready: false,
            operator_health: &operator_health,
            operator_password_enabled: false,
            operator_break_glass_enabled: false,
            control_storage: None,
        });
        assert!(rendered.contains("mako_developer_registration_enabled 1"));
        assert!(rendered.contains("mako_developer_identities{state=\"waitlisted\"} 0"));
        for forbidden in ["email", "source", "identity_id", "token", "reason"] {
            assert!(!rendered.contains(&format!("{{{forbidden}=")));
        }
    }

    #[test]
    fn decision_metrics_count_only_committed_successes() {
        let metrics = DeveloperMetrics::default();
        let failure = Err(HttpApiError::new(
            409,
            ErrorCode::Conflict,
            "decision conflicted",
            "request_metrics_001",
            mako_api::RetryAdvice::Never,
        ));
        metrics.observe_http(DeveloperMetricOperation::Approve, &failure);
        metrics.observe_http(
            DeveloperMetricOperation::Reject,
            &Ok(HttpResponse::empty(200)),
        );
        let health = DeveloperRegistrationHealthSnapshot::default();
        let operator_health = OperatorAuthenticationHealthSnapshot::default();
        let rendered = metrics.render(DeveloperMetricsRenderContext {
            health: &health,
            registration_enabled: true,
            mail_ready: true,
            operator_health: &operator_health,
            operator_password_enabled: false,
            operator_break_glass_enabled: false,
            control_storage: None,
        });
        assert!(rendered.contains("mako_developer_approvals_total 0"));
        assert!(rendered.contains("mako_developer_rejections_total 1"));
        assert!(rendered.contains("mako_developer_failures_total 1"));
    }
}
