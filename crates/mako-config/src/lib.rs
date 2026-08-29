//! Typed, layered service configuration with safe startup diagnostics.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt, fs,
    net::SocketAddr,
    num::{NonZeroU32, NonZeroUsize},
    path::Component,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use serde::Deserialize;
use url::Url;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SECRET_BYTES: u64 = 64 * 1024;
const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORAGE_OPERATIONS: u64 = 1_000_000;
const MAX_TRANSACTION_SECONDS: u64 = 600;
const MIN_PRODUCTION_DISK_RESERVE_BYTES: u64 = 64 * 1024 * 1024;

/// Identifies a deployable service and supplies its local listen default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceKind {
    DataPlane,
    ControlPlane,
    EdgeGateway,
}

impl ServiceKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::DataPlane => "mako-data-plane",
            Self::ControlPlane => "mako-control-plane",
            Self::EdgeGateway => "mako-edge-gateway",
        }
    }

    const fn default_port(self) -> u16 {
        match self {
            Self::DataPlane => 8080,
            Self::ControlPlane => 8081,
            Self::EdgeGateway => 8082,
        }
    }

    #[must_use]
    pub const fn owns_state(self) -> bool {
        matches!(self, Self::DataPlane | Self::ControlPlane)
    }
}

/// Fully bounded RocksDB settings. Production validation makes paths absolute
/// and non-ephemeral; durability is intentionally not configurable and is
/// always upgraded to synchronous by the storage crate.
#[derive(Clone, Debug)]
pub struct ProductionRocksDbSettings {
    pub path: PathBuf,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub transaction_lock_timeout: Duration,
    pub transaction_expiration: Duration,
    pub backup_destination: PathBuf,
    pub backup_retention_count: NonZeroU32,
    pub disk_warning_free_bytes: u64,
    pub disk_critical_free_bytes: u64,
}

/// Server-side SQLite settings used only by the control-plane service.
#[derive(Clone, Debug)]
pub struct ProductionSqliteSettings {
    pub database_path: PathBuf,
    pub lock_path: PathBuf,
    pub database_identity: String,
    pub migration_workspace: PathBuf,
    pub backup_staging: PathBuf,
    pub backup_publish: PathBuf,
    pub restore_workspace: PathBuf,
    pub reserve_path: PathBuf,
    pub maximum_batch_operations: NonZeroUsize,
    pub maximum_scan_items: NonZeroUsize,
    pub busy_timeout: Duration,
    pub transaction_expiration: Duration,
    pub shutdown_timeout: Duration,
    pub wal_autocheckpoint_pages: NonZeroUsize,
    pub maximum_wal_bytes: u64,
    pub integrity_interval: Duration,
    pub backup_retention_count: NonZeroU32,
    pub disk_warning_free_bytes: u64,
    pub disk_critical_free_bytes: u64,
}

/// Deployment modes whose security-sensitive defaults differ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentEnvironment {
    Local,
    Development,
    Staging,
    Production,
}

impl FromStr for DeploymentEnvironment {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "local" => Ok(Self::Local),
            "development" => Ok(Self::Development),
            "staging" => Ok(Self::Staging),
            "production" => Ok(Self::Production),
            _ => Err(()),
        }
    }
}

/// A resolved secret. Debug and display output are always redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(Box<str>);

impl SecretString {
    /// Explicitly exposes the secret to code that must authenticate downstream.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Validated configuration shared by every Mako Cloud service.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub service: ServiceKind,
    pub environment: DeploymentEnvironment,
    pub region: String,
    pub bind_address: SocketAddr,
    pub public_url: Option<Url>,
    pub tls_certificate_path: Option<PathBuf>,
    pub tls_private_key_path: Option<PathBuf>,
    pub rocksdb: ProductionRocksDbSettings,
    pub control_sqlite: Option<ProductionSqliteSettings>,
    pub smtp_address: SocketAddr,
    pub developer_registration: DeveloperRegistrationSettings,
    pub operator_authentication: OperatorAuthenticationSettings,
    pub object_store_endpoint: Url,
    pub data_plane_address: SocketAddr,
    /// Where the control plane's scheduler invokes functions: the edge
    /// gateway's loopback listener.
    pub edge_gateway_address: SocketAddr,
    pub runtime_supervisor_address: SocketAddr,
    pub telemetry_query_address: SocketAddr,
    pub otlp_address: SocketAddr,
    pub max_request_bytes: u64,
    pub shutdown_grace: Duration,
    pub internal_auth_secret: Option<SecretString>,
    pub object_store_access_key: Option<SecretString>,
    pub object_store_secret_key: Option<SecretString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmtpTlsMode {
    Wrapper,
    StartTls,
    /// Unencrypted SMTP for a loopback relay such as mailpit. Refused in
    /// production; the only mode in which credentials may be omitted.
    Plaintext,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedSmtpSettings {
    pub relay_hostname: String,
    pub port: u16,
    pub tls_mode: SmtpTlsMode,
    /// Present together with `password`; both are required unless the TLS
    /// mode is `Plaintext`.
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub sender: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct DeveloperRegistrationSettings {
    pub enabled: bool,
    pub verification_lifetime: Duration,
    pub recovery_lifetime: Duration,
    pub access_lifetime: Duration,
    pub refresh_lifetime: Duration,
    pub decision_retention: Duration,
    pub rate_window: Duration,
    pub global_requests_per_window: u32,
    pub source_requests_per_window: u32,
    pub email_requests_per_window: u32,
    pub token_attempts_per_window: u32,
    pub maximum_parallel_password_work: usize,
    pub maximum_pending_outbox: usize,
    pub maximum_outbox_batch: usize,
    pub outbox_lease: Duration,
    pub outbox_maximum_attempts: u32,
    pub outbox_maximum_backoff: Duration,
    pub delivered_mail_retention: Duration,
    pub mail_encryption_secret: Option<SecretString>,
    pub smtp: Option<AuthenticatedSmtpSettings>,
}

#[derive(Clone, Debug)]
pub struct OperatorAuthenticationSettings {
    pub enabled: bool,
    pub session_lifetime: Duration,
    pub mutation_freshness: Duration,
    pub attempt_window: Duration,
    pub source_attempts_per_window: u32,
    pub identity_attempts_per_window: u32,
    pub base_backoff: Duration,
    pub maximum_backoff: Duration,
    pub cookie_name: String,
    pub cookie_secure: bool,
    pub cookie_http_only: bool,
    pub cookie_same_site: String,
    pub cookie_path: String,
    pub break_glass_bearer_enabled: bool,
}

impl ServiceConfig {
    /// Loads the optional JSON file and then applies process environment overrides.
    pub fn load_from_process(service: ServiceKind) -> Result<Self, ConfigDiagnostic> {
        ConfigLoader::from_process().load(service)
    }

    /// Returns a non-sensitive startup summary suitable for logs.
    #[must_use]
    pub fn startup_diagnostic(&self) -> StartupDiagnostic<'_> {
        StartupDiagnostic { config: self }
    }
}

/// A safe summary of validated startup configuration.
pub struct StartupDiagnostic<'a> {
    config: &'a ServiceConfig,
}

impl fmt::Display for StartupDiagnostic<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "service={} environment={:?} region={} bind={} storage={} tls={} internal_auth={} object_store_auth={} developer_registration={} developer_mail={} operator_password_auth={} operator_break_glass={}",
            self.config.service.name(),
            self.config.environment,
            self.config.region,
            self.config.bind_address,
            if self.config.control_sqlite.is_some() {
                "control-sqlite"
            } else {
                "rocksdb"
            },
            if self.config.tls_certificate_path.is_some() {
                "configured"
            } else {
                "disabled"
            },
            if self.config.internal_auth_secret.is_some() {
                "configured"
            } else {
                "disabled"
            },
            if self.config.object_store_access_key.is_some()
                && self.config.object_store_secret_key.is_some()
            {
                "configured"
            } else {
                "disabled"
            },
            if self.config.developer_registration.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if self.config.developer_registration.smtp.is_some()
                && self
                    .config
                    .developer_registration
                    .mail_encryption_secret
                    .is_some()
            {
                "configured"
            } else {
                "disabled"
            },
            if self.config.operator_authentication.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if self
                .config
                .operator_authentication
                .break_glass_bearer_enabled
            {
                "enabled"
            } else {
                "disabled"
            },
        )
    }
}

/// Stable classes for startup failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigErrorCode {
    FileUnreadable,
    FileTooLarge,
    InvalidDocument,
    InvalidValue,
    MissingValue,
    SecretUnavailable,
}

impl ConfigErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileUnreadable => "CONFIG_FILE_UNREADABLE",
            Self::FileTooLarge => "CONFIG_FILE_TOO_LARGE",
            Self::InvalidDocument => "CONFIG_INVALID_DOCUMENT",
            Self::InvalidValue => "CONFIG_INVALID_VALUE",
            Self::MissingValue => "CONFIG_MISSING_VALUE",
            Self::SecretUnavailable => "CONFIG_SECRET_UNAVAILABLE",
        }
    }
}

/// A non-sensitive, field-addressed startup diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigDiagnostic {
    pub code: ConfigErrorCode,
    pub field: String,
    pub message: String,
}

impl ConfigDiagnostic {
    fn new(code: ConfigErrorCode, field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "configuration error {} at {}: {}",
            self.code.as_str(),
            self.field,
            self.message
        )
    }
}

impl std::error::Error for ConfigDiagnostic {}

/// A configuration source whose environment can be injected in tests.
#[derive(Clone, Debug, Default)]
pub struct ConfigLoader {
    environment: BTreeMap<String, OsString>,
}

impl ConfigLoader {
    #[must_use]
    pub fn from_process() -> Self {
        let environment = env::vars_os()
            .filter_map(|(name, value)| name.into_string().ok().map(|name| (name, value)))
            .collect();
        Self { environment }
    }

