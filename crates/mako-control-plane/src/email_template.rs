//! Per-environment templates for the mail an environment sends to its
//! application users: verification, recovery, invitation, and magic link.
//!
//! A template is plain text: a one-line subject and a text body, each with
//! `{{variable}}` placeholders drawn from a per-kind allowlist. Plain text is
//! the applied "safe subset" of the auth-providers design: the renderer never
//! interprets markup, so a template cannot carry script or remote content --
//! the output is delivered as `text/plain` exactly as rendered. Unknown
//! variables and braces outside a well-formed placeholder are refused when the
//! template is saved or previewed, never discovered at delivery time.

use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize, sync::Arc};

use mako_api::TenantScope;
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperPrincipal, OrganizationId, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
};

/// The subject is one header line: bounded and free of line breaks.
pub const MAXIMUM_SUBJECT_BYTES: usize = 200;
/// The body is bounded so a template cannot turn into a bulk payload.
pub const MAXIMUM_TEXT_BODY_BYTES: usize = 32 * 1024;

/// The variables every kind of mail may reference.
const COMMON_VARIABLES: &[&str] = &[
    "link",
    "expires_at",
    "email",
    "project_name",
    "environment_name",
];
const INVITATION_VARIABLES: &[&str] = &[
    "link",
    "expires_at",
    "email",
    "project_name",
    "environment_name",
    "inviter",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailTemplateKind {
    Verification,
    Recovery,
    Invitation,
    MagicLink,
}

impl EmailTemplateKind {
    pub const ALL: [Self; 4] = [
        Self::Verification,
        Self::Recovery,
        Self::Invitation,
        Self::MagicLink,
    ];

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Recovery => "recovery",
            Self::Invitation => "invitation",
            Self::MagicLink => "magic_link",
        }
    }

    /// The placeholders a template of this kind may use.
    #[must_use]
    pub const fn variables(self) -> &'static [&'static str] {
        match self {
            Self::Verification | Self::Recovery | Self::MagicLink => COMMON_VARIABLES,
            Self::Invitation => INVITATION_VARIABLES,
        }
    }

    #[must_use]
    pub const fn default_subject(self) -> &'static str {
        match self {
            Self::Verification => "Verify your email for {{project_name}}",
            Self::Recovery => "Reset your {{project_name}} password",
            Self::Invitation => "You are invited to {{project_name}}",
            Self::MagicLink => "Your sign-in link for {{project_name}}",
        }
    }

    #[must_use]
    pub const fn default_text_body(self) -> &'static str {
        match self {
            Self::Verification => {
                "Hello,\n\n\
                 Confirm {{email}} as your address for {{project_name}} ({{environment_name}}) \
                 by opening this link:\n\n\
                 {{link}}\n\n\
                 The link expires at {{expires_at}}. If you did not create an account, you can \
                 ignore this message.\n"
            }
            Self::Recovery => {
                "Hello,\n\n\
                 A password reset was requested for {{email}} on {{project_name}} \
                 ({{environment_name}}). Choose a new password here:\n\n\
                 {{link}}\n\n\
                 The link expires at {{expires_at}}. If you did not request a reset, you can \
                 ignore this message and your password stays as it is.\n"
            }
            Self::Invitation => {
                "Hello,\n\n\
                 {{email}} has been invited to join {{project_name}} ({{environment_name}}). \
                 Accept the invitation by opening this link:\n\n\
                 {{link}}\n\n\
                 Invited by: {{inviter}}\n\
                 The invitation expires at {{expires_at}}. If you were not expecting it, you can \
                 ignore this message.\n"
            }
            Self::MagicLink => {
                "Hello,\n\n\
                 Use this link to sign in to {{project_name}} ({{environment_name}}) as \
                 {{email}}:\n\n\
                 {{link}}\n\n\
                 The link can be used once and expires at {{expires_at}}. If you did not request \
                 it, you can ignore this message.\n"
            }
        }
    }
}

impl fmt::Display for EmailTemplateKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Template text as a developer submits it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmailTemplateText {
    pub subject: String,
    pub text_body: String,
}

