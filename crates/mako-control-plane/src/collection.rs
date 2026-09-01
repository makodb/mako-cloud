use std::{error::Error, fmt, num::NonZeroUsize, sync::Arc};

use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, ScopeError, TenantScope};
use mako_documents::{
    CanonicalDocument, CollectionLifecycle, CollectionMetadata, CollectionMetadataError,
    CollectionMetadataVersion, DocumentEngine, DocumentEngineScopeError, DocumentValidationError,
    DocumentValidator, IndexBuildError, IndexBuildProgress, IndexDefinition, IndexError,
    IndexField, IndexKind, IndexName, IndexVersion, PrimaryKeyDefinition, SchemaCompatibility,
    SchemaVersion, ScopedCollectionEngine,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, StorageError, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperPrincipal, OrganizationId, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
};

const COMPATIBILITY_SCAN_PAGE_SIZE: usize = 256;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SchemaMigrationId(String);

impl SchemaMigrationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CollectionAdminError> {
        let value = value.into();
        let Some(suffix) = value.strip_prefix("mig_") else {
            return Err(CollectionAdminError::InvalidMigrationId);
        };
        if !(8..=64).contains(&suffix.len())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(CollectionAdminError::InvalidMigrationId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SchemaMigrationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMigrationState {
    Planned,
    Running,
    Failed,
    Completed,
    Cancelled,
}

impl SchemaMigrationState {
    const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Failed | Self::Completed | Self::Cancelled
                )
                | (Self::Failed, Self::Running | Self::Cancelled)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SchemaMigrationRecord {
    id: SchemaMigrationId,
    project_id: ProjectId,
    environment_id: EnvironmentId,
    collection_id: CollectionId,
    from_schema_version: u64,
    to_schema_version: u64,
    state: SchemaMigrationState,
    reason: String,
    compatibility_issues: Vec<String>,
    #[serde(default)]
    request_fingerprint: String,
    created_at_unix_seconds: u64,
    updated_at_unix_seconds: u64,
}

impl SchemaMigrationRecord {
    #[must_use]
    pub fn id(&self) -> &SchemaMigrationId {
        &self.id
    }

    #[must_use]
    pub const fn state(&self) -> SchemaMigrationState {
        self.state
    }

    #[must_use]
    pub const fn from_schema_version(&self) -> u64 {
        self.from_schema_version
    }

    #[must_use]
    pub const fn to_schema_version(&self) -> u64 {
        self.to_schema_version
    }

    #[must_use]
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    #[must_use]
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment_id
    }

    #[must_use]
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub fn compatibility_issues(&self) -> &[String] {
        &self.compatibility_issues
    }

    #[must_use]
    pub fn matches_request(
        &self,
        target_schema_version: u64,
        target_json_schema: &Value,
        target_primary_key: &PrimaryKeyDefinition,
        reason: &str,
    ) -> bool {
        !self.request_fingerprint.is_empty()
            && self.request_fingerprint
                == migration_request_fingerprint(
                    target_schema_version,
                    target_json_schema,
                    target_primary_key,
                    reason,
                )
    }

    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    #[must_use]
    pub const fn updated_at_unix_seconds(&self) -> u64 {
        self.updated_at_unix_seconds
    }

    pub fn transition(
        &mut self,
        next: SchemaMigrationState,
        now_unix_seconds: u64,
    ) -> Result<(), CollectionAdminError> {
        if !self.state.can_transition_to(next) || now_unix_seconds < self.updated_at_unix_seconds {
            return Err(CollectionAdminError::InvalidMigrationTransition);
        }
        self.state = next;
        self.updated_at_unix_seconds = now_unix_seconds;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewCollection {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub schema_version: u64,
    pub json_schema: Value,
    pub primary_key: PrimaryKeyDefinition,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishSchema {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub schema_version: u64,
    pub json_schema: Value,
    pub primary_key: PrimaryKeyDefinition,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewSchemaMigration {
    pub id: SchemaMigrationId,
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub target_schema_version: u64,
    pub target_json_schema: Value,
    pub target_primary_key: PrimaryKeyDefinition,
    pub reason: String,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewIndex {
    pub tenant: TenantScope,
    pub collection_id: CollectionId,
    pub name: IndexName,
    pub version: u64,
    pub kind: IndexKind,
    pub fields: Vec<IndexField>,
    pub now_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionCompatibilityReport {
    compatible: bool,
    documents_checked: u64,
    issues: Vec<String>,
}

impl CollectionCompatibilityReport {
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.compatible
    }

    #[must_use]
    pub const fn documents_checked(&self) -> u64 {
        self.documents_checked
    }

    #[must_use]
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchemaPublicationOutcome {
    Published(CollectionMetadata),
    MigrationRequired(CollectionCompatibilityReport),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexBuildStatus {
    pub definition: IndexDefinition,
    pub progress: Option<IndexBuildProgress>,
}

#[derive(Clone)]
pub struct CollectionAdminService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
}

impl fmt::Debug for CollectionAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CollectionAdminService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl CollectionAdminService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
    ) -> Result<Self, CollectionAdminError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(CollectionAdminError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
        })
    }

    pub async fn create_collection(
        &self,
        actor: &DeveloperPrincipal,
        input: NewCollection,
    ) -> Result<CollectionMetadata, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                &input.tenant,
                true,
                ControlAuditAction::CollectionCreate,
                input.now_unix_seconds,
            )
            .await?;
        // The collection is not usable until its metadata reaches the data
        // plane that serves this environment, which owns a store the control
        // plane cannot write to directly. It is recorded as Creating here and
        // activated only after that propagation succeeds, so no management
        // surface reports a collection active while document traffic would
        // reject it as not found.
        let metadata = CollectionMetadata::new(
            input.collection_id,
            CollectionMetadataVersion::new(1)?,
            SchemaVersion::new(input.schema_version)?,
            input.json_schema,
            input.primary_key,
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Creating,
        )?;
        DocumentValidator::compile(&metadata)?;
        let scoped = self.scoped(&input.tenant, metadata.collection_id().clone())?;
        let key = scoped.collection_metadata_key()?;
        let mut batch = WriteBatch::new();
        batch.put(&key, metadata.encode()?);
        self.create_record(key, batch).await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionCreate,
            &input.tenant,
            metadata.collection_id().as_str(),
            input.now_unix_seconds,
        );
        Ok(metadata)
    }

