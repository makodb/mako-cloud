use std::{error::Error, fmt, sync::Arc};

use mako_api::{CollectionScope, ScopeError, TenantScope};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    TenantKeyspace, WriteBatch,
};
use serde_json::Value;

use crate::{
    AuthorizationEpochError, CompiledPolicySet, PolicyCompilation, PolicyCompiler, PolicyDecision,
    PolicyDiagnostic, PolicyEvaluationContext, PolicyEvaluator, PolicyModelError, PolicySet,
    PolicyState, PolicyVersion, authorization_epoch::prepare_environment_advance,
};

#[derive(Clone)]
pub struct PolicyStore {
    adapter: Arc<dyn KvAdapter>,
    scope: CollectionScope,
    keyspace: TenantKeyspace,
    durability: Durability,
}

impl fmt::Debug for PolicyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PolicyStore")
            .field("scope", &self.scope)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl PolicyStore {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        trusted_tenant: &TenantScope,
        scope: CollectionScope,
        durability: Durability,
    ) -> Result<Self, PolicyStoreError> {
        scope.tenant().ensure_matches(trusted_tenant)?;
        if adapter.capabilities().strongest_durability < durability {
            return Err(PolicyStoreError::UnsupportedDurability(durability));
        }
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str().as_bytes(),
            trusted_tenant.environment_id().as_str().as_bytes(),
        )?;
        Ok(Self {
            adapter,
            scope,
            keyspace,
            durability,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    pub async fn create_draft(&self, policy: &PolicySet) -> Result<(), PolicyStoreError> {
        if policy.scope() != &self.scope {
            return Err(PolicyStoreError::ScopeMismatch);
        }
        if policy.state() != PolicyState::Draft {
            return Err(PolicyStoreError::NewVersionMustBeDraft);
        }
        let key = self.version_key(policy.version())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, policy.encode()?);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(PolicyStoreError::VersionAlreadyExists),
        }
    }

    pub async fn policy_version(
        &self,
        version: PolicyVersion,
    ) -> Result<Option<PolicySet>, PolicyStoreError> {
        let key = self.version_key(version)?;
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(PolicySet::decode)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn active_policy(&self) -> Result<Option<PolicySet>, PolicyStoreError> {
        let Some(version) = self.active_version().await? else {
            return Ok(None);
        };
        self.policy_version(version)
            .await?
            .ok_or(PolicyStoreError::CorruptActivePointer)
            .map(Some)
    }

    pub async fn validate_version(
        &self,
        version: PolicyVersion,
        schema: &Value,
        compiler: &PolicyCompiler,
    ) -> Result<PolicyValidation, PolicyStoreError> {
        let key = self.version_key(version)?;
        let stored = self
            .adapter
            .get(&key)
            .await?
            .ok_or(PolicyStoreError::VersionNotFound)?;
        let policy = PolicySet::decode(&stored)?;
        let compilation = compiler.compile(&policy, schema)?;
        let state = if compilation.compiled().is_some() {
            PolicyState::Validated
        } else {
            PolicyState::Draft
        };
        let updated = policy.with_state(state, compilation.diagnostics().to_vec())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, updated.encode()?);
        let result = self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals { key, value: stored }],
                batch,
                durability: self.durability,
            })
            .await?;
        if result != CompareAndWriteResult::Applied {
            return Err(PolicyStoreError::ConcurrentLifecycleChange);
        }
        Ok(PolicyValidation {
            policy: updated,
            compilation,
        })
    }

    pub async fn activate(
        &self,
        version: PolicyVersion,
        schema: &Value,
        compiler: &PolicyCompiler,
    ) -> Result<CompiledPolicySet, PolicyStoreError> {
        if self.active_version().await? == Some(version) {
            let policy = self
                .policy_version(version)
                .await?
                .ok_or(PolicyStoreError::CorruptActivePointer)?;
            if policy.state() != PolicyState::Active {
                return Err(PolicyStoreError::CorruptActivePointer);
            }
            let compilation = compiler.compile(&policy, schema)?;
            if compilation.compiled().is_none() {
                return Err(PolicyStoreError::ValidationFailed(
                    compilation.diagnostics().to_vec(),
                ));
            }
            return Ok(compilation
                .into_compiled()
                .expect("compiled policy was checked as present"));
        }
        let validation = self.validate_version(version, schema, compiler).await?;
        let Some(compiled) = validation.compilation.into_compiled() else {
            return Err(PolicyStoreError::ValidationFailed(
                validation.policy.diagnostics().to_vec(),
            ));
        };

        let candidate_key = self.version_key(version)?;
        let active_key = self.active_key()?;
        for _ in 0..8 {
            let candidate_bytes = self
                .adapter
                .get(&candidate_key)
                .await?
                .ok_or(PolicyStoreError::VersionNotFound)?;
            let candidate = PolicySet::decode(&candidate_bytes)?;
            let active_bytes = self.adapter.get(&active_key).await?;
            let active_version = active_bytes.as_deref().map(decode_version).transpose()?;
            if active_version == Some(version) && candidate.state() == PolicyState::Active {
                return Ok(compiled);
            }
            let authorization = prepare_environment_advance(
                self.adapter.as_ref(),
                &self.keyspace,
                self.scope.collection_id().as_str(),
                version.get(),
            )
            .await?;
            let active_record = match active_version {
                Some(active_version) if active_version != version => {
                    let key = self.version_key(active_version)?;
                    let bytes = self
                        .adapter
                        .get(&key)
                        .await?
                        .ok_or(PolicyStoreError::CorruptActivePointer)?;
                    Some((key, bytes.clone(), PolicySet::decode(&bytes)?))
                }
                _ => None,
            };

            let activated = candidate.with_state(PolicyState::Active, Vec::new())?;
            let mut conditions = vec![KeyCondition::ValueEquals {
                key: candidate_key.clone(),
                value: candidate_bytes,
            }];
            conditions.push(match active_bytes {
                Some(value) => KeyCondition::ValueEquals {
                    key: active_key.clone(),
                    value,
                },
                None => KeyCondition::Missing {
                    key: active_key.clone(),
                },
            });
            let mut batch = WriteBatch::with_capacity(3);
            batch.put(&candidate_key, activated.encode()?);
            batch.put(&active_key, version.get().to_be_bytes());
            if let Some((key, bytes, active)) = active_record {
                conditions.push(KeyCondition::ValueEquals {
                    key: key.clone(),
                    value: bytes,
                });
                batch.put(
                    key,
                    active
                        .with_state(PolicyState::Retired, Vec::new())?
                        .encode()?,
                );
            }
            conditions.extend(authorization.conditions);
            for (key, value) in authorization.writes {
                batch.put(key, value);
            }
            if self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions,
                    batch,
                    durability: self.durability,
                })
                .await?
                == CompareAndWriteResult::Applied
            {
                return Ok(compiled);
            }
        }
        Err(PolicyStoreError::ConcurrentLifecycleChange)
    }

    pub async fn rollback(
        &self,
        version: PolicyVersion,
        schema: &Value,
        compiler: &PolicyCompiler,
    ) -> Result<CompiledPolicySet, PolicyStoreError> {
        self.activate(version, schema, compiler).await
    }

    pub async fn test_examples(
        &self,
        version: PolicyVersion,
        schema: &Value,
        compiler: &PolicyCompiler,
        examples: &[PolicyEvaluationContext],
    ) -> Result<Vec<PolicyDecision>, PolicyStoreError> {
        let policy = self
            .policy_version(version)
            .await?
            .ok_or(PolicyStoreError::VersionNotFound)?;
        let compilation = compiler.compile(&policy, schema)?;
        if compilation.compiled().is_none() {
            return Err(PolicyStoreError::ValidationFailed(
                compilation.diagnostics().to_vec(),
            ));
        }
        let compiled = compilation
            .into_compiled()
            .expect("compiled policy was checked as present");
        let evaluator = PolicyEvaluator;
        Ok(examples
            .iter()
            .map(|context| evaluator.evaluate(&compiled, context))
            .collect())
    }

    async fn active_version(&self) -> Result<Option<PolicyVersion>, PolicyStoreError> {
        let key = self.active_key()?;
        self.adapter
            .get(&key)
            .await?
            .as_deref()
            .map(decode_version)
            .transpose()
    }

    fn version_key(&self, version: PolicyVersion) -> Result<Vec<u8>, PolicyStoreError> {
        self.keyspace
            .policy_version_key(self.scope.collection_id().as_str(), version.get())
            .map_err(Into::into)
    }

    fn active_key(&self) -> Result<Vec<u8>, PolicyStoreError> {
        self.keyspace
            .active_policy_key(self.scope.collection_id().as_str())
            .map_err(Into::into)
    }
}