impl EmailTemplateText {
    /// Refuses malformed placeholders, unknown variables, unbalanced braces,
    /// line breaks in the subject, and text outside the size bounds.
    pub fn validate(&self, kind: EmailTemplateKind) -> Result<(), EmailTemplateError> {
        validate_subject_text(&self.subject)?;
        validate_body_text(&self.text_body)?;
        validate_placeholders("subject", &self.subject, kind)?;
        validate_placeholders("textBody", &self.text_body, kind)?;
        // Every kind of mail exists to deliver its link: a verification,
        // recovery, invitation, or sign-in mail without one reaches its
        // reader with nothing to act on, and a verification that can never
        // complete locks the account out.
        if !parse_segments("textBody", &self.text_body)?
            .iter()
            .any(|segment| matches!(segment, Segment::Variable(name) if *name == "link"))
        {
            return Err(EmailTemplateError::InvalidTemplate(
                "textBody: must include {{link}}, the link this mail exists to deliver".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A rendered subject and body, ready to become a mail envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailTemplateRender {
    pub subject: String,
    pub text_body: String,
}

/// The stored customization for one kind of mail. Its absence means the
/// built-in default applies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmailTemplateRecord {
    tenant: TenantScope,
    kind: EmailTemplateKind,
    subject: String,
    text_body: String,
    version: u64,
    updated_at_unix_seconds: u64,
}

impl EmailTemplateRecord {
    #[must_use]
    pub fn tenant(&self) -> &TenantScope {
        &self.tenant
    }

    #[must_use]
    pub const fn kind(&self) -> EmailTemplateKind {
        self.kind
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    #[must_use]
    pub fn text_body(&self) -> &str {
        &self.text_body
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }
}

/// What the management API shows: the effective template, flagged when it is
/// the built-in default rather than a customization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailTemplateView {
    pub kind: EmailTemplateKind,
    pub subject: String,
    pub text_body: String,
    pub is_default: bool,
    pub version: u64,
    pub updated_at_unix_seconds: Option<u64>,
}

impl EmailTemplateView {
    fn default_for(kind: EmailTemplateKind) -> Self {
        Self {
            kind,
            subject: kind.default_subject().to_owned(),
            text_body: kind.default_text_body().to_owned(),
            is_default: true,
            version: 0,
            updated_at_unix_seconds: None,
        }
    }

    fn from_record(record: EmailTemplateRecord) -> Self {
        Self {
            kind: record.kind,
            subject: record.subject,
            text_body: record.text_body,
            is_default: false,
            version: record.version,
            updated_at_unix_seconds: Some(record.updated_at_unix_seconds),
        }
    }
}

/// The template the delivery path applies: customized or default.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEmailTemplate {
    pub kind: EmailTemplateKind,
    pub subject: String,
    pub text_body: String,
    pub is_default: bool,
}

impl ResolvedEmailTemplate {
    /// Substitutes the allowlisted variables into this template.
    pub fn render(
        &self,
        variables: &BTreeMap<String, String>,
    ) -> Result<EmailTemplateRender, EmailTemplateError> {
        render_template(self.kind, &self.subject, &self.text_body, variables)
    }
}

/// Substitutes `variables` into a template of `kind`. Only allowlisted
/// variables are substituted; a variable the caller did not supply renders
/// empty (an invitation may carry no inviter). Values are sanitized for the
/// position they land in: control characters never reach the subject line,
/// and only line breaks and tabs survive in the body.
pub fn render_template(
    kind: EmailTemplateKind,
    subject: &str,
    text_body: &str,
    variables: &BTreeMap<String, String>,
) -> Result<EmailTemplateRender, EmailTemplateError> {
    let text = EmailTemplateText {
        subject: subject.to_owned(),
        text_body: text_body.to_owned(),
    };
    text.validate(kind)?;
    let mut rendered_subject = substitute(subject, kind, variables, sanitize_subject_value)?;
    let rendered_body = substitute(text_body, kind, variables, sanitize_body_value)?;
    if rendered_subject.trim().is_empty() {
        // A subject made only of placeholders that rendered empty still has
        // to say something; the default subject is always non-empty.
        rendered_subject = substitute(
            kind.default_subject(),
            kind,
            variables,
            sanitize_subject_value,
        )?;
    }
    if rendered_subject.len() > MAXIMUM_SUBJECT_BYTES {
        let mut cut = MAXIMUM_SUBJECT_BYTES;
        while !rendered_subject.is_char_boundary(cut) {
            cut -= 1;
        }
        rendered_subject.truncate(cut);
    }
    if rendered_body.is_empty() || rendered_body.len() > MAXIMUM_TEXT_BODY_BYTES {
        return Err(EmailTemplateError::InvalidTemplate(
            "rendered body is empty or exceeds 32 KiB".to_owned(),
        ));
    }
    Ok(EmailTemplateRender {
        subject: rendered_subject,
        text_body: rendered_body,
    })
}

/// Placeholder data a preview renders with. Names come from the real
/// project and environment; everything else is visibly illustrative.
#[must_use]
pub fn preview_variables(
    kind: EmailTemplateKind,
    project_name: &str,
    environment_name: &str,
) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::new();
    variables.insert(
        "link".to_owned(),
        "https://app.example.com/auth/callback?token=preview-token".to_owned(),
    );
    variables.insert("expires_at".to_owned(), "2030-01-01T12:00:00Z".to_owned());
    variables.insert("email".to_owned(), "person@example.com".to_owned());
    variables.insert("project_name".to_owned(), project_name.to_owned());
    variables.insert("environment_name".to_owned(), environment_name.to_owned());
    if kind == EmailTemplateKind::Invitation {
        variables.insert("inviter".to_owned(), "A teammate".to_owned());
    }
    variables
}

enum Segment<'a> {
    Text(&'a str),
    Variable(&'a str),
}

/// Splits template text into literal runs and placeholders. Every brace
/// must belong to a `{{name}}` placeholder: a lone `{` or `}`, an unclosed
/// `{{`, or a nested brace is refused rather than passed through.
fn parse_segments<'a>(
    field: &'static str,
    text: &'a str,
) -> Result<Vec<Segment<'a>>, EmailTemplateError> {
    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        let Some(open) = rest.find("{{") else {
            reject_stray_braces(field, rest)?;
            if !rest.is_empty() {
                segments.push(Segment::Text(rest));
            }
            return Ok(segments);
        };
        let (literal, after_open) = rest.split_at(open);
        reject_stray_braces(field, literal)?;
        if !literal.is_empty() {
            segments.push(Segment::Text(literal));
        }
        let inside = &after_open[2..];
        let Some(close) = inside.find("}}") else {
            return Err(EmailTemplateError::InvalidTemplate(format!(
                "{field}: placeholder is not closed; write {{{{name}}}}"
            )));
        };
        let name = inside[..close].trim();
        if name.is_empty()
            || name.contains(['{', '}'])
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            return Err(EmailTemplateError::InvalidTemplate(format!(
                "{field}: placeholder name is invalid; write {{{{name}}}} with lowercase letters and underscores"
            )));
        }
        segments.push(Segment::Variable(name));
        rest = &inside[close + 2..];
    }
}