    /// Mark a created collection active once its metadata has been durably
    /// installed in the data plane that serves the environment.
    ///
    /// Idempotent: activating an already active collection returns the existing
    /// record, so a retried creation converges instead of conflicting.
    pub async fn activate_collection(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        now_unix_seconds: u64,
    ) -> Result<CollectionMetadata, CollectionAdminError> {
        let current = self.collection(tenant, collection_id).await?;
        match current.lifecycle() {
            CollectionLifecycle::Active => return Ok(current),
            CollectionLifecycle::Creating => {}
            CollectionLifecycle::Suspended | CollectionLifecycle::Deleting => {
                return Err(CollectionAdminError::Conflict);
            }
        }
        let _ = now_unix_seconds;
        let active = CollectionMetadata::new(
            current.collection_id().clone(),
            current.metadata_version(),
            current.schema_version(),
            Value::Object(current.json_schema().clone()),
            current.primary_key().clone(),
            current.compatibility(),
            CollectionLifecycle::Active,
        )?;
        self.replace_collection(tenant, &current, &active).await?;
        Ok(active)
    }

    /// The metadata a data plane must hold to serve this collection. The
    /// control-plane record may still be Creating; what the data plane stores
    /// is always the servable, active form.
    pub fn servable_metadata(
        metadata: &CollectionMetadata,
    ) -> Result<CollectionMetadata, CollectionAdminError> {
        if metadata.lifecycle() == CollectionLifecycle::Active {
            return Ok(metadata.clone());
        }
        CollectionMetadata::new(
            metadata.collection_id().clone(),
            metadata.metadata_version(),
            metadata.schema_version(),
            Value::Object(metadata.json_schema().clone()),
            metadata.primary_key().clone(),
            metadata.compatibility(),
            CollectionLifecycle::Active,
        )
        .map_err(Into::into)
    }

