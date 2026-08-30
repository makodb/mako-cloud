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
        Expression::Index(base, index, _) => {
            let base = evaluate_expression(base, context, budget)?;
            let index = evaluate_expression(index, context, budget)?;
            Ok(index_value(base, &index))
        }
    }
}

/// Looks `index` up in `base`. An object indexed by a string yields the member
/// and an array indexed by a non-negative integer yields the element; every
/// other combination, including an absent member, is `null` so that a missing
/// membership fails an equality instead of failing the evaluation.
fn index_value(base: Value, index: &Value) -> Value {
    match (base, index) {
        (Value::Object(mut members), Value::String(key)) => {
            members.remove(key).unwrap_or(Value::Null)
        }
        (Value::Array(mut items), Value::Number(position)) => position
            .as_u64()
            .and_then(|position| usize::try_from(position).ok())
            .filter(|position| *position < items.len())
            .map_or(Value::Null, |position| items.swap_remove(position)),
        _ => Value::Null,
    }
}

fn resolve_value(path: &[String], context: &PolicyEvaluationContext) -> Value {
    match path.first().map(String::as_str) {
        Some("identity") => match path.get(1).map(String::as_str) {
            Some("user_id") => context.identity().user_id().map_or(Value::Null, |value| {
                Value::String(value.as_str().to_owned())
            }),
            Some("role") => Value::String(context.identity().role().as_str().to_owned()),
            // A caller with no address reads as null, which no comparison
            // matches -- an anonymous caller is never handed a document
            // addressed to somebody.
            Some("email") => context.identity().email().map_or(Value::Null, |email| {
                Value::String(email.address().to_owned())
            }),
            Some("email_verified") => Value::Bool(
                context
                    .identity()
                    .email()
                    .is_some_and(crate::VerifiedEmail::confirmed),
            ),
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
    use crate::VerifiedEmail;
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

    #[test]
    fn indexed_claims_select_the_membership_named_by_the_document() {
        let policy = household_policy([
            household_rule(
                "member-create",
                [DocumentOperation::Create],
                "claims.households[new.household_id] == \"editor\"",
            ),
            household_rule(
                "member-read",
                [DocumentOperation::Read, DocumentOperation::Delete],
                "claims.households[old.household_id] != null",
            ),
        ]);
        let evaluator = PolicyEvaluator;
        let editor = json!({"households": {"hh_1": "editor"}});

        let create = |household: &str, claims: &Value| {
            evaluator.evaluate(
                &policy,
                &household_context(
                    DocumentOperation::Create,
                    claims.clone(),
                    None,
                    Some(json!({"id": "doc-1", "household_id": household})),
                ),
            )
        };
        assert!(create("hh_1", &editor).is_allowed());
        for (household, claims) in [
            ("hh_2", editor.clone()),
            ("hh_1", json!({})),
            ("hh_1", json!({"households": {"hh_1": "viewer"}})),
            ("hh_1", json!({"households": "hh_1"})),
            ("hh_1", json!({"households": ["hh_1"]})),
            ("hh_1", json!({"households": null})),
        ] {
            let decision = create(household, &claims);
            assert_eq!(
                decision.code(),
                PolicyDecisionCode::NoMatchingAllow,
                "{household} with {claims}"
            );
        }

        for operation in [DocumentOperation::Read, DocumentOperation::Delete] {
            let read = |household: &str, claims: Value| {
                evaluator.evaluate(
                    &policy,
                    &household_context(
                        operation,
                        claims,
                        Some(json!({"id": "doc-1", "household_id": household})),
                        None,
                    ),
                )
            };
            assert!(read("hh_1", editor.clone()).is_allowed(), "{operation:?}");
            assert!(
                read("hh_1", json!({"households": {"hh_1": "viewer"}})).is_allowed(),
                "{operation:?}"
            );
            assert_eq!(
                read("hh_2", editor.clone()).code(),
                PolicyDecisionCode::NoMatchingAllow,
                "{operation:?}"
            );
            assert_eq!(
                read("hh_1", json!({})).code(),
                PolicyDecisionCode::NoMatchingAllow,
                "{operation:?}"
            );
        }
    }

    #[test]
    fn indexing_arrays_by_position_and_anything_else_yields_null_without_failing() {
        let claims = json!({
            "team_ids": ["blue", "green"],
            "label": "solo",
            "flag": true,
            "households": {"hh_1": "owner"}
        });
        let document = json!({"id": "doc-1", "household_id": "hh_1", "slot": 1});
        let cases = [
            ("claims.team_ids[0] == \"blue\"", true),
            ("claims.team_ids[1] == \"green\"", true),
            ("claims.team_ids[new.slot] == \"green\"", true),
            ("claims.team_ids[0] != null", true),
            ("claims.team_ids[2] == null", true),
            ("claims.team_ids[-1] == null", true),
            ("claims.team_ids[1.5] == null", true),
            ("claims.team_ids[\"0\"] == null", true),
            ("claims.households[0] == null", true),
            ("claims.households[new.slot] == null", true),
            ("claims.label[0] == null", true),
            ("claims.label[\"x\"] == null", true),
            ("claims.flag[\"x\"] == null", true),
            ("claims.missing[\"x\"] == null", true),
            ("claims.missing[new.household_id] != null", false),
            (
                "claims.households[new.household_id][new.household_id] == null",
                true,
            ),
            ("claims.team_ids[5] != null", false),
            ("claims.team_ids[new.slot] < \"h\"", true),
        ];
        let evaluator = PolicyEvaluator;
        for (expression, expected) in cases {
            let policy = household_policy([household_rule(
                "rule",
                [DocumentOperation::Create],
                expression,
            )]);
            let context = household_context(
                DocumentOperation::Create,
                claims.clone(),
                None,
                Some(document.clone()),
            );
            let decision = evaluator.evaluate(&policy, &context);
            assert_ne!(
                decision.code(),
                PolicyDecisionCode::EvaluationFailed,
                "{expression}"
            );
            assert_eq!(decision.is_allowed(), expected, "{expression}");
        }
    }

    #[test]
    fn index_nodes_consume_the_evaluation_budget_and_evaluate_their_index_once() {
        let policy = household_policy([household_rule(
            "rule",
            [DocumentOperation::Create],
            "claims.households[new.household_id] == \"editor\"",
        )]);
        let context = household_context(
            DocumentOperation::Create,
            json!({"households": {"hh_1": "editor"}}),
            None,
            Some(json!({"id": "doc-1", "household_id": "hh_1"})),
        );
        let expression = policy.rules()[0].expression();
        // Compare, Index, the claims path, the document path, the literal.
        let mut short = EvaluationBudget::new(4);
        assert_eq!(
            evaluate_expression(expression, &context, &mut short),
            Err(PolicyEvaluationError::CostLimitExceeded)
        );
        let mut exact = EvaluationBudget::new(5);
        assert_eq!(
            evaluate_expression(expression, &context, &mut exact),
            Ok(Value::Bool(true))
        );
        assert_eq!(exact.remaining, 0);
    }

    #[test]
    fn indexing_by_a_literal_key_matches_the_dotted_path_for_identifier_keys() {
        let head = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_";
        let tail = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789";
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut keys: Vec<String> = [
            "_",
            "a",
            "Z",
            "x_",
            "__init__",
            "snake_case",
            "CamelCase",
            "a1b2",
            "households",
            "hh_1",
            "nullable",
            "true_",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        while keys.len() < 200 {
            let length = usize::try_from(next() % 12).expect("length");
            let mut key = String::new();
            key.push(char::from(
                head[usize::try_from(next() % head.len() as u64).expect("index")],
            ));
            for _ in 0..length {
                key.push(char::from(
                    tail[usize::try_from(next() % tail.len() as u64).expect("index")],
                ));
            }
            if !matches!(key.as_str(), "true" | "false" | "null") {
                keys.push(key);
            }
        }
        let values = [
            json!("editor"),
            json!(7),
            json!(-2.5),
            json!(true),
            json!(null),
            json!({"nested": 1}),
            json!(["x", "y"]),
        ];
        let document = json!({"id": "doc-1", "household_id": "hh_1"});
        for (position, key) in keys.iter().enumerate() {
            let value = &values[position % values.len()];
            let policy = household_policy([
                household_rule(
                    "dotted",
                    [DocumentOperation::Create],
                    &format!("claims.a.{key}"),
                ),
                household_rule(
                    "indexed",
                    [DocumentOperation::Create],
                    &format!("claims.a[\"{key}\"]"),
                ),
            ]);
            let mut members = serde_json::Map::new();
            members.insert(key.clone(), value.clone());
            members.insert("decoy".to_owned(), json!("decoy"));
            let present = household_context(
                DocumentOperation::Create,
                json!({"a": members}),
                None,
                Some(document.clone()),
            );
            let absent = household_context(
                DocumentOperation::Create,
                json!({"a": {"decoy": "decoy"}}),
                None,
                Some(document.clone()),
            );
            assert_eq!(policy.rules().len(), 2);
            for rule in policy.rules() {
                let mut budget = EvaluationBudget::new(8);
                assert_eq!(
                    evaluate_expression(rule.expression(), &present, &mut budget),
                    Ok(value.clone()),
                    "{key} via {}",
                    rule.id().as_str()
                );
                let mut budget = EvaluationBudget::new(8);
                assert_eq!(
                    evaluate_expression(rule.expression(), &absent, &mut budget),
                    Ok(Value::Null),
                    "{key} via {}",
                    rule.id().as_str()
                );
            }
        }
    }

    fn household_policy(rules: impl IntoIterator<Item = PolicyRule>) -> CompiledPolicySet {
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
                        "household_id": {"type": "string"},
                        "slot": {"type": "integer"}
                    }
                }),
            )
            .expect("compile")
            .into_compiled()
            .expect("compiled")
    }

    fn household_rule(
        id: &str,
        operations: impl IntoIterator<Item = DocumentOperation>,
        expression: &str,
    ) -> PolicyRule {
        PolicyRule::new(
            PolicyRuleId::parse(id).expect("id"),
            PolicyEffect::Allow,
            operations,
            expression,
        )
        .expect("rule")
    }

    /// An invitation names an address; only the person the platform says
    /// controls it may read the row. Both halves are load-bearing: the
    /// address alone is what somebody typed at sign-up, so a rule that omits
    /// the confirmation hands the invitation to whoever registered the
    /// address first -- which is the disclosure this input exists to close.
    #[test]
    fn an_invitation_is_read_only_by_the_confirmed_holder_of_its_address() {
        let policy = compile_with_schema(
            [rule(
                "invitee-reads",
                PolicyEffect::Allow,
                "old.invitee_email == identity.email && identity.email_verified",
            )],
            &json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "invitee_email": {"type": "string"}
                }
            }),
        );
        let evaluator = PolicyEvaluator;
        let document = json!({"id": "inv-1", "invitee_email": "invitee@example.test"});
        let decide = |email: Option<VerifiedEmail>| {
            evaluator.evaluate(
                &policy,
                &PolicyEvaluationContext::new(
                    scope(),
                    DocumentOperation::Read,
                    VerifiedIdentity::user(
                        SubjectId::parse("user-1").expect("subject"),
                        VerifiedRole::parse("member").expect("role"),
                        email,
                        json!({}),
                    )
                    .expect("identity"),
                    Some(document.clone()),
                    None,
                    SafeRequestMetadata::empty(),
                )
                .expect("context"),
            )
        };

        // The address the platform confirmed: allowed, and case does not
        // decide who reads somebody's invitation.
        let confirmed = VerifiedEmail::parse("Invitee@Example.test", true).expect("email");
        assert_eq!(decide(Some(confirmed)).outcome(), PolicyOutcome::Allow);
        // The same address, unconfirmed: registering it is not proof of
        // holding it.
        let claimed = VerifiedEmail::parse("invitee@example.test", false).expect("email");
        assert_eq!(decide(Some(claimed)).outcome(), PolicyOutcome::Deny);
        // Somebody else, and a caller with no address at all.
        let other = VerifiedEmail::parse("stranger@example.test", true).expect("email");
        assert_eq!(decide(Some(other)).outcome(), PolicyOutcome::Deny);
        assert_eq!(decide(None).outcome(), PolicyOutcome::Deny);
    }

    fn household_context(
        operation: DocumentOperation,
        claims: Value,
        old_document: Option<Value>,
        new_document: Option<Value>,
    ) -> PolicyEvaluationContext {
        PolicyEvaluationContext::new(
            scope(),
            operation,
            VerifiedIdentity::user(
                SubjectId::parse("user-1").expect("subject"),
                VerifiedRole::parse("member").expect("role"),
                None,
                claims,
            )
            .expect("identity"),
            old_document,
            new_document,
            SafeRequestMetadata::empty(),
        )
        .expect("context")
    }

    fn compile(rules: impl IntoIterator<Item = PolicyRule>) -> CompiledPolicySet {
        compile_with_schema(
            rules,
            &json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "owner_id": {"type": "string"},
                    "blocked": {"type": "boolean"}
                }
            }),
        )
    }

    fn compile_with_schema(
        rules: impl IntoIterator<Item = PolicyRule>,
        schema: &Value,
    ) -> CompiledPolicySet {
        let policy = PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Validated,
            rules,
            [],
        )
        .expect("policy");
        PolicyCompiler::default()
            .compile(&policy, schema)
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
                None,
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