fn reject_stray_braces(field: &'static str, literal: &str) -> Result<(), EmailTemplateError> {
    if literal.contains(['{', '}']) {
        return Err(EmailTemplateError::InvalidTemplate(format!(
            "{field}: braces are only allowed around a placeholder such as {{{{link}}}}"
        )));
    }
    Ok(())
}

fn validate_placeholders(
    field: &'static str,
    text: &str,
    kind: EmailTemplateKind,
) -> Result<(), EmailTemplateError> {
    for segment in parse_segments(field, text)? {
        if let Segment::Variable(name) = segment
            && !kind.variables().contains(&name)
        {
            return Err(EmailTemplateError::InvalidTemplate(format!(
                "{field}: unknown variable {{{{{name}}}}}; {kind} templates may use {}",
                kind.variables().join(", ")
            )));
        }
    }
    Ok(())
}

fn validate_subject_text(subject: &str) -> Result<(), EmailTemplateError> {
    if !(1..=MAXIMUM_SUBJECT_BYTES).contains(&subject.len()) {
        return Err(EmailTemplateError::InvalidTemplate(
            "subject: must be between 1 and 200 bytes".to_owned(),
        ));
    }
    if subject.chars().any(char::is_control) {
        return Err(EmailTemplateError::InvalidTemplate(
            "subject: must be one line without control characters".to_owned(),
        ));
    }
    if subject.trim().is_empty() {
        return Err(EmailTemplateError::InvalidTemplate(
            "subject: must not be blank".to_owned(),
        ));
    }
    Ok(())
}

fn validate_body_text(text_body: &str) -> Result<(), EmailTemplateError> {
    if !(1..=MAXIMUM_TEXT_BODY_BYTES).contains(&text_body.len()) {
        return Err(EmailTemplateError::InvalidTemplate(
            "textBody: must be between 1 byte and 32 KiB".to_owned(),
        ));
    }
    if text_body
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(EmailTemplateError::InvalidTemplate(
            "textBody: control characters other than line breaks and tabs are not allowed"
                .to_owned(),
        ));
    }
    if text_body.trim().is_empty() {
        return Err(EmailTemplateError::InvalidTemplate(
            "textBody: must not be blank".to_owned(),
        ));
    }
    Ok(())
}

fn substitute(
    text: &str,
    kind: EmailTemplateKind,
    variables: &BTreeMap<String, String>,
    sanitize: fn(&str) -> String,
) -> Result<String, EmailTemplateError> {
    let mut output = String::with_capacity(text.len());
    for segment in parse_segments("template", text)? {
        match segment {
            Segment::Text(literal) => output.push_str(literal),
            Segment::Variable(name) => {
                if kind.variables().contains(&name)
                    && let Some(value) = variables.get(name)
                {
                    output.push_str(&sanitize(value));
                }
            }
        }
    }
    Ok(output)
}

fn sanitize_subject_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn sanitize_body_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