    pub async fn get_collection(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        now_unix_seconds: u64,
    ) -> Result<CollectionMetadata, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::CollectionRead,
                now_unix_seconds,
            )
            .await?;
        let metadata = self.collection(tenant, collection_id).await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionRead,
            tenant,
            collection_id.as_str(),
            now_unix_seconds,
        );
        Ok(metadata)
    }

    pub async fn list_collections(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        limit: NonZeroUsize,
        now_unix_seconds: u64,
    ) -> Result<Vec<CollectionMetadata>, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::CollectionRead,
                now_unix_seconds,
            )
            .await?;
        let keyspace = TenantKeyspace::new(
            tenant.project_id().as_str(),
            tenant.environment_id().as_str(),
        )?;
        let page = self
            .adapter
            .scan(ScanRequest::new(
                keyspace.collections_range()?,
                ScanDirection::Forward,
                limit,
            ))
            .await?;
        let collections = page
            .into_iter()
            .map(|entry| CollectionMetadata::decode(&entry.value))
            .collect::<Result<Vec<_>, _>>()?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionRead,
            tenant,
            "collections",
            now_unix_seconds,
        );
        Ok(collections)
    }

    pub async fn publish_schema(
        &self,
        actor: &DeveloperPrincipal,
        input: PublishSchema,
    ) -> Result<SchemaPublicationOutcome, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                &input.tenant,
                true,
                ControlAuditAction::CollectionSchemaPublish,
                input.now_unix_seconds,
            )
            .await?;
        let current = self.collection(&input.tenant, &input.collection_id).await?;
        if input.schema_version <= current.schema_version().get() {
            return Err(CollectionAdminError::SchemaVersionMustIncrease);
        }
        let next_metadata_version = current
            .metadata_version()
            .get()
            .checked_add(1)
            .ok_or(CollectionAdminError::VersionExhausted)?;
        let proposed = CollectionMetadata::new(
            input.collection_id,
            CollectionMetadataVersion::new(next_metadata_version)?,
            SchemaVersion::new(input.schema_version)?,
            input.json_schema,
            input.primary_key,
            SchemaCompatibility::PendingValidation,
            CollectionLifecycle::Active,
        )?;
        let report = self
            .compatibility_report(&input.tenant, &current, &proposed)
            .await?;
        if !report.is_compatible() {
            return Ok(SchemaPublicationOutcome::MigrationRequired(report));
        }
        let published = CollectionMetadata::new(
            proposed.collection_id().clone(),
            proposed.metadata_version(),
            proposed.schema_version(),
            Value::Object(proposed.json_schema().clone()),
            proposed.primary_key().clone(),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )?;
        self.replace_collection(&input.tenant, &current, &published)
            .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionSchemaPublish,
            &input.tenant,
            published.collection_id().as_str(),
            input.now_unix_seconds,
        );
        Ok(SchemaPublicationOutcome::Published(published))
    }

    pub async fn create_migration(
        &self,
        actor: &DeveloperPrincipal,
        input: NewSchemaMigration,
    ) -> Result<SchemaMigrationRecord, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                &input.tenant,
                true,
                ControlAuditAction::CollectionMigrationCreate,
                input.now_unix_seconds,
            )
            .await?;
        if input.reason.is_empty()
            || input.reason.len() > 500
            || input.reason.chars().any(char::is_control)
        {
            return Err(CollectionAdminError::InvalidMigrationReason);
        }
        let current = self.collection(&input.tenant, &input.collection_id).await?;
        if input.target_schema_version <= current.schema_version().get() {
            return Err(CollectionAdminError::SchemaVersionMustIncrease);
        }
        let request_fingerprint = migration_request_fingerprint(
            input.target_schema_version,
            &input.target_json_schema,
            &input.target_primary_key,
            &input.reason,
        );
        let proposed = CollectionMetadata::new(
            input.collection_id.clone(),
            CollectionMetadataVersion::new(
                current
                    .metadata_version()
                    .get()
                    .checked_add(1)
                    .ok_or(CollectionAdminError::VersionExhausted)?,
            )?,
            SchemaVersion::new(input.target_schema_version)?,
            input.target_json_schema,
            input.target_primary_key,
            SchemaCompatibility::MigrationRequired,
            CollectionLifecycle::Active,
        )?;
        let report = self
            .compatibility_report(&input.tenant, &current, &proposed)
            .await?;
        if report.is_compatible() {
            return Err(CollectionAdminError::MigrationNotRequired);
        }
        let record = SchemaMigrationRecord {
            id: input.id,
            project_id: input.tenant.project_id().clone(),
            environment_id: input.tenant.environment_id().clone(),
            collection_id: input.collection_id,
            from_schema_version: current.schema_version().get(),
            to_schema_version: input.target_schema_version,
            state: SchemaMigrationState::Planned,
            reason: input.reason,
            compatibility_issues: report.issues,
            request_fingerprint,
            created_at_unix_seconds: input.now_unix_seconds,
            updated_at_unix_seconds: input.now_unix_seconds,
        };
        let key = migration_key(&record)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&record)?);
        self.create_record(key, batch).await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionMigrationCreate,
            &input.tenant,
            record.id().as_str(),
            input.now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn get_migration(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        migration_id: &SchemaMigrationId,
        now_unix_seconds: u64,
    ) -> Result<SchemaMigrationRecord, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::CollectionMigrationRead,
                now_unix_seconds,
            )
            .await?;
        let key = ControlKeyspace::schema_migration_key(
            tenant.project_id(),
            tenant.environment_id(),
            collection_id,
            migration_id,
        )?;
        let record = self
            .adapter
            .get(&key)
            .await?
            .ok_or(CollectionAdminError::NotFound)
            .and_then(|value| {
                serde_json::from_slice::<SchemaMigrationRecord>(&value).map_err(Into::into)
            })?;
        if record.project_id != *tenant.project_id()
            || record.environment_id != *tenant.environment_id()
            || record.collection_id != *collection_id
        {
            return Err(CollectionAdminError::CorruptRecord);
        }
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionMigrationRead,
            tenant,
            migration_id.as_str(),
            now_unix_seconds,
        );
        Ok(record)
    }

    pub async fn transition_migration(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        migration_id: &SchemaMigrationId,
        next: SchemaMigrationState,
        now_unix_seconds: u64,
    ) -> Result<SchemaMigrationRecord, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                true,
                ControlAuditAction::CollectionMigrationCreate,
                now_unix_seconds,
            )
            .await?;
        let previous = self
            .get_migration_record(tenant, collection_id, migration_id)
            .await?;
        let mut updated = previous.clone();
        updated.transition(next, now_unix_seconds)?;
        let key = migration_key(&previous)?;
        self.replace_record(
            key,
            serde_json::to_vec(&previous)?,
            serde_json::to_vec(&updated)?,
        )
        .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionMigrationCreate,
            tenant,
            migration_id.as_str(),
            now_unix_seconds,
        );
        Ok(updated)
    }

    pub async fn create_index(
        &self,
        actor: &DeveloperPrincipal,
        input: NewIndex,
    ) -> Result<IndexBuildStatus, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                &input.tenant,
                true,
                ControlAuditAction::CollectionIndexCreate,
                input.now_unix_seconds,
            )
            .await?;
        self.collection(&input.tenant, &input.collection_id).await?;
        let scoped = self.scoped(&input.tenant, input.collection_id.clone())?;
        let definition = IndexDefinition::new_building(
            input.collection_id,
            input.name.clone(),
            IndexVersion::new(input.version)?,
            input.kind,
            input.fields,
        )?;
        scoped
            .create_index(definition.clone(), self.durability)
            .await?;
        let progress = scoped
            .index_build_progress(&input.name, definition.version())
            .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionIndexCreate,
            &input.tenant,
            input.name.as_str(),
            input.now_unix_seconds,
        );
        Ok(IndexBuildStatus {
            definition,
            progress,
        })
    }

    pub async fn list_indexes(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        now_unix_seconds: u64,
    ) -> Result<Vec<IndexBuildStatus>, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::CollectionIndexRead,
                now_unix_seconds,
            )
            .await?;
        self.collection(tenant, collection_id).await?;
        let scoped = self.scoped(tenant, collection_id.clone())?;
        let mut statuses = Vec::new();
        for definition in scoped.index_definitions().await? {
            let progress = scoped
                .index_build_progress(definition.name(), definition.version())
                .await?;
            statuses.push(IndexBuildStatus {
                definition,
                progress,
            });
        }
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionIndexRead,
            tenant,
            collection_id.as_str(),
            now_unix_seconds,
        );
        Ok(statuses)
    }

    pub async fn get_index(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        name: &IndexName,
        version: IndexVersion,
        now_unix_seconds: u64,
    ) -> Result<IndexBuildStatus, CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                false,
                ControlAuditAction::CollectionIndexRead,
                now_unix_seconds,
            )
            .await?;
        self.collection(tenant, collection_id).await?;
        let scoped = self.scoped(tenant, collection_id.clone())?;
        let definition = scoped
            .index_definition(name, version)
            .await?
            .ok_or(CollectionAdminError::NotFound)?;
        let progress = scoped.index_build_progress(name, version).await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionIndexRead,
            tenant,
            name.as_str(),
            now_unix_seconds,
        );
        Ok(IndexBuildStatus {
            definition,
            progress,
        })
    }

    pub async fn delete_index(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        name: &IndexName,
        version: IndexVersion,
        now_unix_seconds: u64,
    ) -> Result<(), CollectionAdminError> {
        let organization_id = self
            .authorize(
                actor,
                tenant,
                true,
                ControlAuditAction::CollectionIndexDelete,
                now_unix_seconds,
            )
            .await?;
        let scoped = self.scoped(tenant, collection_id.clone())?;
        scoped
            .mark_index_deleting(name, version, self.durability)
            .await?;
        self.audit(
            actor,
            &organization_id,
            ControlAuditAction::CollectionIndexDelete,
            tenant,
            name.as_str(),
            now_unix_seconds,
        );
        Ok(())
    }

    async fn compatibility_report(
        &self,
        tenant: &TenantScope,
        current: &CollectionMetadata,
        proposed: &CollectionMetadata,
    ) -> Result<CollectionCompatibilityReport, CollectionAdminError> {
        let validator = DocumentValidator::compile(proposed)?;
        let mut issues = static_compatibility_issues(current, proposed);
        let scoped = self.scoped(tenant, current.collection_id().clone())?;
        let mut range = scoped.documents_range()?;
        let page_size =
            NonZeroUsize::new(COMPATIBILITY_SCAN_PAGE_SIZE).expect("page size is positive");
        let mut documents_checked = 0_u64;
        loop {
            let page = self
                .adapter
                .scan(ScanRequest::new(
                    range.clone(),
                    ScanDirection::Forward,
                    page_size,
                ))
                .await?;
            if page.is_empty() {
                break;
            }
            for entry in &page {
                let document = CanonicalDocument::decode(&entry.value)
                    .map_err(CollectionAdminError::Document)?;
                documents_checked = documents_checked.saturating_add(1);
                if validator
                    .validate_update(&document, Value::Object(document.body().clone()))
                    .is_err()
                {
                    issues.push(format!(
                        "stored document {} does not satisfy the proposed schema",
                        document.primary_key().as_str()
                    ));
                    if issues.len() >= 100 {
                        break;
                    }
                }
            }
            if page.len() < page_size.get() || issues.len() >= 100 {
                break;
            }
            range.start_inclusive = page.last().expect("non-empty page").key.clone();
            range.start_inclusive.push(0);
            if range.start_inclusive >= range.end_exclusive {
                break;
            }
        }
        Ok(CollectionCompatibilityReport {
            compatible: issues.is_empty(),
            documents_checked,
            issues,
        })
    }

    async fn collection(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
    ) -> Result<CollectionMetadata, CollectionAdminError> {
        let scoped = self.scoped(tenant, collection_id.clone())?;
        self.adapter
            .get(&scoped.collection_metadata_key()?)
            .await?
            .ok_or(CollectionAdminError::NotFound)
            .and_then(|bytes| CollectionMetadata::decode(&bytes).map_err(Into::into))
    }

    async fn replace_collection(
        &self,
        tenant: &TenantScope,
        previous: &CollectionMetadata,
        next: &CollectionMetadata,
    ) -> Result<(), CollectionAdminError> {
        let scoped = self.scoped(tenant, previous.collection_id().clone())?;
        self.replace_record(
            scoped.collection_metadata_key()?,
            previous.encode()?,
            next.encode()?,
        )
        .await
    }

    async fn get_migration_record(
        &self,
        tenant: &TenantScope,
        collection_id: &CollectionId,
        migration_id: &SchemaMigrationId,
    ) -> Result<SchemaMigrationRecord, CollectionAdminError> {
        let key = ControlKeyspace::schema_migration_key(
            tenant.project_id(),
            tenant.environment_id(),
            collection_id,
            migration_id,
        )?;
        self.adapter
            .get(&key)
            .await?
            .ok_or(CollectionAdminError::NotFound)
            .and_then(|value| serde_json::from_slice(&value).map_err(Into::into))
    }

    fn scoped(
        &self,
        tenant: &TenantScope,
        collection_id: CollectionId,
    ) -> Result<ScopedCollectionEngine, CollectionAdminError> {
        DocumentEngine::new(self.adapter.clone())
            .scope_collection(tenant, CollectionScope::new(tenant.clone(), collection_id))
            .map_err(Into::into)
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        mutation: bool,
        action: ControlAuditAction,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, CollectionAdminError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(CollectionAdminError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(CollectionAdminError::NotFound)?;
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
                "authorization",
                now_unix_seconds,
            );
            return Err(CollectionAdminError::Forbidden);
        };
        if mutation && !membership.role().can_mutate_projects() {
            self.audit(
                actor,
                project.organization_id(),
                action,
                tenant,
                "authorization",
                now_unix_seconds,
            );
            return Err(CollectionAdminError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    async fn create_record(
        &self,
        key: Vec<u8>,
        batch: WriteBatch,
    ) -> Result<(), CollectionAdminError> {
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
            CompareAndWriteResult::Conflict { .. } => Err(CollectionAdminError::Conflict),
        }
    }

    async fn replace_record(
        &self,
        key: Vec<u8>,
        previous: Vec<u8>,
        next: Vec<u8>,
    ) -> Result<(), CollectionAdminError> {
        let mut batch = WriteBatch::new();
        batch.put(&key, next);
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::ValueEquals {
                    key,
                    value: previous,
                }],
                batch,
                durability: self.durability,
            })
            .await?
        {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(CollectionAdminError::Conflict),
        }
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization_id: &OrganizationId,
        action: ControlAuditAction,
        tenant: &TenantScope,
        target: &str,
        outcome_at: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization_id.clone(),
            actor_id: actor.identity_id().clone(),
            action,
            target: format!(
                "{}/{}/{}",
                tenant.project_id().as_str(),
                tenant.environment_id().as_str(),
                target
            ),
            outcome: if target == "authorization" {
                ControlAuditOutcome::Denied
            } else {
                ControlAuditOutcome::Allowed
            },
            at_unix_seconds: outcome_at,
        });
    }
}

