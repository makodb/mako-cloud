use std::{error::Error, fmt, sync::Arc};

use mako_api::{CollectionId, CollectionScope, TenantScope};
use mako_documents::{CollectionMetadata, CollectionMetadataError, DocumentEngine};
use mako_policy::{
    AuthorizationEpochError, AuthorizationEpochStore, PolicyCompiler, PolicyContextError,
    PolicyDecision, PolicyEvaluationContext, PolicyModelError, PolicyRule, PolicySet, PolicyState,
    PolicyStore, PolicyStoreError, PolicyVersion,
};
use mako_storage::{Durability, KvAdapter, StorageError};
use serde_json::Value;

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink,
    DeveloperPrincipal, OrganizationId, OrganizationStore, OrganizationStoreError, ProjectStore,
    ProjectStoreError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewPolicyDraft {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub version: u64,
    pub rules: Vec<PolicyRule>,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyValidationView {
    pub policy: PolicySet,
    pub valid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyExampleResult {
    pub allowed: bool,
    pub code: String,
    pub matched_rule_ids: Vec<String>,
    pub evaluated_rules: usize,
}

impl From<PolicyDecision> for PolicyExampleResult {
    fn from(decision: PolicyDecision) -> Self {
        Self {
            allowed: decision.is_allowed(),
            code: decision.stable_code().to_owned(),
            matched_rule_ids: decision
                .matched_rule_ids()
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            evaluated_rules: decision.evaluated_rules(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivePolicyView {
    pub policy: Option<PolicySet>,
    pub authorization_epoch: u64,
    pub default_deny: bool,
}

#[derive(Clone)]
pub struct PolicyAdminService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    compiler: PolicyCompiler,
}

impl fmt::Debug for PolicyAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PolicyAdminService")
            .field("durability", &self.durability)
            .field("compiler", &self.compiler)
            .finish_non_exhaustive()
    }
}

impl PolicyAdminService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        compiler: PolicyCompiler,
    ) -> Result<Self, PolicyAdminError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(PolicyAdminError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
            compiler,
        })
    }

    pub async fn create_draft(
        &self,
        actor: &DeveloperPrincipal,
        input: NewPolicyDraft,
    ) -> Result<PolicySet, PolicyAdminError> {
        let organization_id = self
            .authorize(
                actor,
                &input.tenant,
                true,
                ControlAuditAction::PolicyDraftCreate,
                input.now_unix_seconds,
            )
            .await?;
        self.collection_schema(&input.tenant, &input.collection_id)
            .await?;
        let policy = PolicySet::new(
            CollectionScope::new(input.tenant.clone(), input.collection_id.clone()),
            PolicyVersion::new(input.version)?,
            PolicyState::Draft,
            input.rules,
            [],
        )?;
        self.store(&input.tenant, &input.collection_id)?
            .create_draft(&policy)
            .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::PolicyDraftCreate,
            &input.tenant,
            &input.collection_id,
            input.version,
            input.now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(policy)
    }

    pub async fn get_policy(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        now_unix_seconds: u64,
    ) -> Result<PolicySet, PolicyAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::PolicyRead,
                now_unix_seconds,
            )
            .await?;
        let version = PolicyVersion::new(version)?;
        let policy = self
            .store(tenant, collection_id)?
            .policy_version(version)
            .await?
            .ok_or(PolicyAdminError::NotFound)?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::PolicyRead,
            tenant,
            collection_id,
            version.get(),
            now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(policy)
    }

    pub async fn active_policy(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        now_unix_seconds: u64,
    ) -> Result<ActivePolicyView, PolicyAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::PolicyRead,
                now_unix_seconds,
            )
            .await?;
        let policy = self.store(tenant, collection_id)?.active_policy().await?;
        let epoch = self.epochs(tenant)?.environment_epoch().await?.get();
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::PolicyRead,
            tenant,
            collection_id,
            policy.as_ref().map_or(0, |policy| policy.version().get()),
            now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(ActivePolicyView {
            default_deny: policy.is_none(),
            policy,
            authorization_epoch: epoch,
        })
    }

    pub async fn validate(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        now_unix_seconds: u64,
    ) -> Result<PolicyValidationView, PolicyAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                true,
                ControlAuditAction::PolicyValidate,
                now_unix_seconds,
            )
            .await?;
        let schema = self.collection_schema(tenant, collection_id).await?;
        let version = PolicyVersion::new(version)?;
        let validation = self
            .store(tenant, collection_id)?
            .validate_version(version, &schema, &self.compiler)
            .await?;
        let view = PolicyValidationView {
            valid: validation.compilation().compiled().is_some(),
            policy: validation.policy().clone(),
        };
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::PolicyValidate,
            tenant,
            collection_id,
            version.get(),
            now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(view)
    }

    pub async fn test_examples(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        examples: &[PolicyEvaluationContext],
        now_unix_seconds: u64,
    ) -> Result<Vec<PolicyExampleResult>, PolicyAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                true,
                ControlAuditAction::PolicyTest,
                now_unix_seconds,
            )
            .await?;
        if examples.is_empty() || examples.len() > 100 {
            return Err(PolicyAdminError::InvalidExamples);
        }
        let schema = self.collection_schema(tenant, collection_id).await?;
        let version = PolicyVersion::new(version)?;
        let decisions = self
            .store(tenant, collection_id)?
            .test_examples(version, &schema, &self.compiler, examples)
            .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::PolicyTest,
            tenant,
            collection_id,
            version.get(),
            now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(decisions.into_iter().map(Into::into).collect())
    }

    pub async fn activate(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        now_unix_seconds: u64,
    ) -> Result<ActivePolicyView, PolicyAdminError> {
        self.activate_or_rollback(
            actor,
            tenant,
            collection_id,
            version,
            now_unix_seconds,
            false,
        )
        .await
    }

    pub async fn rollback(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        now_unix_seconds: u64,
    ) -> Result<ActivePolicyView, PolicyAdminError> {
        self.activate_or_rollback(
            actor,
            tenant,
            collection_id,
            version,
            now_unix_seconds,
            true,
        )
        .await
    }

    async fn activate_or_rollback(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        now_unix_seconds: u64,
        rollback: bool,
    ) -> Result<ActivePolicyView, PolicyAdminError> {
        let action = if rollback {
            ControlAuditAction::PolicyRollback
        } else {
            ControlAuditAction::PolicyActivate
        };
        let organization_id = self
            .authorize(actor, tenant, true, action, now_unix_seconds)
            .await?;
        let schema = self.collection_schema(tenant, collection_id).await?;
        let version = PolicyVersion::new(version)?;
        let store = self.store(tenant, collection_id)?;
        if rollback {
            store.rollback(version, &schema, &self.compiler).await?;
        } else {
            store.activate(version, &schema, &self.compiler).await?;
        }
        let policy = store
            .active_policy()
            .await?
            .ok_or(PolicyAdminError::NotFound)?;
        let authorization_epoch = self.epochs(tenant)?.environment_epoch().await?.get();
        self.audit(
            actor,
            &organization_id,
            action,
            tenant,
            collection_id,
            version.get(),
            now_unix_seconds,
            ControlAuditOutcome::Allowed,
        );
        Ok(ActivePolicyView {
            policy: Some(policy),
            authorization_epoch,
            default_deny: false,
        })
    }

    async fn collection_schema(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
    ) -> Result<Value, PolicyAdminError> {
        let scoped = DocumentEngine::new(self.adapter.clone()).scope_collection(
            tenant,
            CollectionScope::new(tenant.clone(), collection_id.clone()),
        )?;
        let metadata = self
            .adapter
            .get(&scoped.collection_metadata_key()?)
            .await?
            .ok_or(PolicyAdminError::NotFound)
            .and_then(|bytes| CollectionMetadata::decode(&bytes).map_err(Into::into))?;
        Ok(Value::Object(metadata.json_schema().clone()))
    }

    fn store(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
    ) -> Result<PolicyStore, PolicyAdminError> {
        Ok(PolicyStore::new(
            self.adapter.clone(),
            tenant,
            CollectionScope::new(tenant.clone(), collection_id.clone()),
            self.durability,
        )?)
    }

    fn epochs(&self, tenant: &TenantScope) -> Result<AuthorizationEpochStore, PolicyAdminError> {
        Ok(AuthorizationEpochStore::new(
            self.adapter.clone(),
            tenant,
            tenant,
            self.durability,
        )?)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        mutation: bool,
        action: ControlAuditAction,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, PolicyAdminError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(PolicyAdminError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(PolicyAdminError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        let Some(membership) = membership else {
            self.audit(
                actor,
                project.organization_id(),
                action,
                tenant,
                &CollectionId::parse("authorization").expect("static collection"),
                0,
                now_unix_seconds,
                ControlAuditOutcome::Denied,
            );
            return Err(PolicyAdminError::Forbidden);
        };
        if mutation && !membership.role().can_mutate_projects() {
            return Err(PolicyAdminError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        version: u64,
        at_unix_seconds: u64,
        outcome: ControlAuditOutcome,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization_id.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}/{}/{}",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str(),
                collection_id.as_str(),
                version
            ),
            outcome,
            at_unix_seconds,
        });
    }
}