/// Reads, customizes, resets, and previews an environment's templates with
/// the same membership rules as its other environment-scoped resources:
/// any member may read; a role that can change projects may write.
#[derive(Clone)]
pub struct EmailTemplateService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for EmailTemplateService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmailTemplateService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl EmailTemplateService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Result<Self, EmailTemplateError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(EmailTemplateError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
        })
    }

    /// Every kind, customized or default, in a fixed order.
    pub async fn list(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<Vec<EmailTemplateView>, EmailTemplateError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::EmailTemplateRead,
                false,
                now_unix_seconds,
            )
            .await?;
        let stored = self.stored_templates(tenant).await?;
        let views = EmailTemplateKind::ALL
            .into_iter()
            .map(|kind| {
                stored.get(&kind).cloned().map_or_else(
                    || EmailTemplateView::default_for(kind),
                    EmailTemplateView::from_record,
                )
            })
            .collect();
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::EmailTemplateRead,
            "email-templates",
            now_unix_seconds,
        );
        Ok(views)
    }

    pub async fn get(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
        now_unix_seconds: u64,
    ) -> Result<EmailTemplateView, EmailTemplateError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::EmailTemplateRead,
                false,
                now_unix_seconds,
            )
            .await?;
        let view = self.stored_template(tenant, kind).await?.map_or_else(
            || EmailTemplateView::default_for(kind),
            EmailTemplateView::from_record,
        );
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::EmailTemplateRead,
            kind.as_str(),
            now_unix_seconds,
        );
        Ok(view)
    }

    /// Validates and stores a customization; the version advances on every
    /// save and a concurrent save of the same kind is refused as a conflict.
    pub async fn update(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
        text: EmailTemplateText,
        now_unix_seconds: u64,
    ) -> Result<EmailTemplateView, EmailTemplateError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::EmailTemplateUpdate,
                true,
                now_unix_seconds,
            )
            .await?;
        text.validate(kind)?;
        let key = ControlKeyspace::email_template_key(
            tenant.project_id(),
            tenant.environment_id(),
            kind,
        )?;
        let previous = self.stored_template(tenant, kind).await?;
        let next = EmailTemplateRecord {
            tenant: tenant.clone(),
            kind,
            subject: text.subject,
            text_body: text.text_body,
            version: previous
                .as_ref()
                .map_or(0, EmailTemplateRecord::version)
                .checked_add(1)
                .ok_or(EmailTemplateError::CorruptRecord)?,
            updated_at_unix_seconds: now_unix_seconds,
        };
        let condition = match &previous {
            Some(previous) => KeyCondition::ValueEquals {
                key: key.clone(),
                value: serde_json::to_vec(previous)?,
            },
            None => KeyCondition::Missing { key: key.clone() },
        };
        let mut batch = WriteBatch::with_capacity(1);
        batch.put(&key, serde_json::to_vec(&next)?);
        self.apply(AtomicWrite {
            conditions: vec![condition],
            batch,
            durability: self.durability,
        })
        .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::EmailTemplateUpdate,
            kind.as_str(),
            now_unix_seconds,
        );
        Ok(EmailTemplateView::from_record(next))
    }

    /// Removes the customization so the built-in default applies again.
    /// Resetting a kind that was never customized is a no-op that still
    /// answers with the default.
    pub async fn reset(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
        now_unix_seconds: u64,
    ) -> Result<EmailTemplateView, EmailTemplateError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::EmailTemplateUpdate,
                true,
                now_unix_seconds,
            )
            .await?;
        let key = ControlKeyspace::email_template_key(
            tenant.project_id(),
            tenant.environment_id(),
            kind,
        )?;
        if let Some(previous) = self.stored_template(tenant, kind).await? {
            let mut batch = WriteBatch::with_capacity(1);
            batch.delete(&key);
            self.apply(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key,
                    value: serde_json::to_vec(&previous)?,
                }],
                batch,
                durability: self.durability,
            })
            .await?;
        }
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::EmailTemplateUpdate,
            kind.as_str(),
            now_unix_seconds,
        );
        Ok(EmailTemplateView::default_for(kind))
    }

    /// Renders the effective template with placeholder data and the
    /// environment's real names. Either part may be overridden with unsaved
    /// text; whatever is not given comes from the stored template or the
    /// default, so a developer can preview a subject before saving it.
    pub async fn preview(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
        subject: Option<String>,
        text_body: Option<String>,
        now_unix_seconds: u64,
    ) -> Result<EmailTemplateRender, EmailTemplateError> {
        let organization = self
            .authorize(
                actor,
                tenant,
                ControlAuditAction::EmailTemplateRead,
                false,
                now_unix_seconds,
            )
            .await?;
        let (project_name, environment_name) = self.names(tenant).await?;
        let mut resolved = self.resolve(tenant, kind).await?;
        if let Some(subject) = subject {
            resolved.subject = subject;
            resolved.is_default = false;
        }
        if let Some(text_body) = text_body {
            resolved.text_body = text_body;
            resolved.is_default = false;
        }
        EmailTemplateText {
            subject: resolved.subject.clone(),
            text_body: resolved.text_body.clone(),
        }
        .validate(kind)?;
        let render = resolved.render(&preview_variables(kind, &project_name, &environment_name))?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::EmailTemplateRead,
            kind.as_str(),
            now_unix_seconds,
        );
        Ok(render)
    }

    /// The template delivery applies for `kind`. No authorization: the
    /// caller is the mail worker acting on an intent the data plane issued.
    pub async fn resolve(
        &self,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
    ) -> Result<ResolvedEmailTemplate, EmailTemplateError> {
        Ok(match self.stored_template(tenant, kind).await? {
            Some(record) => ResolvedEmailTemplate {
                kind,
                subject: record.subject,
                text_body: record.text_body,
                is_default: false,
            },
            None => ResolvedEmailTemplate {
                kind,
                subject: kind.default_subject().to_owned(),
                text_body: kind.default_text_body().to_owned(),
                is_default: true,
            },
        })
    }

    /// The project and environment names for a tenant, or `NotFound` when
    /// either record is missing.
    pub async fn names(
        &self,
        tenant: &TenantScope,
    ) -> Result<(String, String), EmailTemplateError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(EmailTemplateError::NotFound)?;
        let environment = self
            .projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(EmailTemplateError::NotFound)?;
        Ok((project.name().to_owned(), environment.name().to_owned()))
    }

    async fn stored_template(
        &self,
        tenant: &TenantScope,
        kind: EmailTemplateKind,
    ) -> Result<Option<EmailTemplateRecord>, EmailTemplateError> {
        let key = ControlKeyspace::email_template_key(
            tenant.project_id(),
            tenant.environment_id(),
            kind,
        )?;
        let Some(value) = self.adapter.get(&key).await? else {
            return Ok(None);
        };
        let record: EmailTemplateRecord = serde_json::from_slice(&value)?;
        if record.tenant != *tenant || record.kind != kind {
            return Err(EmailTemplateError::CorruptRecord);
        }
        Ok(Some(record))
    }

    async fn stored_templates(
        &self,
        tenant: &TenantScope,
    ) -> Result<BTreeMap<EmailTemplateKind, EmailTemplateRecord>, EmailTemplateError> {
        let values = self
            .adapter
            .scan(ScanRequest::new(
                ControlKeyspace::email_templates_range(
                    tenant.project_id(),
                    tenant.environment_id(),
                )?,
                ScanDirection::Forward,
                NonZeroUsize::new(EmailTemplateKind::ALL.len() + 1).expect("constant is positive"),
            ))
            .await?;
        let mut stored = BTreeMap::new();
        for value in values {
            let record: EmailTemplateRecord = serde_json::from_slice(&value.value)?;
            if record.tenant != *tenant {
                return Err(EmailTemplateError::CorruptRecord);
            }
            stored.insert(record.kind, record);
        }
        Ok(stored)
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), EmailTemplateError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(EmailTemplateError::Conflict),
        }
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        action: ControlAuditAction,
        write: bool,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, EmailTemplateError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(EmailTemplateError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(EmailTemplateError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        let allowed =
            membership.is_some_and(|membership| !write || membership.role().can_mutate_projects());
        if !allowed {
            self.audit(
                actor,
                project.organization_id(),
                tenant,
                action,
                "authorization",
                now_unix_seconds,
            );
            return Err(EmailTemplateError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationId,
        tenant: &TenantScope,
        action: ControlAuditAction,
        target: &str,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}/email-templates/{}",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str(),
                target
            ),
            outcome: if target == "authorization" {
                ControlAuditOutcome::Denied
            } else {
                ControlAuditOutcome::Allowed
            },
            at_unix_seconds,
        });
    }
}