fn static_compatibility_issues(
    current: &CollectionMetadata,
    proposed: &CollectionMetadata,
) -> Vec<String> {
    let mut issues = Vec::new();
    if current.primary_key() != proposed.primary_key() {
        issues.push("primary-key changes require a migration".to_owned());
    }
    let current_schema = current.json_schema();
    let proposed_schema = proposed.json_schema();
    if current_schema.get("type") != proposed_schema.get("type") {
        issues.push("the root schema type changed".to_owned());
    }
    let current_properties = object_at(current_schema, "properties");
    let proposed_properties = object_at(proposed_schema, "properties");
    if let Some(current_properties) = current_properties {
        for (name, definition) in current_properties {
            match proposed_properties.and_then(|properties| properties.get(name)) {
                Some(proposed_definition) if proposed_definition == definition => {}
                // A field may *widen*: the same definition with a superset of
                // its `enum` values (or the enum lifted entirely) accepts
                // every value the old definition accepted, so no stored
                // document and no old writer can be broken by it. Anything
                // else -- a narrowing, a type change, a removal -- still
                // requires a migration.
                Some(proposed_definition) if field_widened(definition, proposed_definition) => {}
                _ => issues.push(format!("existing field {name} was removed or changed")),
            }
        }
    }
    let current_required = strings_at(current_schema, "required");
    for required in strings_at(proposed_schema, "required") {
        if !current_required.contains(&required) {
            issues.push(format!(
                "new required field {required} is incompatible with existing clients"
            ));
        }
    }
    if current_schema.get("additionalProperties") != Some(&Value::Bool(false))
        && proposed_schema.get("additionalProperties") == Some(&Value::Bool(false))
    {
        issues.push("additional properties became forbidden".to_owned());
    }
    issues
}

