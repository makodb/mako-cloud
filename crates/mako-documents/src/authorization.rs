use crate::{CanonicalDocument, ScopedCollectionEngine};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationAuthorizationOperation {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug)]
pub struct MutationAuthorizationContext<'a> {
    scope: &'a mako_api::CollectionScope,
    operation: MutationAuthorizationOperation,
    old_document: Option<&'a CanonicalDocument>,
    new_document: &'a CanonicalDocument,
}

impl<'a> MutationAuthorizationContext<'a> {
    pub fn new(
        engine: &'a ScopedCollectionEngine,
        operation: MutationAuthorizationOperation,
        old_document: Option<&'a CanonicalDocument>,
        new_document: &'a CanonicalDocument,
    ) -> Self {
        Self {
            scope: engine.scope(),
            operation,
            old_document,
            new_document,
        }
    }

    #[must_use]
    pub const fn scope(&self) -> &'a mako_api::CollectionScope {
        self.scope
    }

    #[must_use]
    pub const fn operation(&self) -> MutationAuthorizationOperation {
        self.operation
    }

    #[must_use]
    pub const fn old_document(&self) -> Option<&'a CanonicalDocument> {
        self.old_document
    }

    #[must_use]
    pub const fn new_document(&self) -> &'a CanonicalDocument {
        self.new_document
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationAuthorizationDecision {
    allowed: bool,
    stable_code: String,
}

impl MutationAuthorizationDecision {
    #[must_use]
    pub fn allow(stable_code: impl Into<String>) -> Self {
        Self {
            allowed: true,
            stable_code: stable_code.into(),
        }
    }

    #[must_use]
    pub fn deny(stable_code: impl Into<String>) -> Self {
        Self {
            allowed: false,
            stable_code: stable_code.into(),
        }
    }

    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        self.allowed
    }

    #[must_use]
    pub fn stable_code(&self) -> &str {
        &self.stable_code
    }
}

pub trait DocumentMutationAuthorizer: Send + Sync {
    fn authorize(&self, context: MutationAuthorizationContext<'_>)
    -> MutationAuthorizationDecision;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadAuthorizationPath {
    Point,
    TrustedQuery,
    ReplicationPull,
    LiveStream,
    ConflictResponse,
    EdgeSdk,
    OperatorImpersonation,
    ExplorerBrowse,
}

#[derive(Clone, Copy, Debug)]
pub struct ReadAuthorizationContext<'a> {
    scope: &'a mako_api::CollectionScope,
    path: ReadAuthorizationPath,
    document: &'a CanonicalDocument,
}

impl<'a> ReadAuthorizationContext<'a> {
    pub fn new(
        engine: &'a ScopedCollectionEngine,
        path: ReadAuthorizationPath,
        document: &'a CanonicalDocument,
    ) -> Self {
        Self {
            scope: engine.scope(),
            path,
            document,
        }
    }

    #[must_use]
    pub fn for_scope(
        scope: &'a mako_api::CollectionScope,
        path: ReadAuthorizationPath,
        document: &'a CanonicalDocument,
    ) -> Self {
        Self {
            scope,
            path,
            document,
        }
    }

    #[must_use]
    pub const fn scope(&self) -> &'a mako_api::CollectionScope {
        self.scope
    }

    #[must_use]
    pub const fn path(&self) -> ReadAuthorizationPath {
        self.path
    }

    #[must_use]
    pub const fn document(&self) -> &'a CanonicalDocument {
        self.document
    }
}

pub trait DocumentReadAuthorizer: Send + Sync {
    fn authorize_read(
        &self,
        context: ReadAuthorizationContext<'_>,
    ) -> MutationAuthorizationDecision;
}