    #[must_use]
    pub fn from_environment<I, K, V>(values: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<OsString>,
    {
        Self {
            environment: values
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    pub fn load(&self, service: ServiceKind) -> Result<ServiceConfig, ConfigDiagnostic> {
        let mut raw = RawConfig::defaults(service);
        if let Some(path) = self.environment.get("MAKO_CONFIG_FILE") {
            raw.apply_overlay(read_overlay(Path::new(path))?);
        }
        raw.apply_environment(self)?;
        raw.validate(service, self)
    }

    fn utf8(&self, name: &str) -> Result<Option<String>, ConfigDiagnostic> {
        self.environment
            .get(name)
            .map(|value| {
                value.clone().into_string().map_err(|_| {
                    ConfigDiagnostic::new(
                        ConfigErrorCode::InvalidValue,
                        name,
                        "value must be valid UTF-8",
                    )
                })
            })
            .transpose()
    }
}

#[derive(Debug)]
struct RawConfig {
    environment: String,
    region: String,
    bind_address: String,
    public_url: Option<String>,
    tls_certificate_path: Option<String>,
    tls_private_key_path: Option<String>,
    rocksdb_path: String,
    maximum_batch_operations: String,
    maximum_scan_items: String,
    transaction_lock_timeout_seconds: String,
    transaction_expiration_seconds: String,
    backup_destination: String,
    backup_retention_count: String,
    disk_warning_free_bytes: String,
    disk_critical_free_bytes: String,
    legacy_control_rocksdb_configured: bool,
    control_sqlite_path: String,
    control_sqlite_lock_path: String,
    control_sqlite_identity: String,
    control_sqlite_migration_workspace: String,
    control_sqlite_backup_staging: String,
    control_sqlite_backup_publish: String,
    control_sqlite_restore_workspace: String,
    control_sqlite_reserve_path: String,
    control_sqlite_maximum_batch_operations: String,
    control_sqlite_maximum_scan_items: String,
    control_sqlite_busy_timeout_seconds: String,
    control_sqlite_transaction_expiration_seconds: String,
    control_sqlite_shutdown_timeout_seconds: String,
    control_sqlite_wal_autocheckpoint_pages: String,
    control_sqlite_maximum_wal_bytes: String,
    control_sqlite_integrity_interval_seconds: String,
    control_sqlite_backup_retention_count: String,
    control_sqlite_disk_warning_free_bytes: String,
    control_sqlite_disk_critical_free_bytes: String,
    smtp_address: String,
    developer_registration_enabled: String,
    developer_verification_lifetime_seconds: String,
    developer_recovery_lifetime_seconds: String,
    developer_access_lifetime_seconds: String,
    developer_refresh_lifetime_seconds: String,
    developer_decision_retention_seconds: String,
    developer_rate_window_seconds: String,
    developer_global_requests_per_window: String,
    developer_source_requests_per_window: String,
    developer_email_requests_per_window: String,
    developer_token_attempts_per_window: String,
    developer_maximum_parallel_password_work: String,
    developer_maximum_pending_outbox: String,
    developer_maximum_outbox_batch: String,
    developer_outbox_lease_seconds: String,
    developer_outbox_maximum_attempts: String,
    developer_outbox_maximum_backoff_seconds: String,
    developer_delivered_mail_retention_seconds: String,
    developer_smtp_relay_hostname: Option<String>,
    developer_smtp_port: String,
    developer_smtp_tls_mode: String,
    developer_smtp_username: Option<String>,
    developer_smtp_sender: Option<String>,
    developer_smtp_timeout_seconds: String,
    developer_mail_encryption_secret_ref: Option<String>,
    developer_smtp_password_ref: Option<String>,
    operator_authentication_enabled: String,
    operator_session_lifetime_seconds: String,
    operator_mutation_freshness_seconds: String,
    operator_attempt_window_seconds: String,
    operator_source_attempts_per_window: String,
    operator_identity_attempts_per_window: String,
    operator_base_backoff_seconds: String,
    operator_maximum_backoff_seconds: String,
    operator_break_glass_bearer_enabled: String,
    object_store_endpoint: String,
    data_plane_address: String,
    edge_gateway_address: String,
    runtime_supervisor_address: String,
    telemetry_query_address: String,
    otlp_address: String,
    max_request_bytes: String,
    shutdown_grace_seconds: String,
    internal_auth_secret_ref: Option<String>,
    object_store_access_key_ref: Option<String>,
    object_store_secret_key_ref: Option<String>,
}

impl RawConfig {
    fn defaults(service: ServiceKind) -> Self {
        Self {
            environment: "local".into(),
            region: "local".into(),
            bind_address: format!("127.0.0.1:{}", service.default_port()),
            public_url: None,
            tls_certificate_path: None,
            tls_private_key_path: None,
            rocksdb_path: format!(".local/data/{}/rocksdb", service.name()),
            maximum_batch_operations: "10000".into(),
            maximum_scan_items: "10000".into(),
            transaction_lock_timeout_seconds: "2".into(),
            transaction_expiration_seconds: "30".into(),
            backup_destination: format!(".local/backups/{}", service.name()),
            backup_retention_count: "14".into(),
            disk_warning_free_bytes: (512 * 1024 * 1024_u64).to_string(),
            disk_critical_free_bytes: (256 * 1024 * 1024_u64).to_string(),
            legacy_control_rocksdb_configured: false,
            control_sqlite_path: format!(
                ".local/data/{}/control.sqlite3",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_lock_path: format!(
                ".local/data/{}/control.sqlite3.lock",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_identity: "mako-control-local".into(),
            control_sqlite_migration_workspace: format!(
                ".local/migrations/{}",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_backup_staging: format!(
                ".local/backups/{}/staging",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_backup_publish: format!(
                ".local/backups/{}/published",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_restore_workspace: format!(
                ".local/restores/{}",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_reserve_path: format!(
                ".local/reserve/{}",
                ServiceKind::ControlPlane.name()
            ),
            control_sqlite_maximum_batch_operations: "10000".into(),
            control_sqlite_maximum_scan_items: "10000".into(),
            control_sqlite_busy_timeout_seconds: "2".into(),
            control_sqlite_transaction_expiration_seconds: "30".into(),
            control_sqlite_shutdown_timeout_seconds: "10".into(),
            control_sqlite_wal_autocheckpoint_pages: "1000".into(),
            control_sqlite_maximum_wal_bytes: (256 * 1024 * 1024_u64).to_string(),
            control_sqlite_integrity_interval_seconds: (15 * 60).to_string(),
            control_sqlite_backup_retention_count: "14".into(),
            control_sqlite_disk_warning_free_bytes: (512 * 1024 * 1024_u64).to_string(),
            control_sqlite_disk_critical_free_bytes: (256 * 1024 * 1024_u64).to_string(),
            smtp_address: "127.0.0.1:1025".into(),
            developer_registration_enabled: "false".into(),
            developer_verification_lifetime_seconds: (24 * 60 * 60).to_string(),
            developer_recovery_lifetime_seconds: (60 * 60).to_string(),
            developer_access_lifetime_seconds: (15 * 60).to_string(),
            developer_refresh_lifetime_seconds: (30 * 24 * 60 * 60).to_string(),
            developer_decision_retention_seconds: (90 * 24 * 60 * 60).to_string(),
            developer_rate_window_seconds: (15 * 60).to_string(),
            developer_global_requests_per_window: "500".into(),
            developer_source_requests_per_window: "30".into(),
            developer_email_requests_per_window: "8".into(),
            developer_token_attempts_per_window: "20".into(),
            developer_maximum_parallel_password_work: "4".into(),
            developer_maximum_pending_outbox: "5000".into(),
            developer_maximum_outbox_batch: "32".into(),
            developer_outbox_lease_seconds: "60".into(),
            developer_outbox_maximum_attempts: "8".into(),
            developer_outbox_maximum_backoff_seconds: (6 * 60 * 60).to_string(),
            developer_delivered_mail_retention_seconds: (30 * 24 * 60 * 60).to_string(),
            developer_smtp_relay_hostname: None,
            developer_smtp_port: "587".into(),
            developer_smtp_tls_mode: "starttls".into(),
            developer_smtp_username: None,
            developer_smtp_sender: None,
            developer_smtp_timeout_seconds: "15".into(),
            developer_mail_encryption_secret_ref: None,
            developer_smtp_password_ref: None,
            operator_authentication_enabled: "false".into(),
            operator_session_lifetime_seconds: (60 * 60).to_string(),
            operator_mutation_freshness_seconds: (5 * 60).to_string(),
            operator_attempt_window_seconds: (15 * 60).to_string(),
            operator_source_attempts_per_window: "20".into(),
            operator_identity_attempts_per_window: "10".into(),
            operator_base_backoff_seconds: "1".into(),
            operator_maximum_backoff_seconds: (5 * 60).to_string(),
            operator_break_glass_bearer_enabled: "false".into(),
            object_store_endpoint: "http://127.0.0.1:8333".into(),
            data_plane_address: "127.0.0.1:8080".into(),
            edge_gateway_address: "127.0.0.1:8082".into(),
            runtime_supervisor_address: "127.0.0.1:9001".into(),
            telemetry_query_address: "127.0.0.1:9465".into(),
            otlp_address: "127.0.0.1:4317".into(),
            max_request_bytes: (1024 * 1024).to_string(),
            shutdown_grace_seconds: "30".into(),
            internal_auth_secret_ref: None,
            object_store_access_key_ref: None,
            object_store_secret_key_ref: None,
        }
    }

    fn apply_overlay(&mut self, overlay: ConfigOverlay) {
        if let Some(value) = overlay.deployment.environment {
            self.environment = value;
        }
        if let Some(value) = overlay.deployment.region {
            self.region = value;
        }
        if let Some(value) = overlay.server.bind_address {
            self.bind_address = value;
        }
        if let Some(value) = overlay.server.public_url {
            self.public_url = Some(value);
        }
        if let Some(value) = overlay.server.tls_certificate_path {
            self.tls_certificate_path = Some(value);
        }
        if let Some(value) = overlay.server.tls_private_key_path {
            self.tls_private_key_path = Some(value);
        }
        if let Some(value) = overlay.storage.rocksdb_path {
            self.rocksdb_path = value;
            self.legacy_control_rocksdb_configured = true;
        }
        if let Some(value) = overlay.storage.maximum_batch_operations {
            self.maximum_batch_operations = value.to_string();
        }
        if let Some(value) = overlay.storage.maximum_scan_items {
            self.maximum_scan_items = value.to_string();
        }
        if let Some(value) = overlay.storage.transaction_lock_timeout_seconds {
            self.transaction_lock_timeout_seconds = value.to_string();
        }
        if let Some(value) = overlay.storage.transaction_expiration_seconds {
            self.transaction_expiration_seconds = value.to_string();
        }
        if let Some(value) = overlay.storage.backup_destination {
            self.backup_destination = value;
        }
        if let Some(value) = overlay.storage.backup_retention_count {
            self.backup_retention_count = value.to_string();
        }
        if let Some(value) = overlay.storage.disk_warning_free_bytes {
            self.disk_warning_free_bytes = value.to_string();
        }
        if let Some(value) = overlay.storage.disk_critical_free_bytes {
            self.disk_critical_free_bytes = value.to_string();
        }
        macro_rules! apply_control_sqlite_string {
            ($overlay:ident, $target:ident) => {
                if let Some(value) = overlay.control_storage.$overlay {
                    self.$target = value;
                }
            };
        }
        macro_rules! apply_control_sqlite_number {
            ($overlay:ident, $target:ident) => {
                if let Some(value) = overlay.control_storage.$overlay {
                    self.$target = value.to_string();
                }
            };
        }
        apply_control_sqlite_string!(database_path, control_sqlite_path);
        apply_control_sqlite_string!(lock_path, control_sqlite_lock_path);
        apply_control_sqlite_string!(database_identity, control_sqlite_identity);
        apply_control_sqlite_string!(migration_workspace, control_sqlite_migration_workspace);
        apply_control_sqlite_string!(backup_staging, control_sqlite_backup_staging);
        apply_control_sqlite_string!(backup_publish, control_sqlite_backup_publish);
        apply_control_sqlite_string!(restore_workspace, control_sqlite_restore_workspace);
        apply_control_sqlite_string!(reserve_path, control_sqlite_reserve_path);
        apply_control_sqlite_number!(
            maximum_batch_operations,
            control_sqlite_maximum_batch_operations
        );
        apply_control_sqlite_number!(maximum_scan_items, control_sqlite_maximum_scan_items);
        apply_control_sqlite_number!(busy_timeout_seconds, control_sqlite_busy_timeout_seconds);
        apply_control_sqlite_number!(
            transaction_expiration_seconds,
            control_sqlite_transaction_expiration_seconds
        );
        apply_control_sqlite_number!(
            shutdown_timeout_seconds,
            control_sqlite_shutdown_timeout_seconds
        );
        apply_control_sqlite_number!(
            wal_autocheckpoint_pages,
            control_sqlite_wal_autocheckpoint_pages
        );
        apply_control_sqlite_number!(maximum_wal_bytes, control_sqlite_maximum_wal_bytes);
        apply_control_sqlite_number!(
            integrity_interval_seconds,
            control_sqlite_integrity_interval_seconds
        );
        apply_control_sqlite_number!(
            backup_retention_count,
            control_sqlite_backup_retention_count
        );
        apply_control_sqlite_number!(
            disk_warning_free_bytes,
            control_sqlite_disk_warning_free_bytes
        );
        apply_control_sqlite_number!(
            disk_critical_free_bytes,
            control_sqlite_disk_critical_free_bytes
        );
        if let Some(value) = overlay.dependencies.smtp_address {
            self.smtp_address = value;
        }
        if let Some(value) = overlay.developer_registration.enabled {
            self.developer_registration_enabled = value.to_string();
        }
        macro_rules! apply_registration_number {
            ($overlay:ident, $field:ident) => {
                if let Some(value) = overlay.developer_registration.$overlay {
                    self.$field = value.to_string();
                }
            };
        }
        apply_registration_number!(
            verification_lifetime_seconds,
            developer_verification_lifetime_seconds
        );
        apply_registration_number!(
            recovery_lifetime_seconds,
            developer_recovery_lifetime_seconds
        );
        apply_registration_number!(access_lifetime_seconds, developer_access_lifetime_seconds);
        apply_registration_number!(refresh_lifetime_seconds, developer_refresh_lifetime_seconds);
        apply_registration_number!(
            decision_retention_seconds,
            developer_decision_retention_seconds
        );
        apply_registration_number!(rate_window_seconds, developer_rate_window_seconds);
        apply_registration_number!(
            global_requests_per_window,
            developer_global_requests_per_window
        );
        apply_registration_number!(
            source_requests_per_window,
            developer_source_requests_per_window
        );
        apply_registration_number!(
            email_requests_per_window,
            developer_email_requests_per_window
        );
        apply_registration_number!(
            token_attempts_per_window,
            developer_token_attempts_per_window
        );
        apply_registration_number!(
            maximum_parallel_password_work,
            developer_maximum_parallel_password_work
        );
        apply_registration_number!(maximum_pending_outbox, developer_maximum_pending_outbox);
        apply_registration_number!(maximum_outbox_batch, developer_maximum_outbox_batch);
        apply_registration_number!(outbox_lease_seconds, developer_outbox_lease_seconds);
        apply_registration_number!(outbox_maximum_attempts, developer_outbox_maximum_attempts);
        apply_registration_number!(
            outbox_maximum_backoff_seconds,
            developer_outbox_maximum_backoff_seconds
        );
        apply_registration_number!(
            delivered_mail_retention_seconds,
            developer_delivered_mail_retention_seconds
        );
        apply_registration_number!(smtp_port, developer_smtp_port);
        apply_registration_number!(smtp_timeout_seconds, developer_smtp_timeout_seconds);
        if let Some(value) = overlay.developer_registration.smtp_relay_hostname {
            self.developer_smtp_relay_hostname = Some(value);
        }
        if let Some(value) = overlay.developer_registration.smtp_tls_mode {
            self.developer_smtp_tls_mode = value;
        }
        if let Some(value) = overlay.developer_registration.smtp_username {
            self.developer_smtp_username = Some(value);
        }
        if let Some(value) = overlay.developer_registration.smtp_sender {
            self.developer_smtp_sender = Some(value);
        }
        if let Some(value) = overlay.operator_authentication.enabled {
            self.operator_authentication_enabled = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.session_lifetime_seconds {
            self.operator_session_lifetime_seconds = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.mutation_freshness_seconds {
            self.operator_mutation_freshness_seconds = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.attempt_window_seconds {
            self.operator_attempt_window_seconds = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.source_attempts_per_window {
            self.operator_source_attempts_per_window = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.identity_attempts_per_window {
            self.operator_identity_attempts_per_window = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.base_backoff_seconds {
            self.operator_base_backoff_seconds = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.maximum_backoff_seconds {
            self.operator_maximum_backoff_seconds = value.to_string();
        }
        if let Some(value) = overlay.operator_authentication.break_glass_bearer_enabled {
            self.operator_break_glass_bearer_enabled = value.to_string();
        }
        if let Some(value) = overlay.dependencies.object_store_endpoint {
            self.object_store_endpoint = value;
        }
        if let Some(value) = overlay.dependencies.data_plane_address {
            self.data_plane_address = value;
        }
        if let Some(value) = overlay.dependencies.edge_gateway_address {
            self.edge_gateway_address = value;
        }
        if let Some(value) = overlay.dependencies.runtime_supervisor_address {
            self.runtime_supervisor_address = value;
        }
        if let Some(value) = overlay.dependencies.telemetry_query_address {
            self.telemetry_query_address = value;
        }
        if let Some(value) = overlay.dependencies.otlp_address {
            self.otlp_address = value;
        }
        if let Some(value) = overlay.limits.max_request_bytes {
            self.max_request_bytes = value.to_string();
        }
        if let Some(value) = overlay.limits.shutdown_grace_seconds {
            self.shutdown_grace_seconds = value.to_string();
        }
        if let Some(value) = overlay.secrets.internal_auth {
            self.internal_auth_secret_ref = Some(value);
        }
        if let Some(value) = overlay.secrets.object_store_access_key {
            self.object_store_access_key_ref = Some(value);
        }
        if let Some(value) = overlay.secrets.object_store_secret_key {
            self.object_store_secret_key_ref = Some(value);
        }
        if let Some(value) = overlay.secrets.developer_mail_encryption {
            self.developer_mail_encryption_secret_ref = Some(value);
        }
        if let Some(value) = overlay.secrets.developer_smtp_password {
            self.developer_smtp_password_ref = Some(value);
        }
    }

    fn apply_environment(&mut self, loader: &ConfigLoader) -> Result<(), ConfigDiagnostic> {
        apply_string(loader, "MAKO_ENVIRONMENT", &mut self.environment)?;
        apply_string(loader, "MAKO_REGION", &mut self.region)?;
        apply_string(loader, "MAKO_BIND_ADDR", &mut self.bind_address)?;
        apply_optional_string(loader, "MAKO_PUBLIC_URL", &mut self.public_url)?;
        apply_optional_string(loader, "MAKO_TLS_CERT_PATH", &mut self.tls_certificate_path)?;
        apply_optional_string(loader, "MAKO_TLS_KEY_PATH", &mut self.tls_private_key_path)?;
        self.legacy_control_rocksdb_configured |= [
            "MAKO_ROCKSDB_PATH",
            "MAKO_ROCKSDB_MAX_BATCH_OPERATIONS",
            "MAKO_ROCKSDB_MAX_SCAN_ITEMS",
            "MAKO_ROCKSDB_LOCK_TIMEOUT_SECONDS",
            "MAKO_ROCKSDB_TRANSACTION_EXPIRATION_SECONDS",
            "MAKO_ROCKSDB_BACKUP_DESTINATION",
            "MAKO_ROCKSDB_BACKUP_RETENTION_COUNT",
            "MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES",
            "MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES",
        ]
        .iter()
        .any(|name| loader.environment.contains_key(*name));
        apply_string(loader, "MAKO_ROCKSDB_PATH", &mut self.rocksdb_path)?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_MAX_BATCH_OPERATIONS",
            &mut self.maximum_batch_operations,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_MAX_SCAN_ITEMS",
            &mut self.maximum_scan_items,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_LOCK_TIMEOUT_SECONDS",
            &mut self.transaction_lock_timeout_seconds,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_TRANSACTION_EXPIRATION_SECONDS",
            &mut self.transaction_expiration_seconds,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_BACKUP_DESTINATION",
            &mut self.backup_destination,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_BACKUP_RETENTION_COUNT",
            &mut self.backup_retention_count,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_DISK_WARNING_FREE_BYTES",
            &mut self.disk_warning_free_bytes,
        )?;
        apply_string(
            loader,
            "MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES",
            &mut self.disk_critical_free_bytes,
        )?;
        for (name, target) in [
            ("MAKO_CONTROL_SQLITE_PATH", &mut self.control_sqlite_path),
            (
                "MAKO_CONTROL_SQLITE_LOCK_PATH",
                &mut self.control_sqlite_lock_path,
            ),
            (
                "MAKO_CONTROL_SQLITE_IDENTITY",
                &mut self.control_sqlite_identity,
            ),
            (
                "MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE",
                &mut self.control_sqlite_migration_workspace,
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_STAGING",
                &mut self.control_sqlite_backup_staging,
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_PUBLISH",
                &mut self.control_sqlite_backup_publish,
            ),
            (
                "MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE",
                &mut self.control_sqlite_restore_workspace,
            ),
            (
                "MAKO_CONTROL_SQLITE_RESERVE_PATH",
                &mut self.control_sqlite_reserve_path,
            ),
            (
                "MAKO_CONTROL_SQLITE_MAX_BATCH_OPERATIONS",
                &mut self.control_sqlite_maximum_batch_operations,
            ),
            (
                "MAKO_CONTROL_SQLITE_MAX_SCAN_ITEMS",
                &mut self.control_sqlite_maximum_scan_items,
            ),
            (
                "MAKO_CONTROL_SQLITE_BUSY_TIMEOUT_SECONDS",
                &mut self.control_sqlite_busy_timeout_seconds,
            ),
            (
                "MAKO_CONTROL_SQLITE_TRANSACTION_EXPIRATION_SECONDS",
                &mut self.control_sqlite_transaction_expiration_seconds,
            ),
            (
                "MAKO_CONTROL_SQLITE_SHUTDOWN_TIMEOUT_SECONDS",
                &mut self.control_sqlite_shutdown_timeout_seconds,
            ),
            (
                "MAKO_CONTROL_SQLITE_WAL_AUTOCHECKPOINT_PAGES",
                &mut self.control_sqlite_wal_autocheckpoint_pages,
            ),
            (
                "MAKO_CONTROL_SQLITE_MAX_WAL_BYTES",
                &mut self.control_sqlite_maximum_wal_bytes,
            ),
            (
                "MAKO_CONTROL_SQLITE_INTEGRITY_INTERVAL_SECONDS",
                &mut self.control_sqlite_integrity_interval_seconds,
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_RETENTION_COUNT",
                &mut self.control_sqlite_backup_retention_count,
            ),
            (
                "MAKO_CONTROL_SQLITE_DISK_WARNING_FREE_BYTES",
                &mut self.control_sqlite_disk_warning_free_bytes,
            ),
            (
                "MAKO_CONTROL_SQLITE_DISK_CRITICAL_FREE_BYTES",
                &mut self.control_sqlite_disk_critical_free_bytes,
            ),
        ] {
            apply_string(loader, name, target)?;
        }
        apply_string(loader, "MAKO_SMTP_ENDPOINT", &mut self.smtp_address)?;
        apply_string(
            loader,
            "MAKO_DEVELOPER_REGISTRATION_ENABLED",
            &mut self.developer_registration_enabled,
        )?;
        for (name, target) in [
            (
                "MAKO_DEVELOPER_VERIFICATION_TTL_SECONDS",
                &mut self.developer_verification_lifetime_seconds,
            ),
            (
                "MAKO_DEVELOPER_RECOVERY_TTL_SECONDS",
                &mut self.developer_recovery_lifetime_seconds,
            ),
            (
                "MAKO_DEVELOPER_ACCESS_TTL_SECONDS",
                &mut self.developer_access_lifetime_seconds,
            ),
            (
                "MAKO_DEVELOPER_REFRESH_TTL_SECONDS",
                &mut self.developer_refresh_lifetime_seconds,
            ),
            (
                "MAKO_DEVELOPER_DECISION_RETENTION_SECONDS",
                &mut self.developer_decision_retention_seconds,
            ),
            (
                "MAKO_DEVELOPER_RATE_WINDOW_SECONDS",
                &mut self.developer_rate_window_seconds,
            ),
            (
                "MAKO_DEVELOPER_GLOBAL_RATE_LIMIT",
                &mut self.developer_global_requests_per_window,
            ),
            (
                "MAKO_DEVELOPER_SOURCE_RATE_LIMIT",
                &mut self.developer_source_requests_per_window,
            ),
            (
                "MAKO_DEVELOPER_EMAIL_RATE_LIMIT",
                &mut self.developer_email_requests_per_window,
            ),
            (
                "MAKO_DEVELOPER_TOKEN_RATE_LIMIT",
                &mut self.developer_token_attempts_per_window,
            ),
            (
                "MAKO_DEVELOPER_MAX_PASSWORD_WORK",
                &mut self.developer_maximum_parallel_password_work,
            ),
            (
                "MAKO_DEVELOPER_MAX_PENDING_OUTBOX",
                &mut self.developer_maximum_pending_outbox,
            ),
            (
                "MAKO_DEVELOPER_OUTBOX_BATCH",
                &mut self.developer_maximum_outbox_batch,
            ),
            (
                "MAKO_DEVELOPER_OUTBOX_LEASE_SECONDS",
                &mut self.developer_outbox_lease_seconds,
            ),
            (
                "MAKO_DEVELOPER_OUTBOX_MAX_ATTEMPTS",
                &mut self.developer_outbox_maximum_attempts,
            ),
            (
                "MAKO_DEVELOPER_OUTBOX_MAX_BACKOFF_SECONDS",
                &mut self.developer_outbox_maximum_backoff_seconds,
            ),
            (
                "MAKO_DEVELOPER_MAIL_RETENTION_SECONDS",
                &mut self.developer_delivered_mail_retention_seconds,
            ),
            ("MAKO_DEVELOPER_SMTP_PORT", &mut self.developer_smtp_port),
            (
                "MAKO_DEVELOPER_SMTP_TIMEOUT_SECONDS",
                &mut self.developer_smtp_timeout_seconds,
            ),
        ] {
            apply_string(loader, name, target)?;
        }
        apply_optional_string(
            loader,
            "MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME",
            &mut self.developer_smtp_relay_hostname,
        )?;
        apply_string(
            loader,
            "MAKO_DEVELOPER_SMTP_TLS_MODE",
            &mut self.developer_smtp_tls_mode,
        )?;
        apply_optional_string(
            loader,
            "MAKO_DEVELOPER_SMTP_USERNAME",
            &mut self.developer_smtp_username,
        )?;
        apply_optional_string(
            loader,
            "MAKO_DEVELOPER_SMTP_SENDER",
            &mut self.developer_smtp_sender,
        )?;
        for (name, target) in [
            (
                "MAKO_OPERATOR_PASSWORD_AUTH_ENABLED",
                &mut self.operator_authentication_enabled,
            ),
            (
                "MAKO_OPERATOR_SESSION_TTL_SECONDS",
                &mut self.operator_session_lifetime_seconds,
            ),
            (
                "MAKO_OPERATOR_MUTATION_FRESHNESS_SECONDS",
                &mut self.operator_mutation_freshness_seconds,
            ),
            (
                "MAKO_OPERATOR_ATTEMPT_WINDOW_SECONDS",
                &mut self.operator_attempt_window_seconds,
            ),
            (
                "MAKO_OPERATOR_SOURCE_RATE_LIMIT",
                &mut self.operator_source_attempts_per_window,
            ),
            (
                "MAKO_OPERATOR_IDENTITY_RATE_LIMIT",
                &mut self.operator_identity_attempts_per_window,
            ),
            (
                "MAKO_OPERATOR_BASE_BACKOFF_SECONDS",
                &mut self.operator_base_backoff_seconds,
            ),
            (
                "MAKO_OPERATOR_MAX_BACKOFF_SECONDS",
                &mut self.operator_maximum_backoff_seconds,
            ),
            (
                "MAKO_OPERATOR_BREAK_GLASS_BEARER_ENABLED",
                &mut self.operator_break_glass_bearer_enabled,
            ),
        ] {
            apply_string(loader, name, target)?;
        }
        apply_string(
            loader,
            "MAKO_OBJECT_STORE_ENDPOINT",
            &mut self.object_store_endpoint,
        )?;
        apply_string(
            loader,
            "MAKO_DATA_PLANE_ENDPOINT",
            &mut self.data_plane_address,
        )?;
        apply_string(
            loader,
            "MAKO_EDGE_GATEWAY_ENDPOINT",
            &mut self.edge_gateway_address,
        )?;
        apply_string(
            loader,
            "MAKO_RUNTIME_SUPERVISOR_ENDPOINT",
            &mut self.runtime_supervisor_address,
        )?;
        apply_string(
            loader,
            "MAKO_TELEMETRY_QUERY_ENDPOINT",
            &mut self.telemetry_query_address,
        )?;
        apply_string(loader, "MAKO_OTLP_ENDPOINT", &mut self.otlp_address)?;
        apply_string(
            loader,
            "MAKO_MAX_REQUEST_BYTES",
            &mut self.max_request_bytes,
        )?;
        apply_string(
            loader,
            "MAKO_SHUTDOWN_GRACE_SECONDS",
            &mut self.shutdown_grace_seconds,
        )?;
        apply_optional_string(
            loader,
            "MAKO_INTERNAL_AUTH_SECRET_REF",
            &mut self.internal_auth_secret_ref,
        )?;
        apply_optional_string(
            loader,
            "MAKO_OBJECT_STORE_ACCESS_KEY_REF",
            &mut self.object_store_access_key_ref,
        )?;
        apply_optional_string(
            loader,
            "MAKO_OBJECT_STORE_SECRET_KEY_REF",
            &mut self.object_store_secret_key_ref,
        )?;
        apply_optional_string(
            loader,
            "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
            &mut self.developer_mail_encryption_secret_ref,
        )?;
        apply_optional_string(
            loader,
            "MAKO_DEVELOPER_SMTP_PASSWORD_REF",
            &mut self.developer_smtp_password_ref,
        )?;
        Ok(())
    }

    fn validate(
        self,
        service: ServiceKind,
        loader: &ConfigLoader,
    ) -> Result<ServiceConfig, ConfigDiagnostic> {
        let environment = self.environment.parse().map_err(|()| {
            invalid(
                "deployment.environment",
                "must be local, development, staging, or production",
            )
        })?;
        validate_region(&self.region)?;
        let bind_address = parse_socket(&self.bind_address, "server.bind_address")?;
        let public_url = self
            .public_url
            .as_deref()
            .map(|value| parse_http_url(value, "server.public_url"))
            .transpose()?;
        if matches!(
            environment,
            DeploymentEnvironment::Staging | DeploymentEnvironment::Production
        ) && public_url
            .as_ref()
            .is_none_or(|url| url.scheme() != "https")
        {
            return Err(invalid(
                "server.public_url",
                "must be an HTTPS URL in staging and production",
            ));
        }
        let tls_certificate_path =
            optional_path(self.tls_certificate_path, "server.tls_certificate_path")?;
        let tls_private_key_path =
            optional_path(self.tls_private_key_path, "server.tls_private_key_path")?;
        if tls_certificate_path.is_some() != tls_private_key_path.is_some() {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "server.tls",
                "certificate and private-key paths must be configured together",
            ));
        }
        if let Some(path) = tls_certificate_path.as_deref() {
            require_regular_file(path, "server.tls_certificate_path")?;
        }
        if let Some(path) = tls_private_key_path.as_deref() {
            require_regular_file(path, "server.tls_private_key_path")?;
        }
        let rocksdb_path = required_path(self.rocksdb_path, "storage.rocksdb_path")?;
        let backup_destination =
            required_path(self.backup_destination, "storage.backup_destination")?;
        if matches!(environment, DeploymentEnvironment::Production)
            && service != ServiceKind::ControlPlane
        {
            validate_production_path(&rocksdb_path, "storage.rocksdb_path")?;
            validate_production_path(&backup_destination, "storage.backup_destination")?;
            if paths_overlap(&rocksdb_path, &backup_destination) {
                return Err(invalid(
                    "storage.backup_destination",
                    "must be separate from the production database path",
                ));
            }
        }
        let maximum_batch_operations = parse_nonzero_usize(
            &self.maximum_batch_operations,
            "storage.maximum_batch_operations",
            MAX_STORAGE_OPERATIONS,
        )?;
        let maximum_scan_items = parse_nonzero_usize(
            &self.maximum_scan_items,
            "storage.maximum_scan_items",
            MAX_STORAGE_OPERATIONS,
        )?;
        let transaction_lock_timeout_seconds = parse_u64(
            &self.transaction_lock_timeout_seconds,
            "storage.transaction_lock_timeout_seconds",
            1,
            MAX_TRANSACTION_SECONDS,
        )?;
        let transaction_expiration_seconds = parse_u64(
            &self.transaction_expiration_seconds,
            "storage.transaction_expiration_seconds",
            transaction_lock_timeout_seconds,
            MAX_TRANSACTION_SECONDS,
        )?;
        let backup_retention_count = parse_u64(
            &self.backup_retention_count,
            "storage.backup_retention_count",
            1,
            365,
        )?;
        let disk_warning_free_bytes = parse_u64(
            &self.disk_warning_free_bytes,
            "storage.disk_warning_free_bytes",
            1,
            u64::MAX,
        )?;
        let disk_critical_free_bytes = parse_u64(
            &self.disk_critical_free_bytes,
            "storage.disk_critical_free_bytes",
            1,
            u64::MAX,
        )?;
        if disk_warning_free_bytes <= disk_critical_free_bytes {
            return Err(invalid(
                "storage.disk_warning_free_bytes",
                "must be greater than the critical free-byte threshold",
            ));
        }
        if environment == DeploymentEnvironment::Production
            && service.owns_state()
            && disk_critical_free_bytes < MIN_PRODUCTION_DISK_RESERVE_BYTES
        {
            return Err(invalid(
                "storage.disk_critical_free_bytes",
                "production reserve must be at least 64 MiB",
            ));
        }
        let rocksdb = ProductionRocksDbSettings {
            path: rocksdb_path,
            maximum_batch_operations,
            maximum_scan_items,
            transaction_lock_timeout: Duration::from_secs(transaction_lock_timeout_seconds),
            transaction_expiration: Duration::from_secs(transaction_expiration_seconds),
            backup_destination,
            backup_retention_count: NonZeroU32::new(
                u32::try_from(backup_retention_count).expect("retention is at most 365"),
            )
            .expect("retention is positive"),
            disk_warning_free_bytes,
            disk_critical_free_bytes,
        };
        let control_sqlite = if service == ServiceKind::ControlPlane {
            if environment == DeploymentEnvironment::Production
                && self.legacy_control_rocksdb_configured
            {
                return Err(invalid(
                    "storage.rocksdb_path",
                    "legacy RocksDB configuration is not accepted by the production control plane",
                ));
            }
            let database_path = resolve_local_storage_path(
                required_path(self.control_sqlite_path, "control_storage.database_path")?,
                "control_storage.database_path",
                environment,
            )?;
            let lock_path = resolve_local_storage_path(
                required_path(self.control_sqlite_lock_path, "control_storage.lock_path")?,
                "control_storage.lock_path",
                environment,
            )?;
            let migration_workspace = resolve_local_storage_path(
                required_path(
                    self.control_sqlite_migration_workspace,
                    "control_storage.migration_workspace",
                )?,
                "control_storage.migration_workspace",
                environment,
            )?;
            let backup_staging = resolve_local_storage_path(
                required_path(
                    self.control_sqlite_backup_staging,
                    "control_storage.backup_staging",
                )?,
                "control_storage.backup_staging",
                environment,
            )?;
            let backup_publish = resolve_local_storage_path(
                required_path(
                    self.control_sqlite_backup_publish,
                    "control_storage.backup_publish",
                )?,
                "control_storage.backup_publish",
                environment,
            )?;
            let restore_workspace = resolve_local_storage_path(
                required_path(
                    self.control_sqlite_restore_workspace,
                    "control_storage.restore_workspace",
                )?,
                "control_storage.restore_workspace",
                environment,
            )?;
            let reserve_path = resolve_local_storage_path(
                required_path(
                    self.control_sqlite_reserve_path,
                    "control_storage.reserve_path",
                )?,
                "control_storage.reserve_path",
                environment,
            )?;
            validate_control_identity(&self.control_sqlite_identity)?;
            let maximum_batch_operations = parse_nonzero_usize(
                &self.control_sqlite_maximum_batch_operations,
                "control_storage.maximum_batch_operations",
                MAX_STORAGE_OPERATIONS,
            )?;
            let maximum_scan_items = parse_nonzero_usize(
                &self.control_sqlite_maximum_scan_items,
                "control_storage.maximum_scan_items",
                MAX_STORAGE_OPERATIONS,
            )?;
            let busy_timeout_seconds = parse_u64(
                &self.control_sqlite_busy_timeout_seconds,
                "control_storage.busy_timeout_seconds",
                1,
                60,
            )?;
            let transaction_expiration_seconds = parse_u64(
                &self.control_sqlite_transaction_expiration_seconds,
                "control_storage.transaction_expiration_seconds",
                busy_timeout_seconds,
                MAX_TRANSACTION_SECONDS,
            )?;
            let shutdown_timeout_seconds = parse_u64(
                &self.control_sqlite_shutdown_timeout_seconds,
                "control_storage.shutdown_timeout_seconds",
                1,
                300,
            )?;
            let wal_autocheckpoint_pages = parse_nonzero_usize(
                &self.control_sqlite_wal_autocheckpoint_pages,
                "control_storage.wal_autocheckpoint_pages",
                1_000_000,
            )?;
            let maximum_wal_bytes = parse_u64(
                &self.control_sqlite_maximum_wal_bytes,
                "control_storage.maximum_wal_bytes",
                1,
                1024 * 1024 * 1024 * 1024,
            )?;
            let integrity_interval_seconds = parse_u64(
                &self.control_sqlite_integrity_interval_seconds,
                "control_storage.integrity_interval_seconds",
                1,
                24 * 60 * 60,
            )?;
            let backup_retention_count = parse_u64(
                &self.control_sqlite_backup_retention_count,
                "control_storage.backup_retention_count",
                1,
                365,
            )?;
            let sqlite_disk_warning_free_bytes = parse_u64(
                &self.control_sqlite_disk_warning_free_bytes,
                "control_storage.disk_warning_free_bytes",
                1,
                u64::MAX,
            )?;
            let sqlite_disk_critical_free_bytes = parse_u64(
                &self.control_sqlite_disk_critical_free_bytes,
                "control_storage.disk_critical_free_bytes",
                1,
                u64::MAX,
            )?;
            if sqlite_disk_warning_free_bytes <= sqlite_disk_critical_free_bytes {
                return Err(invalid(
                    "control_storage.disk_warning_free_bytes",
                    "must be greater than the critical free-byte threshold",
                ));
            }

            let paths = [
                ("control_storage.database_path", database_path.as_path()),
                ("control_storage.lock_path", lock_path.as_path()),
                (
                    "control_storage.migration_workspace",
                    migration_workspace.as_path(),
                ),
                ("control_storage.backup_staging", backup_staging.as_path()),
                ("control_storage.backup_publish", backup_publish.as_path()),
                (
                    "control_storage.restore_workspace",
                    restore_workspace.as_path(),
                ),
                ("control_storage.reserve_path", reserve_path.as_path()),
            ];
            if environment == DeploymentEnvironment::Production {
                for (field, path) in paths {
                    validate_production_path(path, field)?;
                    validate_no_symlink_components(path, field)?;
                }
                if sqlite_disk_critical_free_bytes < MIN_PRODUCTION_DISK_RESERVE_BYTES {
                    return Err(invalid(
                        "control_storage.disk_critical_free_bytes",
                        "production reserve must be at least 64 MiB",
                    ));
                }
            }
            for first in 0..paths.len() {
                for second in (first + 1)..paths.len() {
                    if paths_overlap(paths[first].1, paths[second].1) {
                        return Err(invalid(
                            paths[second].0,
                            "must not overlap another control-storage path",
                        ));
                    }
                }
            }

            Some(ProductionSqliteSettings {
                database_path,
                lock_path,
                database_identity: self.control_sqlite_identity,
                migration_workspace,
                backup_staging,
                backup_publish,
                restore_workspace,
                reserve_path,
                maximum_batch_operations,
                maximum_scan_items,
                busy_timeout: Duration::from_secs(busy_timeout_seconds),
                transaction_expiration: Duration::from_secs(transaction_expiration_seconds),
                shutdown_timeout: Duration::from_secs(shutdown_timeout_seconds),
                wal_autocheckpoint_pages,
                maximum_wal_bytes,
                integrity_interval: Duration::from_secs(integrity_interval_seconds),
                backup_retention_count: NonZeroU32::new(
                    u32::try_from(backup_retention_count).expect("retention is at most 365"),
                )
                .expect("retention is positive"),
                disk_warning_free_bytes: sqlite_disk_warning_free_bytes,
                disk_critical_free_bytes: sqlite_disk_critical_free_bytes,
            })
        } else {
            None
        };
        let smtp_address = parse_socket(&self.smtp_address, "dependencies.smtp_address")?;
        let registration_enabled = parse_boolean(
            &self.developer_registration_enabled,
            "developer_registration.enabled",
        )?;
        macro_rules! registration_u64 {
            ($field:ident, $name:literal, $minimum:expr, $maximum:expr) => {
                parse_u64(
                    &self.$field,
                    concat!("developer_registration.", $name),
                    $minimum,
                    $maximum,
                )?
            };
        }
        let verification_lifetime_seconds = registration_u64!(
            developer_verification_lifetime_seconds,
            "verification_lifetime_seconds",
            60,
            7 * 24 * 60 * 60
        );
        let recovery_lifetime_seconds = registration_u64!(
            developer_recovery_lifetime_seconds,
            "recovery_lifetime_seconds",
            60,
            24 * 60 * 60
        );
        let access_lifetime_seconds = registration_u64!(
            developer_access_lifetime_seconds,
            "access_lifetime_seconds",
            60,
            60 * 60
        );
        let refresh_lifetime_seconds = registration_u64!(
            developer_refresh_lifetime_seconds,
            "refresh_lifetime_seconds",
            access_lifetime_seconds + 1,
            90 * 24 * 60 * 60
        );
        let decision_retention_seconds = registration_u64!(
            developer_decision_retention_seconds,
            "decision_retention_seconds",
            24 * 60 * 60,
            365 * 24 * 60 * 60
        );
        let rate_window_seconds = registration_u64!(
            developer_rate_window_seconds,
            "rate_window_seconds",
            1,
            24 * 60 * 60
        );
        let global_requests_per_window = registration_u64!(
            developer_global_requests_per_window,
            "global_requests_per_window",
            1,
            1_000_000
        );
        let source_requests_per_window = registration_u64!(
            developer_source_requests_per_window,
            "source_requests_per_window",
            1,
            1_000_000
        );
        let email_requests_per_window = registration_u64!(
            developer_email_requests_per_window,
            "email_requests_per_window",
            1,
            1_000_000
        );
        let token_attempts_per_window = registration_u64!(
            developer_token_attempts_per_window,
            "token_attempts_per_window",
            1,
            1_000_000
        );
        let maximum_parallel_password_work = registration_u64!(
            developer_maximum_parallel_password_work,
            "maximum_parallel_password_work",
            1,
            256
        );
        let maximum_pending_outbox = registration_u64!(
            developer_maximum_pending_outbox,
            "maximum_pending_outbox",
            1,
            9_999
        );
        let maximum_outbox_batch = registration_u64!(
            developer_maximum_outbox_batch,
            "maximum_outbox_batch",
            1,
            100
        );
        let outbox_lease_seconds = registration_u64!(
            developer_outbox_lease_seconds,
            "outbox_lease_seconds",
            1,
            60 * 60
        );
        let outbox_maximum_attempts = registration_u64!(
            developer_outbox_maximum_attempts,
            "outbox_maximum_attempts",
            1,
            100
        );
        let outbox_maximum_backoff_seconds = registration_u64!(
            developer_outbox_maximum_backoff_seconds,
            "outbox_maximum_backoff_seconds",
            1,
            7 * 24 * 60 * 60
        );
        let delivered_mail_retention_seconds = registration_u64!(
            developer_delivered_mail_retention_seconds,
            "delivered_mail_retention_seconds",
            60,
            365 * 24 * 60 * 60
        );
        let smtp_port = registration_u64!(developer_smtp_port, "smtp_port", 1, u16::MAX.into());
        let smtp_timeout_seconds = registration_u64!(
            developer_smtp_timeout_seconds,
            "smtp_timeout_seconds",
            1,
            120
        );
        let mail_encryption_secret = self
            .developer_mail_encryption_secret_ref
            .as_deref()
            .map(|reference| resolve_secret(reference, loader, "secrets.developer_mail_encryption"))
            .transpose()?;
        if let Some(secret) = &mail_encryption_secret {
            validate_secret_shape(secret, "secrets.developer_mail_encryption", 32, 4_096)?;
        }
        let smtp_password = self
            .developer_smtp_password_ref
            .as_deref()
            .map(|reference| resolve_secret(reference, loader, "secrets.developer_smtp_password"))
            .transpose()?;
        if let Some(secret) = &smtp_password {
            validate_secret_shape(secret, "secrets.developer_smtp_password", 16, 4_096)?;
        }
        let smtp_tls_mode = match self.developer_smtp_tls_mode.as_str() {
            "wrapper" => SmtpTlsMode::Wrapper,
            "starttls" => SmtpTlsMode::StartTls,
            "plaintext" => SmtpTlsMode::Plaintext,
            _ => {
                return Err(invalid(
                    "developer_registration.smtp_tls_mode",
                    "must be wrapper, starttls, or plaintext",
                ));
            }
        };
        // Plaintext exists for a loopback mailpit and the smoke suite's stub.
        // Production mail leaves the host, so it is refused there outright
        // rather than degraded to a warning.
        if smtp_tls_mode == SmtpTlsMode::Plaintext
            && environment == DeploymentEnvironment::Production
        {
            return Err(invalid(
                "developer_registration.smtp_tls_mode",
                "plaintext SMTP is refused in production; use starttls or wrapper",
            ));
        }
        let relay_present = self.developer_smtp_relay_hostname.is_some();
        let sender_present = self.developer_smtp_sender.is_some();
        let username_present = self.developer_smtp_username.is_some();
        let password_present = smtp_password.is_some();
        let any_present = relay_present || sender_present || username_present || password_present;
        let complete = if smtp_tls_mode == SmtpTlsMode::Plaintext {
            relay_present && sender_present && username_present == password_present
        } else {
            relay_present && sender_present && username_present && password_present
        };
        if any_present && !complete {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "developer_registration.smtp",
                if smtp_tls_mode == SmtpTlsMode::Plaintext {
                    "relay hostname and sender are required together, as are username and password secret"
                } else {
                    "relay hostname, username, sender, and password secret are required together"
                },
            ));
        }
        let smtp = match (
            self.developer_smtp_relay_hostname,
            self.developer_smtp_sender,
        ) {
            (Some(relay_hostname), Some(sender)) => {
                validate_smtp_text(
                    &relay_hostname,
                    "developer_registration.smtp_relay_hostname",
                    1,
                    253,
                )?;
                if let Some(username) = &self.developer_smtp_username {
                    validate_smtp_text(username, "developer_registration.smtp_username", 1, 512)?;
                }
                validate_smtp_text(&sender, "developer_registration.smtp_sender", 3, 512)?;
                Some(AuthenticatedSmtpSettings {
                    relay_hostname,
                    port: u16::try_from(smtp_port).expect("SMTP port is bounded"),
                    tls_mode: smtp_tls_mode,
                    username: self.developer_smtp_username,
                    password: smtp_password,
                    sender,
                    timeout: Duration::from_secs(smtp_timeout_seconds),
                })
            }
            (None, None) => None,
            _ => unreachable!("SMTP presence was validated"),
        };
        if registration_enabled && (mail_encryption_secret.is_none() || smtp.is_none()) {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "developer_registration",
                "mail encryption and an SMTP relay are required when registration is enabled",
            ));
        }
        let developer_registration = DeveloperRegistrationSettings {
            enabled: registration_enabled,
            verification_lifetime: Duration::from_secs(verification_lifetime_seconds),
            recovery_lifetime: Duration::from_secs(recovery_lifetime_seconds),
            access_lifetime: Duration::from_secs(access_lifetime_seconds),
            refresh_lifetime: Duration::from_secs(refresh_lifetime_seconds),
            decision_retention: Duration::from_secs(decision_retention_seconds),
            rate_window: Duration::from_secs(rate_window_seconds),
            global_requests_per_window: u32::try_from(global_requests_per_window)
                .expect("rate limit is bounded"),
            source_requests_per_window: u32::try_from(source_requests_per_window)
                .expect("rate limit is bounded"),
            email_requests_per_window: u32::try_from(email_requests_per_window)
                .expect("rate limit is bounded"),
            token_attempts_per_window: u32::try_from(token_attempts_per_window)
                .expect("rate limit is bounded"),
            maximum_parallel_password_work: usize::try_from(maximum_parallel_password_work)
                .expect("password concurrency is bounded"),
            maximum_pending_outbox: usize::try_from(maximum_pending_outbox)
                .expect("outbox depth is bounded"),
            maximum_outbox_batch: usize::try_from(maximum_outbox_batch)
                .expect("outbox batch is bounded"),
            outbox_lease: Duration::from_secs(outbox_lease_seconds),
            outbox_maximum_attempts: u32::try_from(outbox_maximum_attempts)
                .expect("outbox attempts are bounded"),
            outbox_maximum_backoff: Duration::from_secs(outbox_maximum_backoff_seconds),
            delivered_mail_retention: Duration::from_secs(delivered_mail_retention_seconds),
            mail_encryption_secret,
            smtp,
        };
        let operator_enabled = parse_boolean(
            &self.operator_authentication_enabled,
            "operator_authentication.enabled",
        )?;
        let operator_break_glass_bearer_enabled = parse_boolean(
            &self.operator_break_glass_bearer_enabled,
            "operator_authentication.break_glass_bearer_enabled",
        )?;
        let operator_session_lifetime_seconds = parse_u64(
            &self.operator_session_lifetime_seconds,
            "operator_authentication.session_lifetime_seconds",
            60,
            60 * 60,
        )?;
        let operator_mutation_freshness_seconds = parse_u64(
            &self.operator_mutation_freshness_seconds,
            "operator_authentication.mutation_freshness_seconds",
            1,
            5 * 60,
        )?;
        let operator_attempt_window_seconds = parse_u64(
            &self.operator_attempt_window_seconds,
            "operator_authentication.attempt_window_seconds",
            1,
            24 * 60 * 60,
        )?;
        let operator_source_attempts_per_window = parse_u64(
            &self.operator_source_attempts_per_window,
            "operator_authentication.source_attempts_per_window",
            1,
            1_000_000,
        )?;
        let operator_identity_attempts_per_window = parse_u64(
            &self.operator_identity_attempts_per_window,
            "operator_authentication.identity_attempts_per_window",
            1,
            1_000_000,
        )?;
        let operator_base_backoff_seconds = parse_u64(
            &self.operator_base_backoff_seconds,
            "operator_authentication.base_backoff_seconds",
            1,
            operator_attempt_window_seconds,
        )?;
        let operator_maximum_backoff_seconds = parse_u64(
            &self.operator_maximum_backoff_seconds,
            "operator_authentication.maximum_backoff_seconds",
            operator_base_backoff_seconds,
            operator_attempt_window_seconds,
        )?;
        let operator_authentication = OperatorAuthenticationSettings {
            enabled: operator_enabled,
            session_lifetime: Duration::from_secs(operator_session_lifetime_seconds),
            mutation_freshness: Duration::from_secs(operator_mutation_freshness_seconds),
            attempt_window: Duration::from_secs(operator_attempt_window_seconds),
            source_attempts_per_window: u32::try_from(operator_source_attempts_per_window)
                .expect("operator rate limit is bounded"),
            identity_attempts_per_window: u32::try_from(operator_identity_attempts_per_window)
                .expect("operator rate limit is bounded"),
            base_backoff: Duration::from_secs(operator_base_backoff_seconds),
            maximum_backoff: Duration::from_secs(operator_maximum_backoff_seconds),
            cookie_name: "__Secure-mako_operator".to_owned(),
            cookie_secure: true,
            cookie_http_only: true,
            cookie_same_site: "Strict".to_owned(),
            cookie_path: "/v1".to_owned(),
            break_glass_bearer_enabled: operator_break_glass_bearer_enabled,
        };
        let object_store_endpoint = parse_http_url(
            &self.object_store_endpoint,
            "dependencies.object_store_endpoint",
        )?;
        // The control plane reaches the data plane over internal RPC, which is
        // loopback-only by design: the two run on one node and the transport
        // carries no network authentication beyond the shared secret.
        let data_plane_address =
            parse_socket(&self.data_plane_address, "dependencies.data_plane_address")?;
        if service == ServiceKind::ControlPlane && !data_plane_address.ip().is_loopback() {
            return Err(invalid(
                "dependencies.data_plane_address",
                "control-plane data plane must use a loopback address",
            ));
        }
        // The scheduler invokes functions through the edge gateway over the
        // same loopback-only internal RPC.
        let edge_gateway_address = parse_socket(
            &self.edge_gateway_address,
            "dependencies.edge_gateway_address",
        )?;
        if service == ServiceKind::ControlPlane && !edge_gateway_address.ip().is_loopback() {
            return Err(invalid(
                "dependencies.edge_gateway_address",
                "control-plane edge gateway must use a loopback address",
            ));
        }
        let runtime_supervisor_address = parse_socket(
            &self.runtime_supervisor_address,
            "dependencies.runtime_supervisor_address",
        )?;
        if service == ServiceKind::ControlPlane && !runtime_supervisor_address.ip().is_loopback() {
            return Err(invalid(
                "dependencies.runtime_supervisor_address",
                "control-plane runtime supervisor must use a loopback address",
            ));
        }
        let telemetry_query_address = parse_socket(
            &self.telemetry_query_address,
            "dependencies.telemetry_query_address",
        )?;
        if service == ServiceKind::ControlPlane && !telemetry_query_address.ip().is_loopback() {
            return Err(invalid(
                "dependencies.telemetry_query_address",
                "control-plane telemetry queries must use a loopback address",
            ));
        }
        let otlp_address = parse_socket(&self.otlp_address, "dependencies.otlp_address")?;
        let max_request_bytes = parse_u64(
            &self.max_request_bytes,
            "limits.max_request_bytes",
            1,
            MAX_REQUEST_BYTES,
        )?;
        let shutdown_grace_seconds = parse_u64(
            &self.shutdown_grace_seconds,
            "limits.shutdown_grace_seconds",
            1,
            300,
        )?;
        let internal_auth_secret = self
            .internal_auth_secret_ref
            .as_deref()
            .map(|reference| resolve_secret(reference, loader, "secrets.internal_auth"))
            .transpose()?;
        if environment == DeploymentEnvironment::Production && internal_auth_secret.is_none() {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "secrets.internal_auth",
                "a secret reference is required in production",
            ));
        }
        let object_store_access_key = self
            .object_store_access_key_ref
            .as_deref()
            .map(|reference| resolve_secret(reference, loader, "secrets.object_store_access_key"))
            .transpose()?;
        let object_store_secret_key = self
            .object_store_secret_key_ref
            .as_deref()
            .map(|reference| resolve_secret(reference, loader, "secrets.object_store_secret_key"))
            .transpose()?;
        if object_store_access_key.is_some() != object_store_secret_key.is_some() {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "secrets.object_store",
                "access-key and secret-key references must be configured together",
            ));
        }
        if environment == DeploymentEnvironment::Production
            && matches!(service, ServiceKind::ControlPlane | ServiceKind::DataPlane)
            && object_store_access_key.is_none()
        {
            return Err(ConfigDiagnostic::new(
                ConfigErrorCode::MissingValue,
                "secrets.object_store",
                "authenticated object storage is required for the production control plane and data plane",
            ));
        }
        if let Some(value) = object_store_access_key.as_ref() {
            validate_secret_shape(value, "secrets.object_store_access_key", 3, 128)?;
        }
        if let Some(value) = object_store_secret_key.as_ref() {
            validate_secret_shape(value, "secrets.object_store_secret_key", 16, 1024)?;
        }
        Ok(ServiceConfig {
            service,
            environment,
            region: self.region,
            bind_address,
            public_url,
            tls_certificate_path,
            tls_private_key_path,
            rocksdb,
            control_sqlite,
            smtp_address,
            developer_registration,
            operator_authentication,
            object_store_endpoint,
            data_plane_address,
            edge_gateway_address,
            runtime_supervisor_address,
            telemetry_query_address,
            otlp_address,
            max_request_bytes,
            shutdown_grace: Duration::from_secs(shutdown_grace_seconds),
            internal_auth_secret,
            object_store_access_key,
            object_store_secret_key,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigOverlay {
    deployment: DeploymentOverlay,
    server: ServerOverlay,
    storage: StorageOverlay,
    control_storage: ControlSqliteOverlay,
    dependencies: DependenciesOverlay,
    developer_registration: DeveloperRegistrationOverlay,
    operator_authentication: OperatorAuthenticationOverlay,
    limits: LimitsOverlay,
    secrets: SecretsOverlay,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DeploymentOverlay {
    environment: Option<String>,
    region: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerOverlay {
    bind_address: Option<String>,
    public_url: Option<String>,
    tls_certificate_path: Option<String>,
    tls_private_key_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StorageOverlay {
    rocksdb_path: Option<String>,
    maximum_batch_operations: Option<u64>,
    maximum_scan_items: Option<u64>,
    transaction_lock_timeout_seconds: Option<u64>,
    transaction_expiration_seconds: Option<u64>,
    backup_destination: Option<String>,
    backup_retention_count: Option<u64>,
    disk_warning_free_bytes: Option<u64>,
    disk_critical_free_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ControlSqliteOverlay {
    database_path: Option<String>,
    lock_path: Option<String>,
    database_identity: Option<String>,
    migration_workspace: Option<String>,
    backup_staging: Option<String>,
    backup_publish: Option<String>,
    restore_workspace: Option<String>,
    reserve_path: Option<String>,
    maximum_batch_operations: Option<u64>,
    maximum_scan_items: Option<u64>,
    busy_timeout_seconds: Option<u64>,
    transaction_expiration_seconds: Option<u64>,
    shutdown_timeout_seconds: Option<u64>,
    wal_autocheckpoint_pages: Option<u64>,
    maximum_wal_bytes: Option<u64>,
    integrity_interval_seconds: Option<u64>,
    backup_retention_count: Option<u64>,
    disk_warning_free_bytes: Option<u64>,
    disk_critical_free_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DependenciesOverlay {
    smtp_address: Option<String>,
    object_store_endpoint: Option<String>,
    data_plane_address: Option<String>,
    edge_gateway_address: Option<String>,
    runtime_supervisor_address: Option<String>,
    telemetry_query_address: Option<String>,
    otlp_address: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DeveloperRegistrationOverlay {
    enabled: Option<bool>,
    verification_lifetime_seconds: Option<u64>,
    recovery_lifetime_seconds: Option<u64>,
    access_lifetime_seconds: Option<u64>,
    refresh_lifetime_seconds: Option<u64>,
    decision_retention_seconds: Option<u64>,
    rate_window_seconds: Option<u64>,
    global_requests_per_window: Option<u64>,
    source_requests_per_window: Option<u64>,
    email_requests_per_window: Option<u64>,
    token_attempts_per_window: Option<u64>,
    maximum_parallel_password_work: Option<u64>,
    maximum_pending_outbox: Option<u64>,
    maximum_outbox_batch: Option<u64>,
    outbox_lease_seconds: Option<u64>,
    outbox_maximum_attempts: Option<u64>,
    outbox_maximum_backoff_seconds: Option<u64>,
    delivered_mail_retention_seconds: Option<u64>,
    smtp_port: Option<u64>,
    smtp_timeout_seconds: Option<u64>,
    smtp_relay_hostname: Option<String>,
    smtp_tls_mode: Option<String>,
    smtp_username: Option<String>,
    smtp_sender: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OperatorAuthenticationOverlay {
    enabled: Option<bool>,
    session_lifetime_seconds: Option<u64>,
    mutation_freshness_seconds: Option<u64>,
    attempt_window_seconds: Option<u64>,
    source_attempts_per_window: Option<u64>,
    identity_attempts_per_window: Option<u64>,
    base_backoff_seconds: Option<u64>,
    maximum_backoff_seconds: Option<u64>,
    break_glass_bearer_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LimitsOverlay {
    max_request_bytes: Option<u64>,
    shutdown_grace_seconds: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SecretsOverlay {
    internal_auth: Option<String>,
    object_store_access_key: Option<String>,
    object_store_secret_key: Option<String>,
    developer_mail_encryption: Option<String>,
    developer_smtp_password: Option<String>,
}

fn read_overlay(path: &Path) -> Result<ConfigOverlay, ConfigDiagnostic> {
    let metadata = fs::metadata(path).map_err(|_| {
        ConfigDiagnostic::new(
            ConfigErrorCode::FileUnreadable,
            "MAKO_CONFIG_FILE",
            "configuration file cannot be read",
        )
    })?;
    if !metadata.is_file() {
        return Err(ConfigDiagnostic::new(
            ConfigErrorCode::FileUnreadable,
            "MAKO_CONFIG_FILE",
            "configuration path must name a regular file",
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigDiagnostic::new(
            ConfigErrorCode::FileTooLarge,
            "MAKO_CONFIG_FILE",
            "configuration file exceeds 1 MiB",
        ));
    }
    let bytes = fs::read(path).map_err(|_| {
        ConfigDiagnostic::new(
            ConfigErrorCode::FileUnreadable,
            "MAKO_CONFIG_FILE",
            "configuration file cannot be read",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ConfigDiagnostic::new(
            ConfigErrorCode::InvalidDocument,
            "MAKO_CONFIG_FILE",
            format!(
                "expected a known-field JSON object (line {}, column {})",
                error.line(),
                error.column()
            ),
        )
    })
}

fn apply_string(
    loader: &ConfigLoader,
    name: &str,
    destination: &mut String,
) -> Result<(), ConfigDiagnostic> {
    if let Some(value) = loader.utf8(name)? {
        *destination = value;
    }
    Ok(())
}

fn apply_optional_string(
    loader: &ConfigLoader,
    name: &str,
    destination: &mut Option<String>,
) -> Result<(), ConfigDiagnostic> {
    if let Some(value) = loader.utf8(name)? {
        *destination = Some(value);
    }
    Ok(())
}

fn invalid(field: &str, message: &str) -> ConfigDiagnostic {
    ConfigDiagnostic::new(ConfigErrorCode::InvalidValue, field, message)
}

fn validate_region(region: &str) -> Result<(), ConfigDiagnostic> {
    if region.is_empty()
        || region.len() > 63
        || region.starts_with('-')
        || region.ends_with('-')
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(invalid(
            "deployment.region",
            "must be a 1-63 character lowercase slug",
        ));
    }
    Ok(())
}

fn parse_socket(value: &str, field: &str) -> Result<SocketAddr, ConfigDiagnostic> {
    value
        .parse()
        .map_err(|_| invalid(field, "must be an IP address and port"))
}

fn parse_boolean(value: &str, field: &str) -> Result<bool, ConfigDiagnostic> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(invalid(field, "must be true or false")),
    }
}

fn validate_smtp_text(
    value: &str,
    field: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ConfigDiagnostic> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(invalid(
            field,
            "contains an invalid SMTP configuration value",
        ))
    } else {
        Ok(())
    }
}

fn parse_http_url(value: &str, field: &str) -> Result<Url, ConfigDiagnostic> {
    let url = Url::parse(value).map_err(|_| invalid(field, "must be an absolute HTTP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            field,
            "must be an absolute HTTP URL without credentials, query, or fragment",
        ));
    }
    Ok(url)
}

fn optional_path(value: Option<String>, field: &str) -> Result<Option<PathBuf>, ConfigDiagnostic> {
    value.map(|value| required_path(value, field)).transpose()
}

fn required_path(value: String, field: &str) -> Result<PathBuf, ConfigDiagnostic> {
    if value.trim().is_empty() {
        return Err(invalid(field, "path cannot be empty"));
    }
    Ok(PathBuf::from(value))
}

/// Control-plane SQLite storage rejects relative paths, but the shipped local
/// defaults are repository-relative. Outside production, resolve them against the
/// process working directory so the documented quickstart starts from a clean
/// checkout. Production is left strict: paths there must already be absolute, and
/// `validate_production_path` still enforces that.
fn resolve_local_storage_path(
    path: PathBuf,
    field: &str,
    environment: DeploymentEnvironment,
) -> Result<PathBuf, ConfigDiagnostic> {
    if environment == DeploymentEnvironment::Production || path.is_absolute() {
        return Ok(path);
    }
    let working_directory = env::current_dir().map_err(|_| {
        invalid(
            field,
            "relative path cannot be resolved without a readable working directory",
        )
    })?;
    Ok(working_directory.join(path))
}

fn validate_production_path(path: &Path, field: &str) -> Result<(), ConfigDiagnostic> {
    if !path.is_absolute() {
        return Err(invalid(field, "must be absolute in production"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid(
            field,
            "must be normalized without . or .. components",
        ));
    }
    if path == Path::new("/")
        || ["/tmp", "/var/tmp", "/dev/shm", "/run"]
            .iter()
            .any(|ephemeral| path.starts_with(ephemeral))
    {
        return Err(invalid(
            field,
            "must name a dedicated non-ephemeral production path",
        ));
    }
    Ok(())
}

fn validate_no_symlink_components(path: &Path, field: &str) -> Result<(), ConfigDiagnostic> {
    for candidate in path.ancestors() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid(
                    field,
                    "must not contain symbolic-link path components",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid(field, "path components could not be inspected")),
        }
    }
    Ok(())
}

fn validate_control_identity(identity: &str) -> Result<(), ConfigDiagnostic> {
    if identity.is_empty()
        || identity.len() > 128
        || !identity
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid(
            "control_storage.database_identity",
            "must be a non-secret deployment identifier",
        ));
    }
    Ok(())
}

fn paths_overlap(first: &Path, second: &Path) -> bool {
    first == second || first.starts_with(second) || second.starts_with(first)
}

fn parse_nonzero_usize(
    value: &str,
    field: &str,
    maximum: u64,
) -> Result<NonZeroUsize, ConfigDiagnostic> {
    let parsed = parse_u64(value, field, 1, maximum)?;
    usize::try_from(parsed)
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| invalid(field, "is outside the supported range"))
}

fn require_regular_file(path: &Path, field: &str) -> Result<(), ConfigDiagnostic> {
    if !path.is_file() {
        return Err(invalid(field, "must name a readable regular file"));
    }
    Ok(())
}

fn parse_u64(
    value: &str,
    field: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, ConfigDiagnostic> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| invalid(field, "must be a base-10 integer"))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(invalid(field, "is outside the supported range"));
    }
    Ok(parsed)
}

enum SecretReference {
    Environment(String),
    File(PathBuf),
}

fn resolve_secret(
    reference: &str,
    loader: &ConfigLoader,
    field: &str,
) -> Result<SecretString, ConfigDiagnostic> {
    let value = match parse_secret_reference(reference, field)? {
        SecretReference::Environment(name) => loader
            .utf8(&name)?
            .ok_or_else(|| {
                ConfigDiagnostic::new(
                    ConfigErrorCode::SecretUnavailable,
                    field,
                    "referenced environment secret is unavailable",
                )
            })?
            .into_bytes(),
        SecretReference::File(path) => read_secret_file(&path, field)?,
    };
    if value.is_empty() || value.len() as u64 > MAX_SECRET_BYTES {
        return Err(ConfigDiagnostic::new(
            ConfigErrorCode::SecretUnavailable,
            field,
            "resolved secret must contain between 1 byte and 64 KiB",
        ));
    }
    let value = String::from_utf8(value).map_err(|_| {
        ConfigDiagnostic::new(
            ConfigErrorCode::SecretUnavailable,
            field,
            "resolved secret must be valid UTF-8",
        )
    })?;
    Ok(SecretString(value.into_boxed_str()))
}

fn validate_secret_shape(
    value: &SecretString,
    field: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), ConfigDiagnostic> {
    let exposed = value.expose_secret();
    if !(minimum..=maximum).contains(&exposed.len()) || exposed.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "resolved secret has an invalid length or contains control characters",
        ));
    }
    Ok(())
}

fn parse_secret_reference(
    reference: &str,
    field: &str,
) -> Result<SecretReference, ConfigDiagnostic> {
    if let Some(name) = reference.strip_prefix("env:") {
        let valid = !name.is_empty()
            && name.len() <= 128
            && name.bytes().enumerate().all(|(index, byte)| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
                b'0'..=b'9' => index > 0,
                _ => false,
            });
        if valid {
            return Ok(SecretReference::Environment(name.to_owned()));
        }
    }
    if let Some(path) = reference.strip_prefix("file:")
        && !path.trim().is_empty()
    {
        return Ok(SecretReference::File(PathBuf::from(path)));
    }
    Err(invalid(
        field,
        "must use an env:VARIABLE or file:/path secret reference",
    ))
}

fn read_secret_file(path: &Path, field: &str) -> Result<Vec<u8>, ConfigDiagnostic> {
    let metadata = fs::metadata(path).map_err(|_| {
        ConfigDiagnostic::new(
            ConfigErrorCode::SecretUnavailable,
            field,
            "referenced secret file is unavailable",
        )
    })?;
    if !metadata.is_file() || metadata.len() > MAX_SECRET_BYTES {
        return Err(ConfigDiagnostic::new(
            ConfigErrorCode::SecretUnavailable,
            field,
            "referenced secret file must be a regular file no larger than 64 KiB",
        ));
    }
    let mut bytes = fs::read(path).map_err(|_| {
        ConfigDiagnostic::new(
            ConfigErrorCode::SecretUnavailable,
            field,
            "referenced secret file cannot be read",
        )
    })?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn the_edge_gateway_endpoint_is_overridable_and_loopback_only_for_the_control_plane() {
        let control =
            ConfigLoader::from_environment([("MAKO_EDGE_GATEWAY_ENDPOINT", "127.0.0.1:18082")])
                .load(ServiceKind::ControlPlane)
                .expect("loopback override");
        assert_eq!(control.edge_gateway_address.port(), 18082);
        let error =
            ConfigLoader::from_environment([("MAKO_EDGE_GATEWAY_ENDPOINT", "10.0.0.5:8082")])
                .load(ServiceKind::ControlPlane)
                .expect_err("the scheduler hop is loopback only");
        assert_eq!(error.code, ConfigErrorCode::InvalidValue);
        assert_eq!(error.field, "dependencies.edge_gateway_address");
        let error =
            ConfigLoader::from_environment([("MAKO_EDGE_GATEWAY_ENDPOINT", "not-a-socket")])
                .load(ServiceKind::ControlPlane)
                .expect_err("malformed");
        assert_eq!(error.field, "dependencies.edge_gateway_address");
    }

    #[test]
    fn defaults_are_typed_and_service_specific() {
        let data = ConfigLoader::default()
            .load(ServiceKind::DataPlane)
            .expect("valid defaults");
        let control = ConfigLoader::default()
            .load(ServiceKind::ControlPlane)
            .expect("valid defaults");
        assert_eq!(data.bind_address.port(), 8080);
        assert_eq!(control.bind_address.port(), 8081);
        assert_eq!(
            control.edge_gateway_address,
            "127.0.0.1:8082".parse().expect("socket"),
            "the scheduler reaches the gateway on its default listener"
        );
        assert_eq!(data.environment, DeploymentEnvironment::Local);
        assert!(data.control_sqlite.is_none());
        assert!(control.control_sqlite.is_some());
        assert!(!control.developer_registration.enabled);
        assert!(control.developer_registration.smtp.is_none());
    }

    #[test]
    fn developer_registration_is_deny_by_default_and_requires_protected_mail() {
        let missing =
            ConfigLoader::from_environment([("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true")])
                .load(ServiceKind::ControlPlane)
                .expect_err("mail dependencies are required");
        assert_eq!(missing.field, "developer_registration");

        let loader = ConfigLoader::from_environment([
            ("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true"),
            ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "smtp.example.test"),
            ("MAKO_DEVELOPER_SMTP_PORT", "587"),
            ("MAKO_DEVELOPER_SMTP_TLS_MODE", "starttls"),
            ("MAKO_DEVELOPER_SMTP_USERNAME", "smtp-user@example.test"),
            (
                "MAKO_DEVELOPER_SMTP_SENDER",
                "Mako Cloud <no-reply@example.test>",
            ),
            (
                "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
                "env:TEST_DEVELOPER_MAIL_KEY",
            ),
            (
                "MAKO_DEVELOPER_SMTP_PASSWORD_REF",
                "env:TEST_DEVELOPER_SMTP_PASSWORD",
            ),
            (
                "TEST_DEVELOPER_MAIL_KEY",
                "a-dedicated-mail-encryption-secret-value",
            ),
            (
                "TEST_DEVELOPER_SMTP_PASSWORD",
                "a-protected-smtp-app-password",
            ),
        ]);
        let config = loader
            .load(ServiceKind::ControlPlane)
            .expect("registration config");
        assert!(config.developer_registration.enabled);
        let smtp = config.developer_registration.smtp.expect("SMTP settings");
        assert_eq!(smtp.tls_mode, SmtpTlsMode::StartTls);
        assert_eq!(
            format!("{:?}", smtp.password),
            "Some(SecretString([REDACTED]))"
        );
    }

    #[test]
    fn plaintext_smtp_is_local_only_and_may_omit_credentials() {
        let loader = ConfigLoader::from_environment([
            ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "127.0.0.1"),
            ("MAKO_DEVELOPER_SMTP_PORT", "1025"),
            ("MAKO_DEVELOPER_SMTP_TLS_MODE", "plaintext"),
            (
                "MAKO_DEVELOPER_SMTP_SENDER",
                "Mako Local <no-reply@localhost>",
            ),
        ]);
        let config = loader
            .load(ServiceKind::ControlPlane)
            .expect("local plaintext");
        let smtp = config.developer_registration.smtp.expect("SMTP settings");
        assert_eq!(smtp.tls_mode, SmtpTlsMode::Plaintext);
        assert!(smtp.username.is_none());
        assert!(smtp.password.is_none());

        // Credentials are still a pair.
        let error = ConfigLoader::from_environment([
            ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "127.0.0.1"),
            ("MAKO_DEVELOPER_SMTP_TLS_MODE", "plaintext"),
            ("MAKO_DEVELOPER_SMTP_SENDER", "no-reply@localhost"),
            ("MAKO_DEVELOPER_SMTP_USERNAME", "mailpit"),
        ])
        .load(ServiceKind::ControlPlane)
        .expect_err("a username without a password is incomplete");
        assert_eq!(error.code, ConfigErrorCode::MissingValue);
        assert_eq!(error.field, "developer_registration.smtp");

        // Outside plaintext, credentials stay mandatory.
        let error = ConfigLoader::from_environment([
            ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "smtp.example.test"),
            ("MAKO_DEVELOPER_SMTP_TLS_MODE", "starttls"),
            ("MAKO_DEVELOPER_SMTP_SENDER", "no-reply@example.test"),
        ])
        .load(ServiceKind::ControlPlane)
        .expect_err("TLS relays require credentials");
        assert_eq!(error.field, "developer_registration.smtp");

        let error = production_control_loader([("MAKO_DEVELOPER_SMTP_TLS_MODE", "plaintext")])
            .load(ServiceKind::ControlPlane)
            .expect_err("production refuses plaintext before anything else about mail");
        assert_eq!(error.code, ConfigErrorCode::InvalidValue);
        assert_eq!(error.field, "developer_registration.smtp_tls_mode");
    }

    #[test]
    fn environment_overrides_are_validated() {
        let loader = ConfigLoader::from_environment([
            ("MAKO_BIND_ADDR", "0.0.0.0:9090"),
            ("MAKO_REGION", "us-east-1"),
            ("MAKO_MAX_REQUEST_BYTES", "2048"),
        ]);
        let config = loader.load(ServiceKind::DataPlane).expect("valid config");
        assert_eq!(config.bind_address.port(), 9090);
        assert_eq!(config.region, "us-east-1");
        assert_eq!(config.max_request_bytes, 2048);
    }

    #[test]
    fn control_plane_runtime_supervisor_is_loopback_only() {
        let loader = ConfigLoader::from_environment([(
            "MAKO_RUNTIME_SUPERVISOR_ENDPOINT",
            "192.0.2.1:9001",
        )]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("non-loopback supervisor must fail");
        assert_eq!(error.code, ConfigErrorCode::InvalidValue);
        assert_eq!(error.field, "dependencies.runtime_supervisor_address");

        let loader =
            ConfigLoader::from_environment([("MAKO_TELEMETRY_QUERY_ENDPOINT", "192.0.2.1:9465")]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("non-loopback telemetry query source must fail");
        assert_eq!(error.code, ConfigErrorCode::InvalidValue);
        assert_eq!(error.field, "dependencies.telemetry_query_address");
    }

    #[test]
    fn file_values_are_overridden_by_the_environment() {
        let path = temporary_file(
            "config",
            br#"{
            "deployment": {"region": "file-region"},
            "server": {"bind_address": "127.0.0.1:7000"},
            "limits": {"max_request_bytes": 4096}
        }"#,
        );
        let loader = ConfigLoader::from_environment([
            ("MAKO_CONFIG_FILE", path.to_str().expect("UTF-8 path")),
            ("MAKO_REGION", "environment-region"),
        ]);
        let config = loader.load(ServiceKind::EdgeGateway).expect("valid config");
        fs::remove_file(path).expect("remove temporary config");
        assert_eq!(config.region, "environment-region");
        assert_eq!(config.bind_address.port(), 7000);
        assert_eq!(config.max_request_bytes, 4096);
    }

    #[test]
    fn secret_references_resolve_without_debug_disclosure() {
        let loader = ConfigLoader::from_environment([
            ("MAKO_INTERNAL_AUTH_SECRET_REF", "env:TEST_MAKO_SECRET"),
            ("TEST_MAKO_SECRET", "extremely-sensitive-value"),
        ]);
        let config = loader.load(ServiceKind::DataPlane).expect("valid config");
        let secret = config.internal_auth_secret.expect("resolved secret");
        assert_eq!(secret.expose_secret(), "extremely-sensitive-value");
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");
        assert_eq!(secret.to_string(), "[REDACTED]");
    }

    #[test]
    fn object_store_credentials_are_paired_validated_and_redacted() {
        let loader = ConfigLoader::from_environment([
            (
                "MAKO_OBJECT_STORE_ACCESS_KEY_REF",
                "env:TEST_OBJECT_STORE_ACCESS",
            ),
            (
                "MAKO_OBJECT_STORE_SECRET_KEY_REF",
                "env:TEST_OBJECT_STORE_SECRET",
            ),
            ("TEST_OBJECT_STORE_ACCESS", "test-access-key"),
            (
                "TEST_OBJECT_STORE_SECRET",
                "test-secret-key-that-is-long-enough",
            ),
        ]);
        let config = loader
            .load(ServiceKind::ControlPlane)
            .expect("valid config");
        let access_key = config.object_store_access_key.expect("access key");
        let secret_key = config.object_store_secret_key.expect("secret key");
        assert_eq!(access_key.expose_secret(), "test-access-key");
        assert_eq!(format!("{secret_key:?}"), "SecretString([REDACTED])");
        assert!(!format!("{secret_key:?}").contains("long-enough"));

        let loader = ConfigLoader::from_environment([
            (
                "MAKO_OBJECT_STORE_ACCESS_KEY_REF",
                "env:TEST_OBJECT_STORE_ACCESS",
            ),
            ("TEST_OBJECT_STORE_ACCESS", "test-access-key"),
        ]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("unpaired credentials must fail");
        assert_eq!(error.field, "secrets.object_store");
    }

    #[test]
    fn operator_authentication_settings_are_bounded_and_fail_closed_by_default() {
        let defaults = ConfigLoader::from_environment(std::iter::empty::<(&str, &str)>())
            .load(ServiceKind::ControlPlane)
            .expect("default control-plane config");
        assert!(!defaults.operator_authentication.enabled);
        assert!(!defaults.operator_authentication.break_glass_bearer_enabled);
        assert_eq!(
            defaults.operator_authentication.cookie_name,
            "__Secure-mako_operator"
        );
        assert!(defaults.operator_authentication.cookie_secure);
        assert!(defaults.operator_authentication.cookie_http_only);
        assert_eq!(defaults.operator_authentication.cookie_same_site, "Strict");
        assert_eq!(defaults.operator_authentication.cookie_path, "/v1");

        let configured = ConfigLoader::from_environment([
            ("MAKO_OPERATOR_PASSWORD_AUTH_ENABLED", "true"),
            ("MAKO_OPERATOR_SESSION_TTL_SECONDS", "1800"),
            ("MAKO_OPERATOR_MUTATION_FRESHNESS_SECONDS", "120"),
            ("MAKO_OPERATOR_ATTEMPT_WINDOW_SECONDS", "600"),
            ("MAKO_OPERATOR_SOURCE_RATE_LIMIT", "12"),
            ("MAKO_OPERATOR_IDENTITY_RATE_LIMIT", "6"),
            ("MAKO_OPERATOR_BASE_BACKOFF_SECONDS", "2"),
            ("MAKO_OPERATOR_MAX_BACKOFF_SECONDS", "120"),
            ("MAKO_OPERATOR_BREAK_GLASS_BEARER_ENABLED", "false"),
        ])
        .load(ServiceKind::ControlPlane)
        .expect("operator config");
        assert!(configured.operator_authentication.enabled);
        assert_eq!(
            configured.operator_authentication.session_lifetime,
            Duration::from_secs(1800)
        );
        assert_eq!(
            configured.operator_authentication.mutation_freshness,
            Duration::from_secs(120)
        );
        let diagnostic = configured.startup_diagnostic().to_string();
        assert!(diagnostic.contains("operator_password_auth=enabled"));
        assert!(diagnostic.contains("operator_break_glass=disabled"));

        let error = ConfigLoader::from_environment([("MAKO_OPERATOR_SESSION_TTL_SECONDS", "3601")])
            .load(ServiceKind::ControlPlane)
            .expect_err("overlong operator session must fail");
        assert_eq!(
            error.field,
            "operator_authentication.session_lifetime_seconds"
        );
    }

    #[test]
    fn production_control_plane_requires_authenticated_object_storage() {
        let loader = production_control_loader([]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("credentials are required");
        assert_eq!(error.code, ConfigErrorCode::MissingValue);
        assert_eq!(error.field, "secrets.object_store");
    }

    #[test]
    fn production_control_plane_requires_sqlite_and_rejects_legacy_rocksdb() {
        let loader = production_control_loader([
            ("MAKO_OBJECT_STORE_ACCESS_KEY_REF", "env:TEST_OBJECT_ACCESS"),
            ("TEST_OBJECT_ACCESS", "test-access"),
            ("MAKO_OBJECT_STORE_SECRET_KEY_REF", "env:TEST_OBJECT_SECRET"),
            ("TEST_OBJECT_SECRET", "test-object-secret-long-enough"),
        ]);
        let config = loader
            .load(ServiceKind::ControlPlane)
            .expect("valid SQLite control configuration");
        assert!(config.control_sqlite.is_some());
        assert!(
            config
                .startup_diagnostic()
                .to_string()
                .contains("storage=control-sqlite")
        );

        let loader = production_control_loader([
            ("MAKO_OBJECT_STORE_ACCESS_KEY_REF", "env:TEST_OBJECT_ACCESS"),
            ("TEST_OBJECT_ACCESS", "test-access"),
            ("MAKO_OBJECT_STORE_SECRET_KEY_REF", "env:TEST_OBJECT_SECRET"),
            ("TEST_OBJECT_SECRET", "test-object-secret-long-enough"),
            ("MAKO_ROCKSDB_PATH", "/srv/mako/control/legacy-rocksdb"),
        ]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("legacy control RocksDB must be rejected");
        assert_eq!(error.field, "storage.rocksdb_path");
    }

    #[test]
    fn control_sqlite_rejects_unsafe_paths_identity_limits_and_secret_fields() {
        for (name, value, expected_field) in [
            (
                "MAKO_CONTROL_SQLITE_PATH",
                "relative/control.sqlite3",
                "control_storage.database_path",
            ),
            (
                "MAKO_CONTROL_SQLITE_PATH",
                "/tmp/mako/control.sqlite3",
                "control_storage.database_path",
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_STAGING",
                "/srv/mako/control/migration",
                "control_storage.backup_staging",
            ),
            (
                "MAKO_CONTROL_SQLITE_IDENTITY",
                "",
                "control_storage.database_identity",
            ),
            (
                "MAKO_CONTROL_SQLITE_BUSY_TIMEOUT_SECONDS",
                "0",
                "control_storage.busy_timeout_seconds",
            ),
            (
                "MAKO_CONTROL_SQLITE_DISK_CRITICAL_FREE_BYTES",
                "1024",
                "control_storage.disk_critical_free_bytes",
            ),
        ] {
            let loader = production_control_loader([(name, value)]);
            let error = loader
                .load(ServiceKind::ControlPlane)
                .expect_err("unsafe control SQLite input must fail");
            assert_eq!(error.field, expected_field);
        }

        let path = temporary_file(
            "control-storage-secret",
            br#"{"control_storage":{"credential":"env:DO_NOT_ACCEPT"}}"#,
        );
        let loader = ConfigLoader::from_environment([(
            "MAKO_CONFIG_FILE",
            path.to_str().expect("UTF-8 path"),
        )]);
        let error = loader
            .load(ServiceKind::ControlPlane)
            .expect_err("control storage credentials are not configuration");
        fs::remove_file(path).expect("remove temporary config");
        assert_eq!(error.code, ConfigErrorCode::InvalidDocument);
    }

    #[test]
    fn local_control_storage_resolves_relative_defaults_and_production_stays_strict() {
        // Control SQLite storage rejects relative paths, but the shipped local
        // defaults are repository-relative. If they are not resolved, the
        // documented quickstart cannot start the control plane at all.
        let local = ConfigLoader::default()
            .load(ServiceKind::ControlPlane)
            .expect("local defaults are usable");
        let storage = local
            .control_sqlite
            .as_ref()
            .expect("control plane configures SQLite");
        let working_directory = env::current_dir().expect("working directory");
        for (field, path) in [
            ("database_path", storage.database_path.as_path()),
            ("lock_path", storage.lock_path.as_path()),
            ("migration_workspace", storage.migration_workspace.as_path()),
            ("backup_staging", storage.backup_staging.as_path()),
            ("backup_publish", storage.backup_publish.as_path()),
            ("restore_workspace", storage.restore_workspace.as_path()),
            ("reserve_path", storage.reserve_path.as_path()),
        ] {
            assert!(
                path.is_absolute(),
                "{field} must resolve to an absolute path, got {}",
                path.display()
            );
            assert!(
                path.starts_with(&working_directory),
                "{field} must resolve under the working directory, got {}",
                path.display()
            );
        }

        // Production is unchanged: a relative path is still refused rather than
        // silently resolved against whatever directory the service started in.
        let error =
            production_control_loader([("MAKO_CONTROL_SQLITE_PATH", "relative/control.sqlite3")])
                .load(ServiceKind::ControlPlane)
                .expect_err("production must refuse a relative control path");
        assert_eq!(error.field, "control_storage.database_path");
    }

    #[cfg(unix)]
    #[test]
    fn control_sqlite_rejects_symlinked_path_components() {
        use std::os::unix::fs::symlink;

        let directory = env::temp_dir().join(format!(
            "mako-config-symlink-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let real = directory.join("real");
        let linked = directory.join("linked");
        fs::create_dir_all(&real).expect("create real directory");
        symlink(&real, &linked).expect("create symlink");
        let error = validate_no_symlink_components(
            &linked.join("control.sqlite3"),
            "control_storage.database_path",
        )
        .expect_err("symlinked storage path must fail");
        fs::remove_dir_all(&directory).expect("remove test directory");
        assert_eq!(error.field, "control_storage.database_path");
    }

    #[test]
    fn production_fails_fast_without_https() {
        let loader = ConfigLoader::from_environment([
            ("MAKO_ENVIRONMENT", "production"),
            ("MAKO_PUBLIC_URL", "http://api.example.test"),
        ]);
        let error = loader
            .load(ServiceKind::DataPlane)
            .expect_err("insecure production URL must fail");
        assert_eq!(error.code, ConfigErrorCode::InvalidValue);
        assert_eq!(error.field, "server.public_url");
    }

    #[test]
    fn production_rejects_relative_ephemeral_and_overlapping_storage_paths() {
        for (database, backup, expected_field) in [
            (
                "relative/rocksdb",
                "/srv/mako/backups",
                "storage.rocksdb_path",
            ),
            (
                "/tmp/mako/rocksdb",
                "/srv/mako/backups",
                "storage.rocksdb_path",
            ),
            (
                "/srv/mako/rocksdb",
                "/var/tmp/mako",
                "storage.backup_destination",
            ),
            (
                "/srv/mako/rocksdb",
                "/srv/mako/rocksdb/backups",
                "storage.backup_destination",
            ),
        ] {
            let loader = production_loader([
                ("MAKO_ROCKSDB_PATH", database),
                ("MAKO_ROCKSDB_BACKUP_DESTINATION", backup),
            ]);
            let error = loader
                .load(ServiceKind::DataPlane)
                .expect_err("unsafe production path must fail");
            assert_eq!(error.field, expected_field);
        }
    }

    #[test]
    fn production_rejects_unsafe_storage_limits_and_durability_fields() {
        for (name, value, expected_field) in [
            (
                "MAKO_ROCKSDB_MAX_BATCH_OPERATIONS",
                "0",
                "storage.maximum_batch_operations",
            ),
            (
                "MAKO_ROCKSDB_MAX_SCAN_ITEMS",
                "1000001",
                "storage.maximum_scan_items",
            ),
            (
                "MAKO_ROCKSDB_LOCK_TIMEOUT_SECONDS",
                "0",
                "storage.transaction_lock_timeout_seconds",
            ),
            (
                "MAKO_ROCKSDB_DISK_CRITICAL_FREE_BYTES",
                "1024",
                "storage.disk_critical_free_bytes",
            ),
        ] {
            let loader = production_loader([(name, value)]);
            let error = loader
                .load(ServiceKind::DataPlane)
                .expect_err("unsafe production limit must fail");
            assert_eq!(error.field, expected_field);
        }

        let path = temporary_file(
            "durability-downgrade",
            br#"{"storage":{"durability":"wal"}}"#,
        );
        let loader = ConfigLoader::from_environment([(
            "MAKO_CONFIG_FILE",
            path.to_str().expect("UTF-8 path"),
        )]);
        let error = loader
            .load(ServiceKind::DataPlane)
            .expect_err("durability is not configurable");
        fs::remove_file(path).expect("remove temporary config");
        assert_eq!(error.code, ConfigErrorCode::InvalidDocument);
    }

    #[test]
    fn malformed_or_unknown_file_fields_fail_safely() {
        let path = temporary_file("config", br#"{"server":{"bind_adress":"secret-text"}}"#);
        let loader = ConfigLoader::from_environment([(
            "MAKO_CONFIG_FILE",
            path.to_str().expect("UTF-8 path"),
        )]);
        let error = loader
            .load(ServiceKind::DataPlane)
            .expect_err("unknown field must fail");
        fs::remove_file(path).expect("remove temporary config");
        assert_eq!(error.code, ConfigErrorCode::InvalidDocument);
        assert!(!error.to_string().contains("secret-text"));
    }

    #[test]
    fn storage_backend_endpoint_and_credential_fields_are_not_configuration() {
        for document in [
            br#"{"storage":{"backend":"memory"}}"#.as_slice(),
            br#"{"storage":{"backend":"distributed","endpoint":"https://kv.invalid"}}"#,
            br#"{"storage":{"credential":"env:UNSUPPORTED_STORAGE_SECRET"}}"#,
        ] {
            let path = temporary_file("unsupported-storage-surface", document);
            let loader = ConfigLoader::from_environment([(
                "MAKO_CONFIG_FILE",
                path.to_str().expect("UTF-8 path"),
            )]);
            let error = loader
                .load(ServiceKind::DataPlane)
                .expect_err("unsupported storage fields must fail closed");
            fs::remove_file(path).expect("remove temporary config");
            assert_eq!(error.code, ConfigErrorCode::InvalidDocument);
            assert_eq!(error.field, "MAKO_CONFIG_FILE");
        }
    }

    #[test]
    fn unsupported_backend_environment_selectors_cannot_change_rocksdb() {
        let loader = ConfigLoader::from_environment([
            ("MAKO_STORAGE_BACKEND", "memory"),
            ("MAKO_KV_BACKEND", "distributed"),
            ("MAKO_STORAGE_ENDPOINT", "https://kv.invalid"),
            (
                "MAKO_STORAGE_CREDENTIAL_REF",
                "env:UNSUPPORTED_STORAGE_SECRET",
            ),
            ("MAKO_ROCKSDB_PATH", "/var/lib/mako/data-plane"),
        ]);
        let config = loader.load(ServiceKind::DataPlane).expect("valid config");
        assert_eq!(config.rocksdb.path, Path::new("/var/lib/mako/data-plane"));
    }

    #[test]
    fn invalid_secret_reference_does_not_echo_input() {
        let loader = ConfigLoader::from_environment([(
            "MAKO_INTERNAL_AUTH_SECRET_REF",
            "literal:do-not-print-this",
        )]);
        let error = loader
            .load(ServiceKind::DataPlane)
            .expect_err("literal secret must fail");
        assert_eq!(error.field, "secrets.internal_auth");
        assert!(!error.to_string().contains("do-not-print-this"));
    }

    fn temporary_file(label: &str, contents: &[u8]) -> PathBuf {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "mako-config-{label}-{}-{sequence}.json",
            std::process::id()
        ));
        fs::write(&path, contents).expect("write temporary file");
        path
    }

    fn production_loader<const N: usize>(overrides: [(&str, &str); N]) -> ConfigLoader {
        let mut values = vec![
            ("MAKO_ENVIRONMENT", "production"),
            ("MAKO_PUBLIC_URL", "https://api.example.test"),
            (
                "MAKO_INTERNAL_AUTH_SECRET_REF",
                "env:TEST_MAKO_INTERNAL_AUTH",
            ),
            ("TEST_MAKO_INTERNAL_AUTH", "test-only-secret"),
            ("MAKO_ROCKSDB_PATH", "/srv/mako/data-plane/rocksdb"),
            (
                "MAKO_ROCKSDB_BACKUP_DESTINATION",
                "/srv/mako/backups/data-plane",
            ),
        ];
        values.extend(overrides);
        ConfigLoader::from_environment(values)
    }

    fn production_control_loader<const N: usize>(overrides: [(&str, &str); N]) -> ConfigLoader {
        let mut values = vec![
            ("MAKO_ENVIRONMENT", "production"),
            ("MAKO_PUBLIC_URL", "https://api.example.test"),
            (
                "MAKO_INTERNAL_AUTH_SECRET_REF",
                "env:TEST_MAKO_INTERNAL_AUTH",
            ),
            ("TEST_MAKO_INTERNAL_AUTH", "test-only-secret"),
            (
                "MAKO_CONTROL_SQLITE_PATH",
                "/srv/mako/control/live/control.sqlite3",
            ),
            (
                "MAKO_CONTROL_SQLITE_LOCK_PATH",
                "/srv/mako/control/lock/control.lock",
            ),
            ("MAKO_CONTROL_SQLITE_IDENTITY", "mako-control-production"),
            (
                "MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE",
                "/srv/mako/control/migration",
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_STAGING",
                "/srv/mako/control/backup-staging",
            ),
            (
                "MAKO_CONTROL_SQLITE_BACKUP_PUBLISH",
                "/mnt/mako-control-backups/published",
            ),
            (
                "MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE",
                "/srv/mako/control/restore",
            ),
            (
                "MAKO_CONTROL_SQLITE_RESERVE_PATH",
                "/srv/mako/control/reserve",
            ),
        ];
        values.extend(overrides);
        ConfigLoader::from_environment(values)
    }
}