/// True when `proposed` accepts everything `current` accepts by construction:
/// identical apart from the `enum` keyword, where the proposed set is a
/// superset of the current one or the constraint is dropped altogether.
fn field_widened(current: &Value, proposed: &Value) -> bool {
    let (Some(current), Some(proposed)) = (current.as_object(), proposed.as_object()) else {
        return false;
    };
    let strip = |definition: &Map<String, Value>| {
        let mut rest = definition.clone();
        rest.remove("enum");
        rest
    };
    if strip(current) != strip(proposed) {
        return false;
    }
    let Some(current_values) = current.get("enum").and_then(Value::as_array) else {
        // The current field carries no enum: only an identical definition
        // (handled by the caller) or a *new* restriction could differ here,
        // and a new restriction is a narrowing.
        return false;
    };
    match proposed.get("enum").and_then(Value::as_array) {
        // The constraint was lifted: everything the old enum allowed remains allowed.
        None => !proposed.contains_key("enum"),
        Some(proposed_values) => current_values
            .iter()
            .all(|value| proposed_values.contains(value)),
    }
}

fn object_at<'a>(schema: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    schema.get(key).and_then(Value::as_object)
}

fn strings_at(schema: &Map<String, Value>, key: &str) -> Vec<String> {
    schema
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn migration_key(record: &SchemaMigrationRecord) -> Result<Vec<u8>, CollectionAdminError> {
    ControlKeyspace::schema_migration_key(
        &record.project_id,
        &record.environment_id,
        &record.collection_id,
        &record.id,
    )
    .map_err(Into::into)
}

fn migration_request_fingerprint(
    target_schema_version: u64,
    target_json_schema: &Value,
    target_primary_key: &PrimaryKeyDefinition,
    reason: &str,
) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("mako/schema-migration-request/v1");
    hasher.update(&target_schema_version.to_be_bytes());
    for encoded in [
        serde_json::to_vec(target_json_schema).expect("JSON values serialize"),
        serde_json::to_vec(target_primary_key).expect("primary keys serialize"),
        reason.as_bytes().to_vec(),
    ] {
        hasher.update(&(encoded.len() as u64).to_be_bytes());
        hasher.update(&encoded);
    }
    hasher.finalize().to_hex().to_string()
}

