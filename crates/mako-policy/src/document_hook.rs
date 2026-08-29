use mako_documents::{
    CanonicalDocument, DocumentMutationAuthorizer, DocumentReadAuthorizer,
    MutationAuthorizationContext, MutationAuthorizationDecision, MutationAuthorizationOperation,
    ReadAuthorizationContext, ReadAuthorizationPath,
};
use serde_json::Value;

use crate::{
    CompiledPolicySet, DocumentOperation, PolicyEvaluationContext, PolicyEvaluator,
    PolicyEvaluatorState, SafeRequestMetadata, VerifiedIdentity,
};

pub struct DocumentPolicyAuthorizer<'a> {
    policy: Option<&'a CompiledPolicySet>,
    identity: &'a VerifiedIdentity,
    request: &'a SafeRequestMetadata,
    evaluator: PolicyEvaluator,
    evaluator_state: PolicyEvaluatorState,
}

pub struct DocumentPolicyReadAuthorizer<'a> {
    policy: Option<&'a CompiledPolicySet>,
    identity: &'a VerifiedIdentity,
    request: &'a SafeRequestMetadata,
    evaluator: PolicyEvaluator,
    evaluator_state: PolicyEvaluatorState,
}

impl<'a> DocumentPolicyReadAuthorizer<'a> {
    #[must_use]
    pub fn new(
        policy: Option<&'a CompiledPolicySet>,
        identity: &'a VerifiedIdentity,
        request: &'a SafeRequestMetadata,
    ) -> Self {
        Self {
            policy,
            identity,
            request,
            evaluator: PolicyEvaluator,
            evaluator_state: PolicyEvaluatorState::Ready,
        }
    }

    #[must_use]
    pub fn with_evaluator_state(mut self, evaluator_state: PolicyEvaluatorState) -> Self {
        self.evaluator_state = evaluator_state;
        self
    }

    #[must_use]
    pub fn filter_documents(
        &self,
        scope: &mako_api::CollectionScope,
        path: ReadAuthorizationPath,
        documents: impl IntoIterator<Item = CanonicalDocument>,
    ) -> Vec<CanonicalDocument> {
        documents
            .into_iter()
            .filter(|document| self.authorize_read_for(scope, path, document).is_allowed())
            .collect()
    }

    #[must_use]
    pub fn filter_readable_conflict(
        &self,
        scope: &mako_api::CollectionScope,
        document: CanonicalDocument,
    ) -> Option<CanonicalDocument> {
        self.authorize_read_for(scope, ReadAuthorizationPath::ConflictResponse, &document)
            .is_allowed()
            .then_some(document)
    }

    #[must_use]
    pub fn decision_for_document(
        &self,
        scope: &mako_api::CollectionScope,
        path: ReadAuthorizationPath,
        document: &CanonicalDocument,
    ) -> MutationAuthorizationDecision {
        self.authorize_read_for(scope, path, document)
    }

    fn authorize_read_for(
        &self,
        scope: &mako_api::CollectionScope,
        _path: ReadAuthorizationPath,
        document: &CanonicalDocument,
    ) -> MutationAuthorizationDecision {
        let context = match PolicyEvaluationContext::new(
            scope.clone(),
            DocumentOperation::Read,
            self.identity.clone(),
            Some(Value::Object(document.body().clone())),
            None,
            self.request.clone(),
        ) {
            Ok(context) => context,
            Err(_) => {
                let decision = crate::PolicyDecision::invalid_context();
                return MutationAuthorizationDecision::deny(decision.stable_code());
            }
        };
        let decision = self
            .evaluator
            .evaluate_guarded(self.policy, &context, self.evaluator_state);
        if decision.is_allowed() {
            MutationAuthorizationDecision::allow(decision.stable_code())
        } else {
            MutationAuthorizationDecision::deny(decision.stable_code())
        }
    }
}

impl DocumentReadAuthorizer for DocumentPolicyReadAuthorizer<'_> {
    fn authorize_read(
        &self,
        context: ReadAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision {
        self.authorize_read_for(context.scope(), context.path(), context.document())
    }
}

impl<'a> DocumentPolicyAuthorizer<'a> {
    #[must_use]
    pub fn new(
        policy: Option<&'a CompiledPolicySet>,
        identity: &'a VerifiedIdentity,
        request: &'a SafeRequestMetadata,
    ) -> Self {
        Self {
            policy,
            identity,
            request,
            evaluator: PolicyEvaluator,
            evaluator_state: PolicyEvaluatorState::Ready,
        }
    }

