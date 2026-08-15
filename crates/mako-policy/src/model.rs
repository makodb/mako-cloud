use std::{collections::BTreeSet, error::Error, fmt, num::NonZeroU64};

use mako_api::CollectionScope;
use serde::{Deserialize, Serialize};

const POLICY_SET_ENCODING_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PolicyVersion(NonZeroU64);

impl PolicyVersion {
    pub fn new(value: u64) -> Result<Self, PolicyModelError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(PolicyModelError::InvalidField {
                field: "version",
                reason: "must be positive",
            })
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOperation {
    Create,
    Read,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyState {
    Draft,
    Validated,
    Active,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PolicyRuleId(String);

impl PolicyRuleId {
    pub fn parse(value: impl Into<String>) -> Result<Self, PolicyModelError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value.chars().all(|character| {
                character == '-' || character == '_' || character.is_ascii_alphanumeric()
            });
        if !valid {
            return Err(PolicyModelError::InvalidField {
                field: "rule.id",
                reason: "must contain 1-128 ASCII letters, digits, hyphens, or underscores",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    line: u32,
    column: u32,
    length: u32,
}

impl SourceSpan {
    pub fn new(line: u32, column: u32, length: u32) -> Result<Self, PolicyModelError> {
        if line == 0 || column == 0 || length == 0 {
            return Err(PolicyModelError::InvalidField {
                field: "diagnostic.span",
                reason: "line, column, and length must be positive",
            });
        }
        Ok(Self {
            line,
            column,
            length,
        })
    }

    #[must_use]
    pub const fn line(self) -> u32 {
        self.line
    }

    #[must_use]
    pub const fn column(self) -> u32 {
        self.column
    }

    #[must_use]
    pub const fn length(self) -> u32 {
        self.length
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDiagnostic {
    severity: DiagnosticSeverity,
    code: String,
    message: String,
    span: Option<SourceSpan>,
}

impl PolicyDiagnostic {
    pub fn new(
        severity: DiagnosticSeverity,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Option<SourceSpan>,
    ) -> Result<Self, PolicyModelError> {
        let code = code.into();
        let message = message.into();
        if code.is_empty()
            || code.len() > 64
            || !code
                .chars()
                .all(|character| character == '_' || character.is_ascii_lowercase())
        {
            return Err(PolicyModelError::InvalidField {
                field: "diagnostic.code",
                reason: "must contain 1-64 lowercase ASCII letters or underscores",
            });
        }
        if message.is_empty() || message.len() > 512 || message.chars().any(char::is_control) {
            return Err(PolicyModelError::InvalidField {
                field: "diagnostic.message",
                reason: "must contain 1-512 non-control characters",
            });
        }
        Ok(Self {
            severity,
            code,
            message,
            span,
        })
    }

    #[must_use]
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn span(&self) -> Option<SourceSpan> {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyRule {
    id: PolicyRuleId,
    effect: PolicyEffect,
    operations: BTreeSet<DocumentOperation>,
    expression: String,
}

impl PolicyRule {
    pub fn new(
        id: PolicyRuleId,
        effect: PolicyEffect,
        operations: impl IntoIterator<Item = DocumentOperation>,
        expression: impl Into<String>,
    ) -> Result<Self, PolicyModelError> {
        let operations: BTreeSet<_> = operations.into_iter().collect();
        if operations.is_empty() {
            return Err(PolicyModelError::InvalidField {
                field: "rule.operations",
                reason: "at least one operation is required",
            });
        }
        let expression = expression.into();
        if expression.trim().is_empty() || expression.len() > 16 * 1024 {
            return Err(PolicyModelError::InvalidField {
                field: "rule.expression",
                reason: "must contain 1-16384 characters",
            });
        }
        Ok(Self {
            id,
            effect,
            operations,
            expression,
        })
    }

    #[must_use]
    pub fn id(&self) -> &PolicyRuleId {
        &self.id
    }

    #[must_use]
    pub const fn effect(&self) -> PolicyEffect {
        self.effect
    }

    #[must_use]
    pub fn operations(&self) -> &BTreeSet<DocumentOperation> {
        &self.operations
    }

    #[must_use]
    pub fn expression(&self) -> &str {
        &self.expression
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicySet {
    scope: CollectionScope,
    version: PolicyVersion,
    state: PolicyState,
    rules: Vec<PolicyRule>,
    diagnostics: Vec<PolicyDiagnostic>,
}

impl PolicySet {
    pub fn new(
        scope: CollectionScope,
        version: PolicyVersion,
        state: PolicyState,
        rules: impl IntoIterator<Item = PolicyRule>,
        diagnostics: impl IntoIterator<Item = PolicyDiagnostic>,
    ) -> Result<Self, PolicyModelError> {
        let mut rules: Vec<_> = rules.into_iter().collect();
        let diagnostics: Vec<_> = diagnostics.into_iter().collect();
        let mut ids = BTreeSet::new();
        if rules.iter().any(|rule| !ids.insert(rule.id().clone())) {
            return Err(PolicyModelError::DuplicateRuleId);
        }
        if state == PolicyState::Active
            && diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
        {
            return Err(PolicyModelError::ActiveSetHasErrors);
        }
        rules.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self {
            scope,
            version,
            state,
            rules,
            diagnostics,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    #[must_use]
    pub const fn version(&self) -> PolicyVersion {
        self.version
    }

    #[must_use]
    pub const fn state(&self) -> PolicyState {
        self.state
    }

    #[must_use]
    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[PolicyDiagnostic] {
        &self.diagnostics
    }

    pub fn encode(&self) -> Result<Vec<u8>, PolicyModelError> {
        serde_json::to_vec(&PolicySetWire::from(self)).map_err(PolicyModelError::Json)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PolicyModelError> {
        let wire: PolicySetWire = serde_json::from_slice(bytes).map_err(PolicyModelError::Json)?;
        if wire.encoding_version != POLICY_SET_ENCODING_VERSION {
            return Err(PolicyModelError::UnsupportedEncoding(wire.encoding_version));
        }
        let rules = wire
            .rules
            .into_iter()
            .map(PolicyRuleWire::into_rule)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            wire.scope,
            PolicyVersion::new(wire.version)?,
            wire.state,
            rules,
            wire.diagnostics,
        )
    }

    pub fn with_state(
        &self,
        state: PolicyState,
        diagnostics: Vec<PolicyDiagnostic>,
    ) -> Result<Self, PolicyModelError> {
        Self::new(
            self.scope.clone(),
            self.version,
            state,
            self.rules.clone(),
            diagnostics,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PolicySetWire {
    encoding_version: u8,
    scope: CollectionScope,
    version: u64,
    state: PolicyState,
    rules: Vec<PolicyRuleWire>,
    diagnostics: Vec<PolicyDiagnostic>,
}

impl From<&PolicySet> for PolicySetWire {
    fn from(policy: &PolicySet) -> Self {
        Self {
            encoding_version: POLICY_SET_ENCODING_VERSION,
            scope: policy.scope.clone(),
            version: policy.version.get(),
            state: policy.state,
            rules: policy.rules.iter().map(PolicyRuleWire::from).collect(),
            diagnostics: policy.diagnostics.clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PolicyRuleWire {
    id: PolicyRuleId,
    effect: PolicyEffect,
    operations: BTreeSet<DocumentOperation>,
    expression: String,
}

impl From<&PolicyRule> for PolicyRuleWire {
    fn from(rule: &PolicyRule) -> Self {
        Self {
            id: rule.id.clone(),
            effect: rule.effect,
            operations: rule.operations.clone(),
            expression: rule.expression.clone(),
        }
    }
}

impl PolicyRuleWire {
    fn into_rule(self) -> Result<PolicyRule, PolicyModelError> {
        PolicyRule::new(self.id, self.effect, self.operations, self.expression)
    }
}

#[derive(Debug)]
pub enum PolicyModelError {
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    DuplicateRuleId,
    ActiveSetHasErrors,
    UnsupportedEncoding(u8),
    Json(serde_json::Error),
}

impl fmt::Display for PolicyModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::DuplicateRuleId => formatter.write_str("policy rule IDs must be unique"),
            Self::ActiveSetHasErrors => {
                formatter.write_str("an active policy set cannot contain error diagnostics")
            }
            Self::UnsupportedEncoding(version) => {
                write!(
                    formatter,
                    "unsupported policy-set encoding version {version}"
                )
            }
            Self::Json(error) => write!(formatter, "invalid policy-set encoding: {error}"),
        }
    }
}

impl Error for PolicyModelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, EnvironmentId, ProjectId, TenantScope};

    use super::*;

    #[test]
    fn versioned_operation_rules_round_trip_deterministically() {
        let update = PolicyRule::new(
            PolicyRuleId::parse("owner-update").expect("id"),
            PolicyEffect::Allow,
            [DocumentOperation::Update],
            "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
        )
        .expect("rule");
        let deny = PolicyRule::new(
            PolicyRuleId::parse("blocked-deny").expect("id"),
            PolicyEffect::Deny,
            [DocumentOperation::Read, DocumentOperation::Update],
            "new.blocked == true",
        )
        .expect("rule");
        let policy = PolicySet::new(
            scope(),
            PolicyVersion::new(2).expect("version"),
            PolicyState::Draft,
            [update, deny],
            [],
        )
        .expect("policy");

        let encoded = policy.encode().expect("encode");
        assert_eq!(PolicySet::decode(&encoded).expect("decode"), policy);
        assert_eq!(policy.rules()[0].id().as_str(), "blocked-deny");
    }

    #[test]
    fn invalid_active_diagnostics_and_duplicate_rules_are_rejected() {
        let diagnostic = PolicyDiagnostic::new(
            DiagnosticSeverity::Error,
            "unknown_field",
            "field does not exist in the collection schema",
            Some(SourceSpan::new(1, 4, 8).expect("span")),
        )
        .expect("diagnostic");
        assert!(matches!(
            PolicySet::new(
                scope(),
                PolicyVersion::new(1).expect("version"),
                PolicyState::Active,
                [],
                [diagnostic]
            ),
            Err(PolicyModelError::ActiveSetHasErrors)
        ));

        let rule = PolicyRule::new(
            PolicyRuleId::parse("read").expect("id"),
            PolicyEffect::Allow,
            [DocumentOperation::Read],
            "true",
        )
        .expect("rule");
        assert!(matches!(
            PolicySet::new(
                scope(),
                PolicyVersion::new(1).expect("version"),
                PolicyState::Draft,
                [rule.clone(), rule],
                []
            ),
            Err(PolicyModelError::DuplicateRuleId)
        ));
    }

    fn scope() -> CollectionScope {
        CollectionScope::new(
            TenantScope::new(
                ProjectId::parse("prj_abcdefgh").expect("project"),
                EnvironmentId::parse("env_abcdefgh").expect("environment"),
            ),
            CollectionId::parse("todos").expect("collection"),
        )
    }
}