#[derive(Debug)]
pub enum CollectionAdminError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    InvalidMigrationId,
    InvalidMigrationReason,
    InvalidMigrationTransition,
    MigrationNotRequired,
    SchemaVersionMustIncrease,
    VersionExhausted,
    Scope(ScopeError),
    EngineScope(DocumentEngineScopeError),
    Metadata(CollectionMetadataError),
    Validation(DocumentValidationError),
    Document(mako_documents::DocumentCodecError),
    Index(IndexError),
    IndexBuild(IndexBuildError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    StorageKey(mako_storage::KeyCodecError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for CollectionAdminError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "collection durability is unsupported",
            Self::NotFound => "collection resource was not found",
            Self::Forbidden => "collection action is forbidden",
            Self::Conflict => "collection resource changed concurrently",
            Self::CorruptRecord => "collection administration record is corrupt",
            Self::InvalidMigrationId => "schema migration identifier is invalid",
            Self::InvalidMigrationReason => "schema migration reason is invalid",
            Self::InvalidMigrationTransition => "schema migration state transition is invalid",
            Self::MigrationNotRequired => "the compatible schema can be published directly",
            Self::SchemaVersionMustIncrease => "schema version must increase",
            Self::VersionExhausted => "collection metadata version is exhausted",
            Self::Scope(_) => "collection scope is invalid",
            Self::EngineScope(_) => "document engine scope is invalid",
            Self::Metadata(_) => "collection metadata is invalid",
            Self::Validation(_) => "collection schema is invalid",
            Self::Document(_) => "stored document is invalid",
            Self::Index(_) => "collection index is invalid",
            Self::IndexBuild(_) => "collection index build state is invalid",
            Self::Project(_) => "project lookup failed",
            Self::Organization(_) => "organization lookup failed",
            Self::Keyspace(_) => "collection management key is invalid",
            Self::StorageKey(_) => "tenant collection key is invalid",
            Self::Storage(_) => "collection management storage operation failed",
            Self::Json(_) => "collection management record is invalid",
        })
    }
}