    #[must_use]
    pub fn with_evaluator_state(mut self, evaluator_state: PolicyEvaluatorState) -> Self {
        self.evaluator_state = evaluator_state;
        self
    }
}

impl DocumentMutationAuthorizer for DocumentPolicyAuthorizer<'_> {
    fn authorize(
        &self,
        context: MutationAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision {
        let (operation, old_document, new_document) = match context.operation() {
            MutationAuthorizationOperation::Create => (
                DocumentOperation::Create,
                None,
                Some(Value::Object(context.new_document().body().clone())),
            ),
            MutationAuthorizationOperation::Update => (
                DocumentOperation::Update,
                context
                    .old_document()
                    .map(|document| Value::Object(document.body().clone())),
                Some(Value::Object(context.new_document().body().clone())),
            ),
            MutationAuthorizationOperation::Delete => (
                DocumentOperation::Delete,
                context
                    .old_document()
                    .map(|document| Value::Object(document.body().clone())),
                None,
            ),
        };
        let evaluation_context = match PolicyEvaluationContext::new(
            context.scope().clone(),
            operation,
            self.identity.clone(),
            old_document,
            new_document,
            self.request.clone(),
        ) {
            Ok(context) => context,
            Err(_) => {
                let decision = crate::PolicyDecision::invalid_context();
                return MutationAuthorizationDecision::deny(decision.stable_code());
            }
        };
        let decision =
            self.evaluator
                .evaluate_guarded(self.policy, &evaluation_context, self.evaluator_state);
        if decision.is_allowed() {
            MutationAuthorizationDecision::allow(decision.stable_code())
        } else {
            MutationAuthorizationDecision::deny(decision.stable_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_documents::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, MutationCommitOutcome, MutationError, MutationId,
        MutationInput, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion, TrustedQuery,
    };
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        PolicyCompiler, PolicyEffect, PolicyRule, PolicyRuleId, PolicySet, PolicyState,
        PolicyVersion, SubjectId, VerifiedRole,
    };

    #[test]
    fn write_hook_uses_proposed_create_and_both_update_states_without_partial_writes() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter.clone());
            let scope = scope();
            let scoped = engine
                .scope_collection(scope.tenant(), scope.clone())
                .expect("scope");
            let sequencer = engine
                .scope_sequencer(scope.tenant(), scope.tenant(), Durability::Memory)
                .expect("sequencer");
            let metadata = metadata();
            let validator = DocumentValidator::compile(&metadata).expect("validator");
            let policy = compiled_policy();
            let identity = VerifiedIdentity::user(
                SubjectId::parse("user-1").expect("subject"),
                VerifiedRole::parse("member").expect("role"),
                json!({}),
            )
            .expect("identity");
            let request = SafeRequestMetadata::empty();
            let authorizer = DocumentPolicyAuthorizer::new(Some(&policy), &identity, &request);
            let mut lease = sequencer
                .lease(NonZeroU64::new(3).expect("non-zero"))
                .await
                .expect("lease");

            let denied = input(
                &validator,
                lease.issue().expect("position"),
                "denied-create",
                "user-2",
            );
            let before = adapter.dump().expect("before");
            assert!(matches!(
                scoped.create_document_authorized(denied, &authorizer).await,
                Err(MutationError::AuthorizationDenied { .. })
            ));
            assert_eq!(adapter.dump().expect("after denied create"), before);

            let allowed = input(
                &validator,
                lease.issue().expect("position"),
                "allowed-create",
                "user-1",
            );
            let MutationCommitOutcome::Applied(created) = scoped
                .create_document_authorized(allowed, &authorizer)
                .await
                .expect("allowed create")
            else {
                panic!("create must apply");
            };
            let stored = scoped
                .get_document(&mako_documents::DocumentId::parse("doc-1").expect("document id"))
                .await
                .expect("read")
                .expect("document");
            let read_authorizer =
                DocumentPolicyReadAuthorizer::new(Some(&policy), &identity, &request);
            for path in [
                ReadAuthorizationPath::Point,
                ReadAuthorizationPath::ReplicationPull,
                ReadAuthorizationPath::TrustedQuery,
                ReadAuthorizationPath::LiveStream,
                ReadAuthorizationPath::ConflictResponse,
                ReadAuthorizationPath::EdgeSdk,
                ReadAuthorizationPath::OperatorImpersonation,
            ] {
                assert_eq!(
                    read_authorizer
                        .filter_documents(&scope, path, [stored.clone()])
                        .len(),
                    1
                );
            }
            let trusted_page = scoped
                .trusted_query_authorized(
                    &TrustedQuery::by_primary_key(
                        mako_documents::DocumentId::parse("doc-1").expect("document id"),
                    ),
                    &read_authorizer,
                )
                .await
                .expect("trusted query");
            assert_eq!(trusted_page.documents().len(), 1);

            let other_identity = VerifiedIdentity::user(
                SubjectId::parse("user-2").expect("subject"),
                VerifiedRole::parse("member").expect("role"),
                json!({}),
            )
            .expect("identity");
            let other_authorizer =
                DocumentPolicyReadAuthorizer::new(Some(&policy), &other_identity, &request);
            for path in [
                ReadAuthorizationPath::Point,
                ReadAuthorizationPath::ReplicationPull,
                ReadAuthorizationPath::TrustedQuery,
                ReadAuthorizationPath::LiveStream,
                ReadAuthorizationPath::ConflictResponse,
                ReadAuthorizationPath::EdgeSdk,
                ReadAuthorizationPath::OperatorImpersonation,
            ] {
                assert_eq!(
                    read_authorizer
                        .decision_for_document(&scope, path, &stored)
                        .stable_code(),
                    "policy_allowed"
                );
                assert_eq!(
                    other_authorizer
                        .decision_for_document(&scope, path, &stored)
                        .stable_code(),
                    "policy_default_deny"
                );
            }
            let denied_page = scoped
                .trusted_query_authorized(
                    &TrustedQuery::by_primary_key(
                        mako_documents::DocumentId::parse("doc-1").expect("document id"),
                    ),
                    &other_authorizer,
                )
                .await
                .expect("trusted query");
            assert!(denied_page.documents().is_empty());
            assert!(
                read_authorizer
                    .filter_readable_conflict(&scope, stored.clone())
                    .is_some()
            );
            assert!(
                other_authorizer
                    .filter_readable_conflict(&scope, stored)
                    .is_none()
            );
            let before_transfer = adapter.dump().expect("before transfer");
            let transfer = input(
                &validator,
                lease.issue().expect("position"),
                "denied-transfer",
                "user-2",
            );
            assert!(matches!(
                scoped
                    .update_document_authorized(created.revision, transfer, &authorizer)
                    .await,
                Err(MutationError::AuthorizationDenied { .. })
            ));
            assert_eq!(adapter.dump().expect("after transfer"), before_transfer);
        });
    }

    #[test]
    fn hooks_index_the_verified_identity_trusted_claims_by_the_document_household() {
        block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let engine = DocumentEngine::new(adapter.clone());
            let scope = scope();
            let scoped = engine
                .scope_collection(scope.tenant(), scope.clone())
                .expect("scope");
            let sequencer = engine
                .scope_sequencer(scope.tenant(), scope.tenant(), Durability::Memory)
                .expect("sequencer");
            let metadata = household_metadata();
            let validator = DocumentValidator::compile(&metadata).expect("validator");
            let policy = household_policy(&metadata);
            let request = SafeRequestMetadata::empty();
            let editor = member("user-1", json!({"households": {"hh_1": "editor"}}));
            let viewer = member("user-2", json!({"households": {"hh_1": "viewer"}}));
            let stranger = member("user-3", json!({}));
            let mut lease = sequencer
                .lease(NonZeroU64::new(8).expect("non-zero"))
                .await
                .expect("lease");

            let editor_authorizer = DocumentPolicyAuthorizer::new(Some(&policy), &editor, &request);
            let before = adapter.dump().expect("before");
            assert!(matches!(
                scoped
                    .create_document_authorized(
                        household_input(
                            &validator,
                            lease.issue().expect("position"),
                            "editor-other-household",
                            "doc-2",
                            "hh_2",
                        ),
                        &editor_authorizer,
                    )
                    .await,
                Err(MutationError::AuthorizationDenied { .. })
            ));
            for (name, identity) in [("viewer", &viewer), ("stranger", &stranger)] {
                let authorizer = DocumentPolicyAuthorizer::new(Some(&policy), identity, &request);
                assert!(
                    matches!(
                        scoped
                            .create_document_authorized(
                                household_input(
                                    &validator,
                                    lease.issue().expect("position"),
                                    &format!("{name}-create"),
                                    "doc-1",
                                    "hh_1",
                                ),
                                &authorizer,
                            )
                            .await,
                        Err(MutationError::AuthorizationDenied { .. })
                    ),
                    "{name}"
                );
            }
            assert_eq!(adapter.dump().expect("after denied creates"), before);

            let MutationCommitOutcome::Applied(_) = scoped
                .create_document_authorized(
                    household_input(
                        &validator,
                        lease.issue().expect("position"),
                        "editor-create",
                        "doc-1",
                        "hh_1",
                    ),
                    &editor_authorizer,
                )
                .await
                .expect("editor create")
            else {
                panic!("create must apply");
            };
            let stored = scoped
                .get_document(&mako_documents::DocumentId::parse("doc-1").expect("document id"))
                .await
                .expect("read")
                .expect("document");
            for (name, identity, expected) in [
                ("editor", &editor, "policy_allowed"),
                ("viewer", &viewer, "policy_allowed"),
                ("stranger", &stranger, "policy_default_deny"),
            ] {
                let read_authorizer =
                    DocumentPolicyReadAuthorizer::new(Some(&policy), identity, &request);
                assert_eq!(
                    read_authorizer
                        .decision_for_document(&scope, ReadAuthorizationPath::Point, &stored)
                        .stable_code(),
                    expected,
                    "{name}"
                );
            }
        });
    }

    fn household_policy(metadata: &CollectionMetadata) -> CompiledPolicySet {
        let rules = [
            PolicyRule::new(
                PolicyRuleId::parse("member-write").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Create],
                "(claims.households[new.household_id] == \"owner\" || claims.households[new.household_id] == \"editor\")",
            )
            .expect("rule"),
            PolicyRule::new(
                PolicyRuleId::parse("member-read").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Read],
                "claims.households[old.household_id] != null",
            )
            .expect("rule"),
        ];
        let policy = PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Validated,
            rules,
            [],
        )
        .expect("policy");
        PolicyCompiler::default()
            .compile(&policy, &Value::Object(metadata.json_schema().clone()))
            .expect("compile")
            .into_compiled()
            .expect("compiled")
    }

    fn member(user_id: &str, trusted_claims: Value) -> VerifiedIdentity {
        VerifiedIdentity::user(
            SubjectId::parse(user_id).expect("subject"),
            VerifiedRole::parse("member").expect("role"),
            trusted_claims,
        )
        .expect("identity")
    }

    fn household_input(
        validator: &DocumentValidator,
        position: u64,
        mutation: &str,
        id: &str,
        household_id: &str,
    ) -> MutationInput {
        MutationInput {
            mutation_id: MutationId::parse(mutation).expect("mutation"),
            commit_position: CommitPosition::new(position).expect("position"),
            document: validator
                .validate_create(json!({"id": id, "household_id": household_id}))
                .expect("document"),
            durability: Durability::Memory,
        }
    }

    fn household_metadata() -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "household_id": {"type": "string"}
                },
                "required": ["id", "household_id"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
    }

    fn compiled_policy() -> CompiledPolicySet {
        let rules = [
            PolicyRule::new(
                PolicyRuleId::parse("owner-create").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Create],
                "new.owner_id == identity.user_id",
            )
            .expect("rule"),
            PolicyRule::new(
                PolicyRuleId::parse("owner-update").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Update],
                "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
            )
            .expect("rule"),
            PolicyRule::new(
                PolicyRuleId::parse("owner-read").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Read],
                "old.owner_id == identity.user_id",
            )
            .expect("rule"),
        ];
        let policy = PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Validated,
            rules,
            [],
        )
        .expect("policy");
        PolicyCompiler::default()
            .compile(&policy, &Value::Object(metadata().json_schema().clone()))
            .expect("compile")
            .into_compiled()
            .expect("compiled")
    }

    fn input(
        validator: &DocumentValidator,
        position: u64,
        mutation: &str,
        owner_id: &str,
    ) -> MutationInput {
        MutationInput {
            mutation_id: MutationId::parse(mutation).expect("mutation"),
            commit_position: CommitPosition::new(position).expect("position"),
            document: validator
                .validate_create(json!({"id": "doc-1", "owner_id": owner_id}))
                .expect("document"),
            durability: Durability::Memory,
        }
    }

    fn metadata() -> CollectionMetadata {
        CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "owner_id": {"type": "string"}
                },
                "required": ["id", "owner_id"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata")
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
