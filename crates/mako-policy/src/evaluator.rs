use std::{error::Error, fmt};

use serde_json::Value;

use crate::{
    CompiledPolicySet, DocumentOperation, PolicyEffect, PolicyEvaluationContext, PolicyRuleId,
    compiler::{Comparison, Expression},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyOutcome {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyDecisionCode {
    Allowed,
    ExplicitDeny,
    NoMatchingAllow,
    ScopeMismatch,
    EvaluationFailed,
    CostLimitExceeded,
    PolicyUnavailable,
    EvaluationTimedOut,
    EvaluatorUnavailable,
    ContextInvalid,
    InternalError,
}

impl PolicyDecisionCode {
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::Allowed => "policy_allowed",
            Self::ExplicitDeny => "policy_explicit_deny",
            Self::NoMatchingAllow => "policy_default_deny",
            Self::ScopeMismatch => "policy_scope_mismatch",
            Self::EvaluationFailed => "policy_evaluation_failed",
            Self::CostLimitExceeded => "policy_cost_limit_exceeded",
            Self::PolicyUnavailable => "policy_unavailable",
            Self::EvaluationTimedOut => "policy_evaluation_timeout",
            Self::EvaluatorUnavailable => "policy_evaluator_unavailable",
            Self::ContextInvalid => "policy_context_invalid",
            Self::InternalError => "policy_internal_error",
        }
    }
}

/// Health supplied by the evaluator supervisor. A non-ready state is checked
/// before any policy expression can run or document can be returned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PolicyEvaluatorState {
    #[default]
    Ready,
    TimedOut,
    Unavailable,
    InternalError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyDecision {
    outcome: PolicyOutcome,
    code: PolicyDecisionCode,
    matched_rule_ids: Vec<PolicyRuleId>,
    evaluated_rules: usize,
}

impl PolicyDecision {
    #[must_use]
    pub const fn outcome(&self) -> PolicyOutcome {
        self.outcome
    }

    #[must_use]
    pub const fn code(&self) -> PolicyDecisionCode {
        self.code
    }