#[derive(Debug)]
pub enum EmailTemplateError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    /// The template text was refused; the message names the field and why.
    InvalidTemplate(String),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for EmailTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDurability => {
                formatter.write_str("email template durability is unsupported")
            }
            Self::NotFound => formatter.write_str("environment was not found"),
            Self::Forbidden => formatter.write_str("email template operation is forbidden"),
            Self::Conflict => formatter.write_str("email template changed concurrently"),
            Self::CorruptRecord => formatter.write_str("email template record is corrupt"),
            Self::InvalidTemplate(message) => formatter.write_str(message),
            Self::Project(_) => formatter.write_str("email template project lookup failed"),
            Self::Organization(_) => {
                formatter.write_str("email template organization lookup failed")
            }
            Self::Keyspace(_) => formatter.write_str("email template key is invalid"),
            Self::Storage(_) => formatter.write_str("email template storage operation failed"),
            Self::Json(_) => formatter.write_str("email template record is invalid"),
        }
    }
}

impl Error for EmailTemplateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Project(error) => Some(error),
            Self::Organization(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProjectStoreError> for EmailTemplateError {
    fn from(value: ProjectStoreError) -> Self {
        Self::Project(value)
    }
}

impl From<OrganizationStoreError> for EmailTemplateError {
    fn from(value: OrganizationStoreError) -> Self {
        Self::Organization(value)
    }
}

impl From<ControlKeyspaceError> for EmailTemplateError {
    fn from(value: ControlKeyspaceError) -> Self {
        Self::Keyspace(value)
    }
}

impl From<StorageError> for EmailTemplateError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<serde_json::Error> for EmailTemplateError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        DeveloperIdentityId, EnvironmentRecord, MembershipRecord, OrganizationRecord,
        OrganizationRole, ProjectRecord,
    };

    pub(crate) const NOW: u64 = 1_800_000_000;

    #[derive(Default)]
    pub(crate) struct RecordingAudit(pub(crate) Mutex<Vec<ControlAuditEvent>>);

    impl ControlAuditSink for RecordingAudit {
        fn record(&self, event: ControlAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    pub(crate) struct Fixture {
        pub(crate) adapter: Arc<MemoryAdapter>,
        pub(crate) audit: Arc<RecordingAudit>,
        pub(crate) service: EmailTemplateService,
        pub(crate) tenant: TenantScope,
        pub(crate) owner: DeveloperPrincipal,
        pub(crate) viewer: DeveloperPrincipal,
        pub(crate) stranger: DeveloperPrincipal,
    }

    /// A team with an owner and a viewer, one project, one environment.
    pub(crate) fn fixture() -> Fixture {
        let adapter = Arc::new(MemoryAdapter::new());
        let kv: Arc<dyn KvAdapter> = adapter.clone();
        let organizations =
            OrganizationStore::new(Arc::clone(&kv), Durability::Memory).expect("organizations");
        let projects = ProjectStore::new(Arc::clone(&kv), Durability::Memory).expect("projects");
        let audit = Arc::new(RecordingAudit::default());
        let service = EmailTemplateService::new(
            Arc::clone(&kv),
            Durability::Memory,
            projects.clone(),
            organizations.clone(),
            audit.clone(),
        )
        .expect("service");
        let owner_id = DeveloperIdentityId::parse("dev_owner00000001").expect("owner id");
        let viewer_id = DeveloperIdentityId::parse("dev_viewer0000001").expect("viewer id");
        let stranger_id = DeveloperIdentityId::parse("dev_stranger00001").expect("stranger id");
        let organization_id = OrganizationId::parse("org_mailteam00001").expect("organization id");
        let project_id = ProjectId::parse("prj_mailproject01").expect("project id");
        let environment_id = EnvironmentId::parse("env_mailenviron01").expect("environment id");
        block_on(async {
            let organization = OrganizationRecord::new(organization_id.clone(), "Mail Team", NOW)
                .expect("organization");
            organizations
                .create_organization(
                    &organization,
                    &MembershipRecord::new(
                        organization_id.clone(),
                        owner_id.clone(),
                        OrganizationRole::Owner,
                        NOW,
                    ),
                )
                .await
                .expect("organization created");
            // The store only creates memberships through invitations; the
            // viewer is written directly, as an accepted invitation would.
            let viewer = MembershipRecord::new(
                organization_id.clone(),
                viewer_id.clone(),
                OrganizationRole::Viewer,
                NOW,
            );
            let mut batch = WriteBatch::with_capacity(1);
            batch.put(
                ControlKeyspace::membership_key(&organization_id, &viewer_id).expect("key"),
                serde_json::to_vec(&viewer).expect("membership"),
            );
            kv.compare_and_write(AtomicWrite {
                conditions: Vec::new(),
                batch,
                durability: Durability::Memory,
            })
            .await
            .expect("viewer membership");
            let project = ProjectRecord::new(
                project_id.clone(),
                organization_id.clone(),
                "Field Notes",
                "local",
                NOW,
            )
            .expect("project");
            projects
                .create_project(&project)
                .await
                .expect("project created");
            let environment =
                EnvironmentRecord::new(environment_id.clone(), project_id.clone(), "Staging", NOW)
                    .expect("environment");
            projects
                .create_environment(&environment)
                .await
                .expect("environment created");
        });
        Fixture {
            adapter,
            audit,
            service,
            tenant: TenantScope::new(project_id, environment_id),
            owner: DeveloperPrincipal::for_test(owner_id, "owner@example.test"),
            viewer: DeveloperPrincipal::for_test(viewer_id, "viewer@example.test"),
            stranger: DeveloperPrincipal::for_test(stranger_id, "stranger@example.test"),
        }
    }

    fn variables(kind: EmailTemplateKind) -> BTreeMap<String, String> {
        let mut variables = preview_variables(kind, "Field Notes", "Staging");
        variables.insert(
            "link".to_owned(),
            "https://notes.example.com/auth?token=abc123".to_owned(),
        );
        variables
    }

    #[test]
    fn defaults_render_with_the_link_and_expiry_for_every_kind() {
        for kind in EmailTemplateKind::ALL {
            let render = render_template(
                kind,
                kind.default_subject(),
                kind.default_text_body(),
                &variables(kind),
            )
            .expect("default renders");
            assert!(
                render
                    .text_body
                    .contains("https://notes.example.com/auth?token=abc123")
            );
            assert!(render.text_body.contains("2030-01-01T12:00:00Z"));
            assert!(render.text_body.contains("Field Notes"));
            assert!(render.subject.contains("Field Notes"));
            assert!(!render.subject.contains("{{"));
            assert!(!render.text_body.contains("{{"));
        }
    }

    #[test]
    fn unknown_variables_and_unbalanced_braces_are_refused_at_validation() {
        let kind = EmailTemplateKind::Verification;
        let refused = |subject: &str, body: &str| {
            EmailTemplateText {
                subject: subject.to_owned(),
                text_body: body.to_owned(),
            }
            .validate(kind)
            .expect_err("refused")
        };
        assert!(matches!(
            refused("Hi {{inviter}}", "body {{link}}"),
            EmailTemplateError::InvalidTemplate(message) if message.contains("unknown variable {{inviter}}")
        ));
        assert!(matches!(
            refused("Hi", "open {{link"),
            EmailTemplateError::InvalidTemplate(message) if message.contains("not closed")
        ));
        assert!(matches!(
            refused("Hi", "stray } brace"),
            EmailTemplateError::InvalidTemplate(message) if message.contains("braces are only allowed")
        ));
        assert!(matches!(
            refused("Hi", "{ {link} }"),
            EmailTemplateError::InvalidTemplate(_)
        ));
        assert!(matches!(
            refused("Hi", "{{Link}}"),
            EmailTemplateError::InvalidTemplate(message) if message.contains("placeholder name is invalid")
        ));
        assert!(matches!(
            refused("Line\nbreak", "{{link}}"),
            EmailTemplateError::InvalidTemplate(message) if message.starts_with("subject:")
        ));
        assert!(matches!(
            refused("", "{{link}}"),
            EmailTemplateError::InvalidTemplate(_)
        ));
        assert!(matches!(
            refused("Hi", &"x".repeat(MAXIMUM_TEXT_BODY_BYTES + 1)),
            EmailTemplateError::InvalidTemplate(_)
        ));
        // A mail without its link is refused: the reader could act on nothing.
        assert!(matches!(
            refused("Welcome", "Welcome aboard. See you soon."),
            EmailTemplateError::InvalidTemplate(message) if message.contains("must include {{link}}")
        ));
        assert!(matches!(
            refused("Your {{link}}", "The link is in the subject only."),
            EmailTemplateError::InvalidTemplate(message) if message.contains("must include {{link}}")
        ));
        // The inviter is only an invitation variable.
        EmailTemplateText {
            subject: "{{inviter}} invited you".to_owned(),
            text_body: "{{link}}".to_owned(),
        }
        .validate(EmailTemplateKind::Invitation)
        .expect("invitation may name the inviter");
        // Whitespace inside the braces is tolerated; the name is what counts.
        EmailTemplateText {
            subject: "Sign in to {{ project_name }}".to_owned(),
            text_body: "{{ link }}".to_owned(),
        }
        .validate(kind)
        .expect("padded placeholder");
    }

    #[test]
    fn rendering_substitutes_only_allowlisted_values_and_sanitizes_them() {
        let mut variables = variables(EmailTemplateKind::Invitation);
        variables.insert(
            "inviter".to_owned(),
            "Eve\r\nBcc: victim@example.com".to_owned(),
        );
        variables.insert("email".to_owned(), "new\u{7}person@example.com".to_owned());
        let render = render_template(
            EmailTemplateKind::Invitation,
            "{{inviter}} invited you to {{project_name}}",
            "Hi {{email}}, {{inviter}} says:\n{{link}}\n",
            &variables,
        )
        .expect("renders");
        assert_eq!(
            render.subject,
            "Eve  Bcc: victim@example.com invited you to Field Notes"
        );
        assert_eq!(
            render.text_body,
            "Hi newperson@example.com, Eve\nBcc: victim@example.com says:\nhttps://notes.example.com/auth?token=abc123\n"
        );
        // A variable the caller did not send renders empty rather than as a placeholder.
        let mut missing = variables.clone();
        missing.remove("inviter");
        let render = render_template(
            EmailTemplateKind::Invitation,
            "{{inviter}}",
            "Invited by: {{inviter}}. {{link}}",
            &missing,
        )
        .expect("renders");
        assert_eq!(
            render.text_body,
            "Invited by: . https://notes.example.com/auth?token=abc123"
        );
        // ... and an empty subject falls back to the default rather than sending a blank line.
        assert_eq!(render.subject, "You are invited to Field Notes");
        // Rendering never accepts what validation refuses.
        assert!(
            render_template(
                EmailTemplateKind::Recovery,
                "Reset",
                "{{secret}}",
                &variables
            )
            .is_err()
        );
    }

    #[test]
    fn members_read_templates_and_only_mutating_roles_change_them() {
        let fixture = fixture();
        block_on(async {
            let listed = fixture
                .service
                .list(&fixture.viewer, &fixture.tenant, NOW)
                .await
                .expect("viewer lists");
            assert_eq!(listed.len(), 4);
            assert!(
                listed
                    .iter()
                    .all(|view| view.is_default && view.version == 0)
            );
            assert_eq!(
                listed.iter().map(|view| view.kind).collect::<Vec<_>>(),
                EmailTemplateKind::ALL
            );
            let text = EmailTemplateText {
                subject: "Welcome to {{project_name}}".to_owned(),
                text_body: "Open {{link}} before {{expires_at}}.".to_owned(),
            };
            assert!(matches!(
                fixture
                    .service
                    .update(
                        &fixture.viewer,
                        &fixture.tenant,
                        EmailTemplateKind::Verification,
                        text.clone(),
                        NOW
                    )
                    .await,
                Err(EmailTemplateError::Forbidden)
            ));
            assert!(matches!(
                fixture
                    .service
                    .list(&fixture.stranger, &fixture.tenant, NOW)
                    .await,
                Err(EmailTemplateError::Forbidden)
            ));
            let missing = TenantScope::new(
                fixture.tenant.project_id().clone(),
                EnvironmentId::parse("env_doesnotexist1").expect("environment id"),
            );
            assert!(matches!(
                fixture.service.list(&fixture.owner, &missing, NOW).await,
                Err(EmailTemplateError::NotFound)
            ));
            let saved = fixture
                .service
                .update(
                    &fixture.owner,
                    &fixture.tenant,
                    EmailTemplateKind::Verification,
                    text.clone(),
                    NOW,
                )
                .await
                .expect("owner saves");
            assert_eq!(saved.version, 1);
            assert!(!saved.is_default);
            assert_eq!(saved.updated_at_unix_seconds, Some(NOW));
            let again = fixture
                .service
                .update(
                    &fixture.owner,
                    &fixture.tenant,
                    EmailTemplateKind::Verification,
                    EmailTemplateText {
                        subject: "Confirm {{email}}".to_owned(),
                        ..text.clone()
                    },
                    NOW + 5,
                )
                .await
                .expect("owner saves again");
            assert_eq!(again.version, 2);
            assert_eq!(again.updated_at_unix_seconds, Some(NOW + 5));
            let read = fixture
                .service
                .get(
                    &fixture.viewer,
                    &fixture.tenant,
                    EmailTemplateKind::Verification,
                    NOW + 6,
                )
                .await
                .expect("viewer reads");
            assert_eq!(read.subject, "Confirm {{email}}");
            assert_eq!(read.version, 2);
            let listed = fixture
                .service
                .list(&fixture.owner, &fixture.tenant, NOW + 6)
                .await
                .expect("list");
            assert_eq!(listed.iter().filter(|view| !view.is_default).count(), 1);
            // Invalid text is refused before anything is stored.
            assert!(matches!(
                fixture
                    .service
                    .update(
                        &fixture.owner,
                        &fixture.tenant,
                        EmailTemplateKind::Recovery,
                        EmailTemplateText {
                            subject: "Reset".to_owned(),
                            text_body: "{{password}}".to_owned(),
                        },
                        NOW,
                    )
                    .await,
                Err(EmailTemplateError::InvalidTemplate(_))
            ));
            // Delivery sees the customization; the other kinds stay default.
            let resolved = fixture
                .service
                .resolve(&fixture.tenant, EmailTemplateKind::Verification)
                .await
                .expect("resolve");
            assert!(!resolved.is_default);
            assert_eq!(resolved.subject, "Confirm {{email}}");
            assert!(
                fixture
                    .service
                    .resolve(&fixture.tenant, EmailTemplateKind::Recovery)
                    .await
                    .expect("resolve")
                    .is_default
            );
            // Reset goes back to the default; resetting again is harmless.
            assert!(matches!(
                fixture
                    .service
                    .reset(
                        &fixture.viewer,
                        &fixture.tenant,
                        EmailTemplateKind::Verification,
                        NOW
                    )
                    .await,
                Err(EmailTemplateError::Forbidden)
            ));
            let reset = fixture
                .service
                .reset(
                    &fixture.owner,
                    &fixture.tenant,
                    EmailTemplateKind::Verification,
                    NOW + 7,
                )
                .await
                .expect("reset");
            assert!(reset.is_default);
            assert_eq!(reset.version, 0);
            assert!(
                fixture
                    .service
                    .reset(
                        &fixture.owner,
                        &fixture.tenant,
                        EmailTemplateKind::Verification,
                        NOW + 8
                    )
                    .await
                    .expect("reset again")
                    .is_default
            );
            assert!(
                fixture
                    .service
                    .resolve(&fixture.tenant, EmailTemplateKind::Verification)
                    .await
                    .expect("resolve")
                    .is_default
            );
            // A fresh customization starts its version count over.
            let fresh = fixture
                .service
                .update(
                    &fixture.owner,
                    &fixture.tenant,
                    EmailTemplateKind::Verification,
                    text,
                    NOW + 9,
                )
                .await
                .expect("save after reset");
            assert_eq!(fresh.version, 1);
        });
        let events = fixture.audit.0.lock().expect("audit lock");
        assert!(events.iter().any(|event| {
            event.action == ControlAuditAction::EmailTemplateUpdate
                && event.outcome == ControlAuditOutcome::Denied
        }));
        assert!(events.iter().any(|event| {
            event.action == ControlAuditAction::EmailTemplateUpdate
                && event.outcome == ControlAuditOutcome::Allowed
                && event.target.ends_with("/email-templates/verification")
        }));
        assert!(events.iter().any(|event| {
            event.action == ControlAuditAction::EmailTemplateRead
                && event.outcome == ControlAuditOutcome::Denied
        }));
    }

    #[test]
    fn preview_renders_placeholder_data_with_the_real_names() {
        let fixture = fixture();
        block_on(async {
            let stored = fixture
                .service
                .preview(
                    &fixture.viewer,
                    &fixture.tenant,
                    EmailTemplateKind::MagicLink,
                    None,
                    None,
                    NOW,
                )
                .await
                .expect("preview of the default");
            assert_eq!(stored.subject, "Your sign-in link for Field Notes");
            assert!(stored.text_body.contains("Field Notes (Staging)"));
            assert!(stored.text_body.contains("person@example.com"));
            assert!(
                stored
                    .text_body
                    .contains("https://app.example.com/auth/callback?token=preview-token")
            );
            let unsaved = fixture
                .service
                .preview(
                    &fixture.viewer,
                    &fixture.tenant,
                    EmailTemplateKind::MagicLink,
                    Some("Sign in to {{environment_name}}".to_owned()),
                    Some("{{link}}".to_owned()),
                    NOW,
                )
                .await
                .expect("preview of unsaved text");
            assert_eq!(unsaved.subject, "Sign in to Staging");
            assert_eq!(
                unsaved.text_body,
                "https://app.example.com/auth/callback?token=preview-token"
            );
            // One part at a time: the other comes from what is in effect.
            let subject_only = fixture
                .service
                .preview(
                    &fixture.viewer,
                    &fixture.tenant,
                    EmailTemplateKind::MagicLink,
                    Some("Open {{project_name}}".to_owned()),
                    None,
                    NOW,
                )
                .await
                .expect("preview of an unsaved subject");
            assert_eq!(subject_only.subject, "Open Field Notes");
            assert_eq!(subject_only.text_body, stored.text_body);
            assert!(matches!(
                fixture
                    .service
                    .preview(
                        &fixture.viewer,
                        &fixture.tenant,
                        EmailTemplateKind::MagicLink,
                        Some("Sign in".to_owned()),
                        Some("{{inviter}}".to_owned()),
                        NOW,
                    )
                    .await,
                Err(EmailTemplateError::InvalidTemplate(message)) if message.contains("unknown variable")
            ));
            assert!(matches!(
                fixture
                    .service
                    .preview(
                        &fixture.stranger,
                        &fixture.tenant,
                        EmailTemplateKind::MagicLink,
                        None,
                        None,
                        NOW
                    )
                    .await,
                Err(EmailTemplateError::Forbidden)
            ));
        });
    }

    #[test]
    fn kinds_have_stable_wire_names() {
        for kind in EmailTemplateKind::ALL {
            assert_eq!(EmailTemplateKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_string(&kind).expect("kind"),
                format!("\"{}\"", kind.as_str())
            );
        }
        assert_eq!(EmailTemplateKind::parse("magic-link"), None);
        assert_eq!(EmailTemplateKind::parse("Verification"), None);
    }
}
