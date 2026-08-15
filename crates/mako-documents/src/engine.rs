use std::{error::Error, fmt, sync::Arc};

use mako_api::{CollectionScope, ScopeError, TenantScope};
use mako_storage::{AdapterCapabilities, KeyCodecError, KeyRange, KvAdapter, TenantKeyspace};

use crate::DocumentId;

/// Root document service. Callers must bind it to a trusted collection scope
/// before any collection or document key can be addressed.
#[derive(Clone)]
pub struct DocumentEngine {
    pub(crate) adapter: Arc<dyn KvAdapter>,
}

impl fmt::Debug for DocumentEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentEngine")
            .finish_non_exhaustive()
    }
}

impl DocumentEngine {
    #[must_use]
    pub fn new(adapter: Arc<dyn KvAdapter>) -> Self {
        Self { adapter }
    }

    pub fn scope_collection(
        &self,
        trusted_tenant: &TenantScope,
        requested: CollectionScope,
    ) -> Result<ScopedCollectionEngine, DocumentEngineScopeError> {
        requested.tenant().ensure_matches(trusted_tenant)?;
        let keyspace = TenantKeyspace::new(
            trusted_tenant.project_id().as_str().as_bytes(),
            trusted_tenant.environment_id().as_str().as_bytes(),
        )?;
        Ok(ScopedCollectionEngine {
            adapter: Arc::clone(&self.adapter),
            scope: requested,
            keyspace,
        })
    }
}

/// A document-engine handle whose keys are permanently bound to one verified
/// project, environment, and collection.
#[derive(Clone)]
pub struct ScopedCollectionEngine {
    pub(crate) adapter: Arc<dyn KvAdapter>,
    scope: CollectionScope,
    pub(crate) keyspace: TenantKeyspace,
}

impl fmt::Debug for ScopedCollectionEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopedCollectionEngine")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl ScopedCollectionEngine {
    #[must_use]
    pub fn scope(&self) -> &CollectionScope {
        &self.scope
    }

    pub fn collection_metadata_key(&self) -> Result<Vec<u8>, DocumentEngineScopeError> {
        self.keyspace
            .collection_key(self.scope.collection_id().as_str())
            .map_err(Into::into)
    }

    pub fn documents_range(&self) -> Result<KeyRange, DocumentEngineScopeError> {
        self.keyspace
            .documents_range(self.scope.collection_id().as_str())
            .map_err(Into::into)
    }

    pub fn document_key(
        &self,
        document_id: &DocumentId,
    ) -> Result<Vec<u8>, DocumentEngineScopeError> {
        self.keyspace
            .document_key(self.scope.collection_id().as_str(), document_id.as_str())
            .map_err(Into::into)
    }

    #[must_use]
    pub fn storage_capabilities(&self) -> AdapterCapabilities {
        self.adapter.capabilities()
    }
}

#[derive(Debug)]
pub enum DocumentEngineScopeError {
    Scope(ScopeError),
    KeyCodec(KeyCodecError),
}

impl fmt::Display for DocumentEngineScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
        }
    }
}

impl Error for DocumentEngineScopeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
        }
    }
}

impl From<ScopeError> for DocumentEngineScopeError {
    fn from(error: ScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<KeyCodecError> for DocumentEngineScopeError {
    fn from(error: KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

#[cfg(test)]
mod tests {
    use mako_api::{CollectionId, EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    fn tenant(project: &str, environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse(project).expect("project"),
            EnvironmentId::parse(environment).expect("environment"),
        )
    }

    fn collection(tenant: TenantScope, collection: &str) -> CollectionScope {
        CollectionScope::new(tenant, CollectionId::parse(collection).expect("collection"))
    }

    #[test]
    fn binds_keys_to_the_verified_scope() {
        let adapter = Arc::new(MemoryAdapter::new());
        let engine = DocumentEngine::new(adapter);
        let trusted = tenant("prj_abcdefgh", "env_abcdefgh");
        let scoped = engine
            .scope_collection(&trusted, collection(trusted.clone(), "todos"))
            .expect("matching scope");

        let document = DocumentId::parse("todo-1").expect("document id");
        let key = scoped.document_key(&document).expect("document key");
        assert!(scoped.documents_range().expect("range").contains(&key));
        assert_eq!(scoped.scope().tenant(), &trusted);
    }

    #[test]
    fn rejects_cross_project_and_cross_environment_requests() {
        let adapter = Arc::new(MemoryAdapter::new());
        let engine = DocumentEngine::new(adapter.clone());
        let trusted = tenant("prj_abcdefgh", "env_abcdefgh");

        let other_project = engine.scope_collection(
            &trusted,
            collection(tenant("prj_ijklmnop", "env_abcdefgh"), "todos"),
        );
        let other_environment = engine.scope_collection(
            &trusted,
            collection(tenant("prj_abcdefgh", "env_ijklmnop"), "todos"),
        );

        assert!(matches!(
            other_project,
            Err(DocumentEngineScopeError::Scope(ScopeError::TenantMismatch))
        ));
        assert!(matches!(
            other_environment,
            Err(DocumentEngineScopeError::Scope(ScopeError::TenantMismatch))
        ));
        assert!(adapter.dump().expect("adapter state").is_empty());
    }

    #[test]
    fn untrusted_collection_identifiers_cannot_enter_the_engine() {
        assert!(CollectionId::parse("../another-tenant").is_err());
        assert!(CollectionId::parse("Todos").is_err());
        assert!(CollectionId::parse("").is_err());
    }
}