impl Error for CollectionAdminError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for CollectionAdminError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(ScopeError, Scope);
error_from!(DocumentEngineScopeError, EngineScope);
error_from!(CollectionMetadataError, Metadata);
error_from!(DocumentValidationError, Validation);
error_from!(mako_documents::DocumentCodecError, Document);
error_from!(IndexError, Index);
error_from!(IndexBuildError, IndexBuild);
error_from!(ProjectStoreError, Project);
error_from!(OrganizationStoreError, Organization);
error_from!(ControlKeyspaceError, Keyspace);
error_from!(mako_storage::KeyCodecError, StorageKey);
error_from!(StorageError, Storage);
error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    /// A widened enum accepts everything the old field accepted, so it is not
    /// "removed or changed" -- refusing it forced a migration for a change
    /// that can strand neither a stored document nor an old writer (finding
    /// #40). A narrowed enum, a dropped value, or a definition change beside
    /// the enum still is.
    #[test]
    fn a_widened_enum_is_compatible_and_a_narrowed_or_reshaped_field_is_not() {
        let field = |definition: serde_json::Value| {
            serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" }, "kind": definition },
                "required": ["id"],
            })
        };
        let metadata = |schema: serde_json::Value| {
            CollectionMetadata::new(
                CollectionId::parse("widening").expect("collection id"),
                CollectionMetadataVersion::new(1).expect("metadata version"),
                SchemaVersion::new(1).expect("schema version"),
                schema,
                mako_documents::PrimaryKeyDefinition::Field { field: "id".into() },
                SchemaCompatibility::Compatible,
                CollectionLifecycle::Active,
            )
            .expect("metadata")
        };
        let current = metadata(field(
            serde_json::json!({ "type": "string", "enum": ["institution", "import"] }),
        ));
        let superset = metadata(field(
            serde_json::json!({ "type": "string", "enum": ["institution", "plaid", "import"] }),
        ));
        let lifted = metadata(field(serde_json::json!({ "type": "string" })));
        let narrowed = metadata(field(
            serde_json::json!({ "type": "string", "enum": ["institution"] }),
        ));
        let reshaped = metadata(field(
            serde_json::json!({ "type": "integer", "enum": ["institution", "plaid", "import"] }),
        ));
        assert!(static_compatibility_issues(&current, &superset).is_empty());
        assert!(static_compatibility_issues(&current, &lifted).is_empty());
        assert!(!static_compatibility_issues(&current, &narrowed).is_empty());
        assert!(!static_compatibility_issues(&current, &reshaped).is_empty());
        // The reverse direction -- adding an enum where none existed -- is a
        // narrowing however it is spelled.
        assert!(!static_compatibility_issues(&lifted, &current).is_empty());
    }

    use mako_documents::{IndexDirection, IndexState};
    use mako_storage::MemoryAdapter;
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

    async fn fixture() -> (CollectionAdminService, DeveloperPrincipal, TenantScope) {
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
            .expect("create organization");
        let project_id = ProjectId::parse("prj_example00").expect("project");
        let environment_id = EnvironmentId::parse("env_example00").expect("environment");
        projects
            .create_project(
                &ProjectRecord::new(project_id.clone(), organization_id, "Mako", "local", 1)
                    .expect("project"),
            )
            .await
            .expect("create project");
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
            .expect("create environment");
        let service = CollectionAdminService::new(
            adapter,
            Durability::Memory,
            projects,
            organizations,
            Arc::new(AuditLog::default()),
        )
        .expect("service");
        (
            service,
            DeveloperPrincipal::for_test(developer_id, "owner@example.test"),
            TenantScope::new(project_id, environment_id),
        )
    }

    fn schema(extra: Value) -> Value {
        let mut value = json!({
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "string" } },
            "additionalProperties": false
        });
        if let (Some(properties), Some(extra)) =
            (value["properties"].as_object_mut(), extra.as_object())
        {
            properties.extend(extra.clone());
        }
        value
    }

    #[test]
    fn compatible_publication_succeeds_and_incompatible_change_requires_migration() {
        futures::executor::block_on(async {
            let (service, actor, tenant) = fixture().await;
            let collection_id = CollectionId::parse("todos").expect("collection");
            service
                .create_collection(
                    &actor,
                    NewCollection {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        schema_version: 1,
                        json_schema: schema(json!({})),
                        primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        now_unix_seconds: 2,
                    },
                )
                .await
                .expect("create collection");
            let published = service
                .publish_schema(
                    &actor,
                    PublishSchema {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        schema_version: 2,
                        json_schema: schema(json!({ "title": { "type": "string" } })),
                        primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        now_unix_seconds: 3,
                    },
                )
                .await
                .expect("publish");
            assert!(matches!(published, SchemaPublicationOutcome::Published(_)));

            let incompatible_schema = json!({
                "type": "object",
                "required": ["id", "title"],
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" }
                },
                "additionalProperties": false
            });
            let outcome = service
                .publish_schema(
                    &actor,
                    PublishSchema {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        schema_version: 3,
                        json_schema: incompatible_schema.clone(),
                        primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        now_unix_seconds: 4,
                    },
                )
                .await
                .expect("compatibility outcome");
            assert!(matches!(
                outcome,
                SchemaPublicationOutcome::MigrationRequired(_)
            ));
            assert_eq!(
                service
                    .get_collection(&actor, &tenant, &collection_id, 5)
                    .await
                    .expect("active schema after rejected publication")
                    .schema_version()
                    .get(),
                2,
                "an incompatible publication must leave the last compatible schema active"
            );
            let migration = service
                .create_migration(
                    &actor,
                    NewSchemaMigration {
                        id: SchemaMigrationId::parse("mig_example00").expect("migration"),
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        target_schema_version: 3,
                        target_json_schema: incompatible_schema,
                        target_primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        reason: "Backfill title before requiring it".to_owned(),
                        now_unix_seconds: 5,
                    },
                )
                .await
                .expect("migration");
            assert_eq!(migration.state(), SchemaMigrationState::Planned);
            assert_eq!(
                service
                    .get_migration(&actor, &tenant, &collection_id, migration.id(), 6)
                    .await
                    .expect("inspect"),
                migration
            );
        });
    }

    #[test]
    fn index_requests_expose_building_and_deleting_lifecycle() {
        futures::executor::block_on(async {
            let (service, actor, tenant) = fixture().await;
            let collection_id = CollectionId::parse("todos").expect("collection");
            service
                .create_collection(
                    &actor,
                    NewCollection {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        schema_version: 1,
                        json_schema: schema(json!({ "createdAt": { "type": "integer" } })),
                        primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        now_unix_seconds: 2,
                    },
                )
                .await
                .expect("create collection");
            let name = IndexName::parse("by-created-at").expect("name");
            let created = service
                .create_index(
                    &actor,
                    NewIndex {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        name: name.clone(),
                        version: 1,
                        kind: IndexKind::NonUnique,
                        fields: vec![
                            IndexField::new("createdAt", IndexDirection::Descending)
                                .expect("field"),
                        ],
                        now_unix_seconds: 3,
                    },
                )
                .await
                .expect("create index");
            assert_eq!(created.definition.state(), IndexState::Building);
            assert_eq!(
                service
                    .list_indexes(&actor, &tenant, &collection_id, 4)
                    .await
                    .expect("indexes")
                    .len(),
                1
            );
            service
                .delete_index(
                    &actor,
                    &tenant,
                    &collection_id,
                    &name,
                    IndexVersion::new(1).expect("version"),
                    5,
                )
                .await
                .expect("delete index");
            assert_eq!(
                service
                    .list_indexes(&actor, &tenant, &collection_id, 6)
                    .await
                    .expect("indexes")[0]
                    .definition
                    .state(),
                IndexState::Deleting
            );
        });
    }

    #[test]
    fn created_collection_stays_pending_until_activated_and_activation_is_idempotent() {
        futures::executor::block_on(async {
            let (service, actor, tenant) = fixture().await;
            let collection_id = CollectionId::parse("todos").expect("collection");
            let created = service
                .create_collection(
                    &actor,
                    NewCollection {
                        tenant: tenant.clone(),
                        collection_id: collection_id.clone(),
                        schema_version: 1,
                        json_schema: schema(json!({})),
                        primary_key: PrimaryKeyDefinition::field("id").expect("primary key"),
                        now_unix_seconds: 2,
                    },
                )
                .await
                .expect("create collection");

            // Until propagation to the data plane succeeds the collection is not
            // advertised as active, so no surface reports it usable while
            // document traffic would reject it.
            assert_eq!(created.lifecycle(), CollectionLifecycle::Creating);
            let stored = service
                .get_collection(&actor, &tenant, &collection_id, 3)
                .await
                .expect("read back");
            assert_eq!(stored.lifecycle(), CollectionLifecycle::Creating);

            // What the data plane must store is always the servable form.
            let servable =
                CollectionAdminService::servable_metadata(&created).expect("servable metadata");
            assert_eq!(servable.lifecycle(), CollectionLifecycle::Active);
            assert_eq!(servable.metadata_version(), created.metadata_version());
            assert_eq!(servable.schema_version(), created.schema_version());

            let activated = service
                .activate_collection(&tenant, &collection_id, 4)
                .await
                .expect("activate");
            assert_eq!(activated.lifecycle(), CollectionLifecycle::Active);

            // A retried creation converges rather than conflicting.
            let replayed = service
                .activate_collection(&tenant, &collection_id, 5)
                .await
                .expect("activate again");
            assert_eq!(replayed.lifecycle(), CollectionLifecycle::Active);
            assert_eq!(replayed.metadata_version(), activated.metadata_version());
        });
    }

    #[test]
    fn activating_an_unknown_collection_reports_not_found() {
        futures::executor::block_on(async {
            let (service, _actor, tenant) = fixture().await;
            let missing = CollectionId::parse("absent").expect("collection");
            let result = service.activate_collection(&tenant, &missing, 2).await;
            assert!(matches!(result, Err(CollectionAdminError::NotFound)));
        });
    }
}
