use mako_documents::{
    CanonicalDocument, CommitPosition, DocumentId, ReadAuthorizationPath, RevisionToken,
};

use crate::DocumentPolicyReadAuthorizer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntheticTombstone {
    document_id: DocumentId,
    revision: RevisionToken,
    commit_position: CommitPosition,
}

impl SyntheticTombstone {
    #[must_use]
    pub fn document_id(&self) -> &DocumentId {
        &self.document_id
    }

    #[must_use]
    pub fn revision(&self) -> &RevisionToken {
        &self.revision
    }

    #[must_use]
    pub const fn commit_position(&self) -> CommitPosition {
        self.commit_position
    }

    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VisibilityTransition {
    Visible(CanonicalDocument),
    SyntheticTombstone(SyntheticTombstone),
    Hidden,
}

impl DocumentPolicyReadAuthorizer<'_> {
    #[must_use]
    pub fn classify_visibility(
        &self,
        scope: &mako_api::CollectionScope,
        old_document: Option<&CanonicalDocument>,
        new_document: &CanonicalDocument,
    ) -> VisibilityTransition {
        let old_visible = old_document.is_some_and(|document| {
            self.decision_for_document(scope, ReadAuthorizationPath::ReplicationPull, document)
                .is_allowed()
        });
        let new_visible = !new_document.is_deleted()
            && self
                .decision_for_document(scope, ReadAuthorizationPath::ReplicationPull, new_document)
                .is_allowed();
        if new_visible {
            VisibilityTransition::Visible(new_document.clone())
        } else if old_visible {
            VisibilityTransition::SyntheticTombstone(SyntheticTombstone {
                document_id: new_document.primary_key().clone(),
                revision: new_document.revision().clone(),
                commit_position: new_document.commit_position(),
            })
        } else {
            VisibilityTransition::Hidden
        }
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_documents::{CommitPosition, DocumentId, RevisionToken, SchemaVersion};
    use serde_json::json;

    use super::*;
    use crate::{
        CompiledPolicySet, DocumentOperation, PolicyCompiler, PolicyEffect, PolicyRule,
        PolicyRuleId, PolicySet, PolicyState, PolicyVersion, SafeRequestMetadata, SubjectId,
        VerifiedIdentity, VerifiedRole,
    };

    #[test]
    fn classifies_all_visibility_transitions_without_protected_new_content() {
        let policy = policy();
        let identity = VerifiedIdentity::user(
            SubjectId::parse("user-1").expect("subject"),
            VerifiedRole::parse("member").expect("role"),
            json!({"team": "blue"}),
        )
        .expect("identity");
        let request = SafeRequestMetadata::empty();
        let authorizer = DocumentPolicyReadAuthorizer::new(Some(&policy), &identity, &request);
        let blue_old = document("blue", "rev-1", 1, false);
        let blue_new = document("blue", "rev-2", 2, false);
        let red_old = document("red", "rev-3", 3, false);
        let red_new = document("red", "rev-4", 4, false);

        assert!(matches!(
            authorizer.classify_visibility(&scope(), Some(&blue_old), &blue_new),
            VisibilityTransition::Visible(_)
        ));
        assert!(matches!(
            authorizer.classify_visibility(&scope(), Some(&red_old), &blue_new),
            VisibilityTransition::Visible(_)
        ));
        let VisibilityTransition::SyntheticTombstone(tombstone) =
            authorizer.classify_visibility(&scope(), Some(&blue_old), &red_new)
        else {
            panic!("revoked visibility must emit a tombstone");
        };
        assert_eq!(tombstone.document_id().as_str(), "doc-1");
        assert_eq!(tombstone.revision().as_str(), "rev-4");
        assert!(tombstone.is_deleted());
        assert!(matches!(
            authorizer.classify_visibility(&scope(), Some(&red_old), &red_new),
            VisibilityTransition::Hidden
        ));
    }

    fn policy() -> CompiledPolicySet {
        let set = PolicySet::new(
            scope(),
            PolicyVersion::new(1).expect("version"),
            PolicyState::Validated,
            [PolicyRule::new(
                PolicyRuleId::parse("team-read").expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Read],
                "old.team == claims.team",
            )
            .expect("rule")],
            [],
        )
        .expect("policy");
        PolicyCompiler::default()
            .compile(
                &set,
                &json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "team": {"type": "string"},
                        "secret": {"type": "string"}
                    }
                }),
            )
            .expect("compile")
            .into_compiled()
            .expect("compiled")
    }

    fn document(team: &str, revision: &str, position: u64, deleted: bool) -> CanonicalDocument {
        CanonicalDocument::new(
            DocumentId::parse("doc-1").expect("id"),
            SchemaVersion::new(1).expect("schema"),
            RevisionToken::parse(revision).expect("revision"),
            CommitPosition::new(position).expect("position"),
            deleted,
            json!({"id": "doc-1", "team": team, "secret": "protected"}),
        )
        .expect("document")
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