pub struct PolicyValidation {
    policy: PolicySet,
    compilation: PolicyCompilation,
}

impl PolicyValidation {
    #[must_use]
    pub fn policy(&self) -> &PolicySet {
        &self.policy
    }

    #[must_use]
    pub fn compilation(&self) -> &PolicyCompilation {
        &self.compilation
    }
}

fn decode_version(bytes: &[u8]) -> Result<PolicyVersion, PolicyStoreError> {
    let encoded: [u8; 8] = bytes
        .try_into()
        .map_err(|_| PolicyStoreError::CorruptActivePointer)?;
    PolicyVersion::new(u64::from_be_bytes(encoded)).map_err(Into::into)
}

#[derive(Debug)]
pub enum PolicyStoreError {
    Scope(ScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Model(PolicyModelError),
    Compile(crate::PolicyCompileError),
    AuthorizationEpoch(AuthorizationEpochError),
    UnsupportedDurability(Durability),
    ScopeMismatch,
    NewVersionMustBeDraft,
    VersionAlreadyExists,
    VersionNotFound,
    CorruptActivePointer,
    ConcurrentLifecycleChange,
    ValidationFailed(Vec<PolicyDiagnostic>),
}

impl fmt::Display for PolicyStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Model(error) => error.fmt(formatter),
            Self::Compile(error) => error.fmt(formatter),
            Self::AuthorizationEpoch(error) => error.fmt(formatter),
            Self::UnsupportedDurability(durability) => {
                write!(
                    formatter,
                    "storage does not support requested durability {durability:?}"
                )
            }
            Self::ScopeMismatch => formatter.write_str("policy scope does not match policy store"),
            Self::NewVersionMustBeDraft => {
                formatter.write_str("new policy versions must start as drafts")
            }
            Self::VersionAlreadyExists => formatter.write_str("policy version already exists"),
            Self::VersionNotFound => formatter.write_str("policy version was not found"),
            Self::CorruptActivePointer => formatter.write_str("active policy pointer is corrupt"),
            Self::ConcurrentLifecycleChange => {
                formatter.write_str("policy lifecycle changed concurrently")
            }
            Self::ValidationFailed(diagnostics) => write!(
                formatter,
                "policy validation failed with {} diagnostics",
                diagnostics.len()
            ),
        }
    }
}