    #[must_use]
    pub fn stable_code(&self) -> &'static str {
        self.code.stable_code()
    }

    #[must_use]
    pub fn matched_rule_ids(&self) -> &[PolicyRuleId] {
        &self.matched_rule_ids
    }

    #[must_use]
    pub const fn evaluated_rules(&self) -> usize {
        self.evaluated_rules
    }

    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self.outcome, PolicyOutcome::Allow)
    }

    fn deny(code: PolicyDecisionCode, evaluated_rules: usize) -> Self {
        Self {
            outcome: PolicyOutcome::Deny,
            code,
            matched_rule_ids: Vec::new(),
            evaluated_rules,
        }
    }

    #[must_use]
    pub fn invalid_context() -> Self {
        Self::deny(PolicyDecisionCode::ContextInvalid, 0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PolicyEvaluator;

impl PolicyEvaluator {
    #[must_use]
    pub fn evaluate(
        &self,
        policy: &CompiledPolicySet,
        context: &PolicyEvaluationContext,
    ) -> PolicyDecision {
        if policy.scope() != context.scope() {
            return PolicyDecision::deny(PolicyDecisionCode::ScopeMismatch, 0);
        }
        let mut evaluated_rules = 0;
        let mut matched_allows = Vec::new();
        let mut matched_denies = Vec::new();
        for rule in policy
            .rules()
            .iter()
            .filter(|rule| rule.applies_to(context.operation()))
        {
            evaluated_rules += 1;
            let mut budget = EvaluationBudget::new(policy.maximum_evaluation_nodes());
            let matched = match evaluate_expression(rule.expression(), context, &mut budget) {
                Ok(Value::Bool(matched)) => matched,
                Ok(_) | Err(PolicyEvaluationError::TypeMismatch) => {
                    return PolicyDecision::deny(
                        PolicyDecisionCode::EvaluationFailed,
                        evaluated_rules,
                    );
                }
                Err(PolicyEvaluationError::CostLimitExceeded) => {
                    return PolicyDecision::deny(
                        PolicyDecisionCode::CostLimitExceeded,
                        evaluated_rules,
                    );
                }
            };
            if matched {
                match rule.effect() {
                    PolicyEffect::Allow => matched_allows.push(rule.id().clone()),
                    PolicyEffect::Deny => matched_denies.push(rule.id().clone()),
                }
            }
        }
        if !matched_denies.is_empty() {
            PolicyDecision {
                outcome: PolicyOutcome::Deny,
                code: PolicyDecisionCode::ExplicitDeny,
                matched_rule_ids: matched_denies,
                evaluated_rules,
            }
        } else if matched_allows.is_empty() {
            PolicyDecision::deny(PolicyDecisionCode::NoMatchingAllow, evaluated_rules)
        } else {
            PolicyDecision {
                outcome: PolicyOutcome::Allow,
                code: PolicyDecisionCode::Allowed,
                matched_rule_ids: matched_allows,
                evaluated_rules,
            }
        }
    }

    #[must_use]
    pub fn evaluate_available(
        &self,
        policy: Option<&CompiledPolicySet>,
        context: &PolicyEvaluationContext,
    ) -> PolicyDecision {
        self.evaluate_guarded(policy, context, PolicyEvaluatorState::Ready)
    }

    #[must_use]
    pub fn evaluate_guarded(
        &self,
        policy: Option<&CompiledPolicySet>,
        context: &PolicyEvaluationContext,
        state: PolicyEvaluatorState,
    ) -> PolicyDecision {
        let failure = match state {
            PolicyEvaluatorState::Ready => None,
            PolicyEvaluatorState::TimedOut => Some(PolicyDecisionCode::EvaluationTimedOut),
            PolicyEvaluatorState::Unavailable => Some(PolicyDecisionCode::EvaluatorUnavailable),
            PolicyEvaluatorState::InternalError => Some(PolicyDecisionCode::InternalError),
        };
        if let Some(code) = failure {
            return PolicyDecision::deny(code, 0);
        }
        policy.map_or_else(
            || PolicyDecision::deny(PolicyDecisionCode::PolicyUnavailable, 0),
            |policy| self.evaluate(policy, context),
        )
    }
}

#[derive(Debug)]
struct EvaluationBudget {
    remaining: usize,
}

impl EvaluationBudget {
    const fn new(limit: usize) -> Self {
        Self { remaining: limit }
    }

    fn consume(&mut self) -> Result<(), PolicyEvaluationError> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(PolicyEvaluationError::CostLimitExceeded)?;
        Ok(())
    }
}

fn evaluate_expression(
    expression: &Expression,
    context: &PolicyEvaluationContext,
    budget: &mut EvaluationBudget,
) -> Result<Value, PolicyEvaluationError> {
    budget.consume()?;
    match expression {
        Expression::Literal(value, _) => Ok(value.clone()),
        Expression::Path(path, _) => Ok(resolve_value(path, context)),
        Expression::Not(expression, _) => evaluate_expression(expression, context, budget)?
            .as_bool()
            .map(|value| Value::Bool(!value))
            .ok_or(PolicyEvaluationError::TypeMismatch),
        Expression::And(left, right, _) => {
            let left = evaluate_expression(left, context, budget)?
                .as_bool()
                .ok_or(PolicyEvaluationError::TypeMismatch)?;
            if !left {
                return Ok(Value::Bool(false));
            }
            evaluate_expression(right, context, budget)?
                .as_bool()
                .map(Value::Bool)
                .ok_or(PolicyEvaluationError::TypeMismatch)
        }
        Expression::Or(left, right, _) => {
            let left = evaluate_expression(left, context, budget)?
                .as_bool()
                .ok_or(PolicyEvaluationError::TypeMismatch)?;
            if left {
                return Ok(Value::Bool(true));
            }
            evaluate_expression(right, context, budget)?
                .as_bool()
                .map(Value::Bool)
                .ok_or(PolicyEvaluationError::TypeMismatch)
        }
        Expression::Compare(comparison, left, right, _) => {
            let left = evaluate_expression(left, context, budget)?;
            let right = evaluate_expression(right, context, budget)?;
            compare_values(*comparison, &left, &right).map(Value::Bool)
        }
    }
}

fn resolve_value(path: &[String], context: &PolicyEvaluationContext) -> Value {
    match path.first().map(String::as_str) {
        Some("identity") => match path.get(1).map(String::as_str) {
            Some("user_id") => context.identity().user_id().map_or(Value::Null, |value| {
                Value::String(value.as_str().to_owned())
            }),
            Some("role") => Value::String(context.identity().role().as_str().to_owned()),
            _ => Value::Null,
        },
        Some("claims") => nested_value(
            &Value::Object(context.identity().trusted_claims().clone()),
            &path[1..],
        ),
        Some("request") => path
            .get(1)
            .and_then(|name| context.request().get(name))
            .map_or(Value::Null, |value| Value::String(value.to_owned())),
        Some("operation") => Value::String(operation_name(context.operation()).to_owned()),
        Some("project_id") => {
            Value::String(context.scope().tenant().project_id().as_str().to_owned())
        }
        Some("environment_id") => Value::String(
            context
                .scope()
                .tenant()
                .environment_id()
                .as_str()
                .to_owned(),
        ),
        Some("collection_id") => Value::String(context.scope().collection_id().as_str().to_owned()),
        Some("old") => context.old_document().map_or(Value::Null, |document| {
            nested_value(&Value::Object(document.clone()), &path[1..])
        }),
        Some("new") => context.new_document().map_or(Value::Null, |document| {
            nested_value(&Value::Object(document.clone()), &path[1..])
        }),
        _ => Value::Null,
    }
}

fn nested_value(root: &Value, path: &[String]) -> Value {
    let mut value = root;
    for segment in path {
        let Some(next) = value.as_object().and_then(|object| object.get(segment)) else {
            return Value::Null;
        };
        value = next;
    }
    value.clone()
}

fn operation_name(operation: DocumentOperation) -> &'static str {
    match operation {
        DocumentOperation::Create => "create",
        DocumentOperation::Read => "read",
        DocumentOperation::Update => "update",
        DocumentOperation::Delete => "delete",
    }
}