#[derive(Debug)]
pub enum PolicyAdminError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    InvalidExamples,
    DocumentScope(mako_documents::DocumentEngineScopeError),
    Collection(CollectionMetadataError),
    Model(PolicyModelError),
    Context(PolicyContextError),
    Store(PolicyStoreError),
    Epoch(AuthorizationEpochError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Storage(StorageError),
}

impl fmt::Display for PolicyAdminError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "policy durability is unsupported",
            Self::NotFound => "policy resource was not found",
            Self::Forbidden => "policy action is forbidden",
            Self::InvalidExamples => "policy tests require between 1 and 100 examples",
            Self::DocumentScope(_) => "policy collection scope is invalid",
            Self::Collection(_) => "policy collection metadata is invalid",
            Self::Model(_) => "policy model is invalid",
            Self::Context(_) => "policy example context is invalid",
            Self::Store(_) => "policy lifecycle operation failed",
            Self::Epoch(_) => "authorization epoch operation failed",
            Self::Project(_) => "policy project lookup failed",
            Self::Organization(_) => "policy organization lookup failed",
            Self::Storage(_) => "policy storage operation failed",
        })
    }
}

impl Error for PolicyAdminError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for PolicyAdminError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(mako_documents::DocumentEngineScopeError, DocumentScope);
error_from!(CollectionMetadataError, Collection);
error_from!(PolicyModelError, Model);
error_from!(PolicyContextError, Context);
error_from!(PolicyStoreError, Store);
error_from!(AuthorizationEpochError, Epoch);
error_from!(ProjectStoreError, Project);
error_from!(OrganizationStoreError, Organization);
error_from!(StorageError, Storage);

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mako_api::{EnvironmentId, ProjectId};
    use mako_documents::{
        CollectionLifecycle, CollectionMetadataVersion, PrimaryKeyDefinition, SchemaCompatibility,
        SchemaVersion,
    };
    use mako_policy::{
        DocumentOperation, PolicyEffect, PolicyRuleId, SafeRequestMetadata, SubjectId,
        VerifiedIdentity, VerifiedRole,
    };
    use mako_storage::{MemoryAdapter, WriteBatch};
    use serde_json::json;

    use super::*;
    use crate::{
        DeveloperIdentityId, EnvironmentRecord, MembershipRecord, OrganizationRecord,
        OrganizationRole, ProjectRecord,
    };

    #[derive(Default)]
    struct AuditLog(Mutex<Vec<ControlAuditEvent>>);

    impl ControlAuditSink for AuditLog {
        fn record(&self, event: ControlAuditEvent) {
            self.0.lock().expect("audit lock").push(event);
        }
    }

    async fn fixture() -> (
        PolicyAdminService,
        DeveloperPrincipal,
        TenantScope,
        CollectionId,
    ) {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let organizations =
            OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
        let projects = ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
        let organization_id = OrganizationId::parse("org_example00").expect("organization");
        let developer_id = DeveloperIdentityId::parse("dev_example00").expect("developer");
        organizations
            .create_organization(
                &OrganizationRecord::new(organization_id.clone(), "Example", 1)
                    .expect("organization"),
                &MembershipRecord::new(
                    organization_id.clone(),
                    developer_id.clone(),
                    OrganizationRole::Owner,
                    1,
                ),
            )
            .await
            .expect("organization");
        let project_id = ProjectId::parse("prj_example00").expect("project");
        let environment_id = EnvironmentId::parse("env_example00").expect("environment");
        projects
            .create_project(
                &ProjectRecord::new(project_id.clone(), organization_id, "Mako", "local", 1)
                    .expect("project"),
            )
            .await
            .expect("project");
        projects
            .create_environment(
                &EnvironmentRecord::new(
                    environment_id.clone(),
                    project_id.clone(),
                    "Development",
                    1,
                )
                .expect("environment"),
            )
            .await
            .expect("environment");
        let tenant = TenantScope::new(project_id, environment_id);
        let collection_id = CollectionId::parse("todos").expect("collection");
        let metadata = CollectionMetadata::new(
            collection_id.clone(),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "required": ["id", "owner"],
                "properties": {
                    "id": { "type": "string" },
                    "owner": { "type": "string" }
                }
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        let scoped = DocumentEngine::new(adapter.clone())
            .scope_collection(
                &tenant,
                CollectionScope::new(tenant.clone(), collection_id.clone()),
            )
            .expect("scope");
        let mut batch = WriteBatch::new();
        batch.put(
            scoped.collection_metadata_key().expect("key"),
            metadata.encode().expect("encode"),
        );
        adapter
            .write(batch, Durability::Memory)
            .await
            .expect("metadata");
        let service = PolicyAdminService::new(
            adapter,
            Durability::Memory,
            projects,
            organizations,
            Arc::new(AuditLog::default()),
            PolicyCompiler::default(),
        )
        .expect("service");
        (
            service,
            DeveloperPrincipal::for_test(developer_id, "owner@example.test"),
            tenant,
            collection_id,
        )
    }

    fn rule(expression: &str) -> PolicyRule {
        PolicyRule::new(
            PolicyRuleId::parse("owner-read").expect("rule id"),
            PolicyEffect::Allow,
            [DocumentOperation::Read],
            expression,
        )
        .expect("rule")
    }

    fn example(tenant: &TenantScope, collection_id: &CollectionId) -> PolicyEvaluationContext {
        PolicyEvaluationContext::new(
            CollectionScope::new(tenant.clone(), collection_id.clone()),
            DocumentOperation::Read,
            VerifiedIdentity::user(
                SubjectId::parse("user-a").expect("subject"),
                VerifiedRole::parse("authenticated").expect("role"),
                None,
                json!({}),
            )
            .expect("identity"),
            Some(json!({ "id": "one", "owner": "user-a" })),
            None,
            SafeRequestMetadata::empty(),
        )
        .expect("context")
    }

    #[test]
    fn draft_validation_testing_activation_and_rollback_report_epochs() {
        futures::executor::block_on(async {
            let (service, actor, tenant, collection_id) = fixture().await;
            let empty = service
                .active_policy(&actor, &tenant, &collection_id, 2)
                .await
                .expect("active");
            assert!(empty.default_deny);
            assert_eq!(empty.authorization_epoch, 0);

            service
                .create_draft(
                    &actor,
                    NewPolicyDraft {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        version: 1,
                        rules: vec![rule("old.owner == identity.user_id")],
                        now_unix_seconds: 3,
                    },
                )
                .await
                .expect("draft");
            assert!(
                service
                    .validate(&actor, &tenant, &collection_id, 1, 4)
                    .await
                    .expect("validate")
                    .valid
            );
            let results = service
                .test_examples(
                    &actor,
                    &tenant,
                    &collection_id,
                    1,
                    &[example(&tenant, &collection_id)],
                    5,
                )
                .await
                .expect("test");
            assert!(results[0].allowed);
            assert_eq!(
                service
                    .activate(&actor, &tenant, &collection_id, 1, 6)
                    .await
                    .expect("activate")
                    .authorization_epoch,
                1
            );

            service
                .create_draft(
                    &actor,
                    NewPolicyDraft {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        version: 2,
                        rules: vec![rule("false")],
                        now_unix_seconds: 7,
                    },
                )
                .await
                .expect("draft two");
            assert_eq!(
                service
                    .activate(&actor, &tenant, &collection_id, 2, 8)
                    .await
                    .expect("activate two")
                    .authorization_epoch,
                2
            );
            let rolled_back = service
                .rollback(&actor, &tenant, &collection_id, 1, 9)
                .await
                .expect("rollback");
            assert_eq!(rolled_back.policy.expect("policy").version().get(), 1);
            assert_eq!(rolled_back.authorization_epoch, 3);
        });
    }

    #[test]
    fn invalid_draft_never_replaces_the_active_policy() {
        futures::executor::block_on(async {
            let (service, actor, tenant, collection_id) = fixture().await;
            service
                .create_draft(
                    &actor,
                    NewPolicyDraft {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        version: 1,
                        rules: vec![rule("true")],
                        now_unix_seconds: 2,
                    },
                )
                .await
                .expect("draft");
            service
                .activate(&actor, &tenant, &collection_id, 1, 3)
                .await
                .expect("activate");
            service
                .create_draft(
                    &actor,
                    NewPolicyDraft {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        version: 2,
                        rules: vec![rule("old.missing == true")],
                        now_unix_seconds: 4,
                    },
                )
                .await
                .expect("invalid draft");
            assert!(matches!(
                service
                    .activate(&actor, &tenant, &collection_id, 2, 5)
                    .await,
                Err(PolicyAdminError::Store(PolicyStoreError::ValidationFailed(
                    _
                )))
            ));
            assert_eq!(
                service
                    .active_policy(&actor, &tenant, &collection_id, 6)
                    .await
                    .expect("active")
                    .policy
                    .expect("policy")
                    .version()
                    .get(),
                1
            );
        });
    }
}