impl Error for PolicyStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Compile(error) => Some(error),
            Self::AuthorizationEpoch(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ScopeError> for PolicyStoreError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for PolicyStoreError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for PolicyStoreError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<PolicyModelError> for PolicyStoreError {
    fn from(error: PolicyModelError) -> Self {
        Self::Model(error)
    }
}

impl From<crate::PolicyCompileError> for PolicyStoreError {
    fn from(error: crate::PolicyCompileError) -> Self {
        Self::Compile(error)
    }
}

impl From<AuthorizationEpochError> for PolicyStoreError {
    fn from(error: AuthorizationEpochError) -> Self {
        Self::AuthorizationEpoch(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;
    use serde_json::json;

    use super::*;
    use crate::{
        AuthorizationEpochStore, DocumentOperation, PolicyEffect, PolicyRule, PolicyRuleId,
        SubjectId,
    };

    #[test]
    fn validation_activation_failure_and_rollback_preserve_atomic_history() {
        futures::executor::block_on(async {
            let adapter = Arc::new(MemoryAdapter::new());
            let scope = scope();
            let store = PolicyStore::new(
                adapter.clone(),
                scope.tenant(),
                scope.clone(),
                Durability::Memory,
            )
            .expect("store");
            let epochs = AuthorizationEpochStore::new(
                adapter,
                scope.tenant(),
                scope.tenant(),
                Durability::Memory,
            )
            .expect("epoch store");
            let subject = SubjectId::parse("user-a").expect("subject");
            let compiler = PolicyCompiler::default();
            let schema = schema();
            let first = policy(1, "true");
            store.create_draft(&first).await.expect("draft one");
            store
                .activate(first.version(), &schema, &compiler)
                .await
                .expect("activate one");
            assert_eq!(
                store
                    .active_policy()
                    .await
                    .expect("active")
                    .expect("policy")
                    .version(),
                first.version()
            );
            assert_eq!(
                epochs
                    .epochs_for(&subject)
                    .await
                    .expect("epochs")
                    .environment()
                    .get(),
                1
            );

            let invalid = policy(2, "old.missing == true");
            store.create_draft(&invalid).await.expect("draft invalid");
            assert!(matches!(
                store.activate(invalid.version(), &schema, &compiler).await,
                Err(PolicyStoreError::ValidationFailed(_))
            ));
            assert_eq!(
                store
                    .active_policy()
                    .await
                    .expect("active")
                    .expect("policy")
                    .version(),
                first.version()
            );
            assert_eq!(
                epochs
                    .epochs_for(&subject)
                    .await
                    .expect("epochs")
                    .environment()
                    .get(),
                1
            );

            let third = policy(3, "old.owner_id == identity.user_id");
            store.create_draft(&third).await.expect("draft three");
            store
                .activate(third.version(), &schema, &compiler)
                .await
                .expect("activate three");
            assert_eq!(
                store
                    .policy_version(first.version())
                    .await
                    .expect("version")
                    .expect("first")
                    .state(),
                PolicyState::Retired
            );
            assert_eq!(
                epochs
                    .epochs_for(&subject)
                    .await
                    .expect("epochs")
                    .environment()
                    .get(),
                2
            );
            store
                .rollback(first.version(), &schema, &compiler)
                .await
                .expect("rollback");
            assert_eq!(
                store
                    .active_policy()
                    .await
                    .expect("active")
                    .expect("policy")
                    .version(),
                first.version()
            );
            assert_eq!(
                store
                    .policy_version(third.version())
                    .await
                    .expect("version")
                    .expect("third")
                    .state(),
                PolicyState::Retired
            );
            assert_eq!(
                epochs
                    .epochs_for(&subject)
                    .await
                    .expect("epochs")
                    .environment()
                    .get(),
                3
            );
            store
                .rollback(first.version(), &schema, &compiler)
                .await
                .expect("idempotent rollback");
            assert_eq!(
                epochs
                    .epochs_for(&subject)
                    .await
                    .expect("epochs")
                    .environment()
                    .get(),
                3
            );
        });
    }

    fn policy(version: u64, expression: &str) -> PolicySet {
        PolicySet::new(
            scope(),
            PolicyVersion::new(version).expect("version"),
            PolicyState::Draft,
            [PolicyRule::new(
                PolicyRuleId::parse(format!("read-{version}")).expect("id"),
                PolicyEffect::Allow,
                [DocumentOperation::Read],
                expression,
            )
            .expect("rule")],
            [],
        )
        .expect("policy")
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "owner_id": {"type": "string"}
            }
        })
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