fn compare_values(
    comparison: Comparison,
    left: &Value,
    right: &Value,
) -> Result<bool, PolicyEvaluationError> {
    match comparison {
        Comparison::Equal => Ok(left == right),
        Comparison::NotEqual => Ok(left != right),
        Comparison::Less
        | Comparison::LessOrEqual
        | Comparison::Greater
        | Comparison::GreaterOrEqual => {
            let ordering = match (left, right) {
                (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
                (Value::Number(left), Value::Number(right)) => left
                    .as_f64()
                    .zip(right.as_f64())
                    .and_then(|(left, right)| left.partial_cmp(&right)),
                _ => None,
            }
            .ok_or(PolicyEvaluationError::TypeMismatch)?;
            Ok(match comparison {
                Comparison::Less => ordering.is_lt(),
                Comparison::LessOrEqual => ordering.is_le(),
                Comparison::Greater => ordering.is_gt(),
                Comparison::GreaterOrEqual => ordering.is_ge(),
                Comparison::Equal | Comparison::NotEqual => unreachable!(),
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyEvaluationError {
    TypeMismatch,
    CostLimitExceeded,
}

impl fmt::Display for PolicyEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TypeMismatch => "policy value type mismatch",
            Self::CostLimitExceeded => "policy evaluation cost limit exceeded",
        })
    }
}

impl Error for PolicyEvaluationError {}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use serde_json::json;

    use super::*;
    use crate::{
        PolicyCompiler, PolicyRule, PolicySet, PolicyState, PolicyVersion, SafeRequestMetadata,
        SubjectId, VerifiedIdentity, VerifiedRole,
    };

    #[test]
    fn matching_allow_is_required_and_matching_deny_overrides() {
        let policy = compile([
            rule(
                "owner-allow",
                PolicyEffect::Allow,
                "old.owner_id == identity.user_id",
            ),
            rule("blocked-deny", PolicyEffect::Deny, "old.blocked == true"),
        ]);
        let evaluator = PolicyEvaluator;

        let allowed = evaluator.evaluate(&policy, &context("user-1", false));
        assert!(allowed.is_allowed());
        assert_eq!(allowed.stable_code(), "policy_allowed");

        let denied = evaluator.evaluate(&policy, &context("user-1", true));
        assert!(!denied.is_allowed());
        assert_eq!(denied.code(), PolicyDecisionCode::ExplicitDeny);
        assert_eq!(denied.matched_rule_ids()[0].as_str(), "blocked-deny");

        let default_denied = evaluator.evaluate(&policy, &context("user-2", false));
        assert_eq!(default_denied.code(), PolicyDecisionCode::NoMatchingAllow);
    }

    #[test]
    fn absent_policy_and_scope_mismatch_fail_closed() {
        let policy = compile([rule("allow", PolicyEffect::Allow, "true")]);
        let mut context = context("user-1", false);
        context = PolicyEvaluationContext::new(
            CollectionScope::new(
                TenantScope::new(
                    ProjectId::parse("prj_ijklmnop").expect("project"),
                    EnvironmentId::parse("env_abcdefgh").expect("environment"),
                ),
                CollectionId::parse("todos").expect("collection"),
            ),
            DocumentOperation::Read,
            context.identity().clone(),
            Some(json!({"id": "doc-1", "owner_id": "user-1", "blocked": false})),
            None,
            SafeRequestMetadata::empty(),
        )
        .expect("context");
        let evaluator = PolicyEvaluator;
        assert_eq!(
            evaluator.evaluate(&policy, &context).code(),
            PolicyDecisionCode::ScopeMismatch
        );
        assert_eq!(
            evaluator.evaluate_available(None, &context).code(),
            PolicyDecisionCode::PolicyUnavailable
        );
    }

    #[test]
    fn timeout_unavailability_invalid_context_and_internal_failure_deny() {
        let policy = compile([rule("allow", PolicyEffect::Allow, "true")]);
        let context = context("user-1", false);
        let evaluator = PolicyEvaluator;
        for (state, expected) in [
            (
                PolicyEvaluatorState::TimedOut,
                PolicyDecisionCode::EvaluationTimedOut,
            ),
            (
                PolicyEvaluatorState::Unavailable,
                PolicyDecisionCode::EvaluatorUnavailable,
            ),
            (
                PolicyEvaluatorState::InternalError,
                PolicyDecisionCode::InternalError,
            ),
        ] {
            let decision = evaluator.evaluate_guarded(Some(&policy), &context, state);
            assert!(!decision.is_allowed());
            assert_eq!(decision.code(), expected);
        }
        assert_eq!(
            PolicyDecision::invalid_context().code(),
            PolicyDecisionCode::ContextInvalid
        );
    }

    fn compile(rules: impl IntoIterator<Item = PolicyRule>) -> CompiledPolicySet {
        let policy = PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Validated,
            rules,
            [],
        )
        .expect("policy");
        PolicyCompiler::default()
            .compile(
                &policy,
                &json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "owner_id": {"type": "string"},
                        "blocked": {"type": "boolean"}
                    }
                }),
            )
            .expect("compile")
            .into_compiled()
            .expect("compiled")
    }

    fn rule(id: &str, effect: PolicyEffect, expression: &str) -> PolicyRule {
        PolicyRule::new(
            PolicyRuleId::parse(id).expect("id"),
            effect,
            [DocumentOperation::Read],
            expression,
        )
        .expect("rule")
    }

    fn context(user_id: &str, blocked: bool) -> PolicyEvaluationContext {
        PolicyEvaluationContext::new(
            scope(),
            DocumentOperation::Read,
            VerifiedIdentity::user(
                SubjectId::parse(user_id).expect("subject"),
                VerifiedRole::parse("member").expect("role"),
                json!({}),
            )
            .expect("identity"),
            Some(json!({"id": "doc-1", "owner_id": "user-1", "blocked": blocked})),
            None,
            SafeRequestMetadata::empty(),
        )
        .expect("context")
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
