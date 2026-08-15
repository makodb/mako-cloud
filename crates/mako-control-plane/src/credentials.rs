use std::{error::Error, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use mako_api::TenantScope;
use mako_identity::{
    IdentityRecordError, IdentityStore, IdentityStoreError, IssuedProjectCredential,
    KeyEncryptionKey, ProjectCredentialId, ProjectCredentialMetadata, ProjectSigningKeyRecord,
    ProjectSigningKeyRing, ServiceCredentialScope, SigningKeyError, SigningKeyState,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, StorageError,
    WriteBatch,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
    ControlAuditAction, ControlAuditEvent, ControlAuditOutcome, ControlAuditSink, ControlKeyspace,
    ControlKeyspaceError, DeveloperPrincipal, OrganizationId, OrganizationRole, OrganizationStore,
    OrganizationStoreError, ProjectStore, ProjectStoreError,
};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FunctionSecretName(String);

impl FunctionSecretName {
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialAdminError> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = (1..=128).contains(&value.len())
            && bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if !valid {
            return Err(CredentialAdminError::InvalidSecretName);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for FunctionSecretName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionSecretState {
    Active,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionSecretMetadata {
    tenant: TenantScope,
    name: FunctionSecretName,
    version: u64,
    state: FunctionSecretState,
    created_at_unix_seconds: u64,
    rotated_at_unix_seconds: Option<u64>,
    retired_at_unix_seconds: Option<u64>,
}

impl FunctionSecretMetadata {
    #[must_use]
    pub fn name(&self) -> &FunctionSecretName {
        &self.name
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub const fn state(&self) -> FunctionSecretState {
        self.state
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct FunctionSecretValue(String);

impl FunctionSecretValue {
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialAdminError> {
        let value = value.into();
        if value.is_empty() || value.len() > 64 * 1024 || value.contains('\0') {
            return Err(CredentialAdminError::InvalidSecretValue);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose_once(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn expose_for_runtime(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn expose_for_redaction(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FunctionSecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionSecretValue([REDACTED])")
    }
}

#[derive(Debug)]
pub struct FunctionSecretIssue {
    pub metadata: FunctionSecretMetadata,
    pub value: FunctionSecretValue,
}

#[derive(Debug)]
pub struct ResolvedFunctionSecret {
    pub metadata: FunctionSecretMetadata,
    pub value: FunctionSecretValue,
}

#[derive(Clone, Eq, PartialEq)]
pub struct FunctionSecretEncryptionKey([u8; 32]);

impl FunctionSecretEncryptionKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

impl fmt::Debug for FunctionSecretEncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionSecretEncryptionKey([REDACTED])")
    }
}

/// Read-only control-plane authority for exact function-secret versions.
/// Unlike [`CredentialAdminService`], this type has no application-identity
/// methods and therefore cannot create a second data-plane identity authority.
#[derive(Clone)]
pub struct FunctionSecretResolver {
    adapter: Arc<dyn KvAdapter>,
    key: FunctionSecretEncryptionKey,
}

impl fmt::Debug for FunctionSecretResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionSecretResolver")
            .finish_non_exhaustive()
    }
}

impl FunctionSecretResolver {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        key: FunctionSecretEncryptionKey,
    ) -> Result<Self, CredentialAdminError> {
        if adapter.capabilities().strongest_durability < Durability::Sync {
            return Err(CredentialAdminError::UnsupportedDurability);
        }
        Ok(Self { adapter, key })
    }

    pub async fn resolve_version(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        version: u64,
    ) -> Result<ResolvedFunctionSecret, CredentialAdminError> {
        let key = ControlKeyspace::function_secret_version_key(
            tenant.project_id(),
            tenant.environment_id(),
            name,
            version,
        )?;
        let stored = self
            .adapter
            .get(&key)
            .await?
            .ok_or(CredentialAdminError::NotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<StoredFunctionSecret>(&bytes).map_err(Into::into)
            })?;
        if stored.metadata.tenant != *tenant
            || stored.metadata.name != *name
            || stored.metadata.version != version
        {
            return Err(CredentialAdminError::CorruptRecord);
        }
        Ok(ResolvedFunctionSecret {
            metadata: stored.metadata.clone(),
            value: decrypt_secret(&self.key, &stored)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredFunctionSecret {
    metadata: FunctionSecretMetadata,
    ciphertext: Vec<u8>,
    nonce: [u8; 24],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SigningKeyView {
    pub key_id: String,
    pub state: SigningKeyState,
    pub created_at_unix_seconds: u64,
    pub retire_at_unix_seconds: Option<u64>,
}

impl From<&ProjectSigningKeyRecord> for SigningKeyView {
    fn from(record: &ProjectSigningKeyRecord) -> Self {
        Self {
            key_id: record.key_id().to_owned(),
            state: record.state(),
            created_at_unix_seconds: record.created_at_unix_seconds(),
            retire_at_unix_seconds: record.retire_at_unix_seconds(),
        }
    }
}

#[derive(Clone)]
pub struct CredentialAdminService {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    projects: ProjectStore,
    organizations: OrganizationStore,
    audit: Arc<dyn ControlAuditSink>,
    signing_encryption_key: KeyEncryptionKey,
    function_encryption_key: FunctionSecretEncryptionKey,
}

impl fmt::Debug for CredentialAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialAdminService")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl CredentialAdminService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        projects: ProjectStore,
        organizations: OrganizationStore,
        audit: Arc<dyn ControlAuditSink>,
        signing_encryption_key: KeyEncryptionKey,
        function_encryption_key: FunctionSecretEncryptionKey,
    ) -> Result<Self, CredentialAdminError> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(CredentialAdminError::UnsupportedDurability);
        }
        Ok(Self {
            adapter,
            durability,
            projects,
            organizations,
            audit,
            signing_encryption_key,
            function_encryption_key,
        })
    }

    pub async fn create_public_key(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let issue = self
            .identity(tenant)?
            .create_public_project_key(id, now_unix_seconds)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialCreate,
            issue.metadata.id().as_str(),
            now_unix_seconds,
        );
        Ok(issue)
    }

    pub async fn create_service_credential(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: ProjectCredentialId,
        scope: ServiceCredentialScope,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let issue = self
            .identity(tenant)?
            .create_service_credential(id, scope, now_unix_seconds)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialCreate,
            issue.metadata.id().as_str(),
            now_unix_seconds,
        );
        Ok(issue)
    }

    pub async fn credential_metadata(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<ProjectCredentialMetadata, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let metadata = self
            .identity(tenant)?
            .project_credential_metadata(id)
            .await?
            .ok_or(CredentialAdminError::NotFound)?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialRead,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(metadata)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn rotate_credential(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        current_id: &ProjectCredentialId,
        replacement_id: ProjectCredentialId,
        overlap_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<IssuedProjectCredential, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let issue = self
            .identity(tenant)?
            .rotate_project_credential(
                current_id,
                replacement_id,
                overlap_seconds,
                now_unix_seconds,
            )
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialRotate,
            current_id.as_str(),
            now_unix_seconds,
        );
        Ok(issue)
    }

    pub async fn retire_credential(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        id: &ProjectCredentialId,
        now_unix_seconds: u64,
    ) -> Result<(), CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        self.identity(tenant)?
            .retire_project_credential(id, now_unix_seconds)
            .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialRetire,
            id.as_str(),
            now_unix_seconds,
        );
        Ok(())
    }

    pub async fn initialize_signing_key(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<SigningKeyView, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let (stored, mut ring) = self.load_ring(tenant).await?;
        let view = SigningKeyView::from(
            ring.create_initial(&self.signing_encryption_key, now_unix_seconds)?,
        );
        self.save_ring(tenant, stored, &ring).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::SigningKeyRotate,
            &view.key_id,
            now_unix_seconds,
        );
        Ok(view)
    }

    pub async fn rotate_signing_key(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        overlap_seconds: u64,
        now_unix_seconds: u64,
    ) -> Result<SigningKeyView, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let (stored, mut ring) = self.load_ring(tenant).await?;
        let view = SigningKeyView::from(ring.rotate(
            &self.signing_encryption_key,
            now_unix_seconds,
            overlap_seconds,
        )?);
        self.save_ring(tenant, stored, &ring).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::SigningKeyRotate,
            &view.key_id,
            now_unix_seconds,
        );
        Ok(view)
    }

    pub async fn signing_keys(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<Vec<SigningKeyView>, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let (_, ring) = self.load_ring(tenant).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialRead,
            "signing-keys",
            now_unix_seconds,
        );
        Ok(ring.records().iter().map(Into::into).collect())
    }

    pub async fn create_function_secret(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: FunctionSecretName,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretIssue, CredentialAdminError> {
        let value = generate_function_secret();
        let metadata = self
            .create_function_secret_value(actor, tenant, name, &value, now_unix_seconds)
            .await?;
        Ok(FunctionSecretIssue { metadata, value })
    }

    /// Stores a caller-supplied value, such as a scoped service credential,
    /// without ever returning that value from the control-plane operation.
    pub async fn create_function_secret_with_value(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: FunctionSecretName,
        value: FunctionSecretValue,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        self.create_function_secret_value(actor, tenant, name, &value, now_unix_seconds)
            .await
    }

    async fn create_function_secret_value(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: FunctionSecretName,
        value: &FunctionSecretValue,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let metadata = FunctionSecretMetadata {
            tenant: tenant.clone(),
            name,
            version: 1,
            state: FunctionSecretState::Active,
            created_at_unix_seconds: now_unix_seconds,
            rotated_at_unix_seconds: None,
            retired_at_unix_seconds: None,
        };
        let stored = self.encrypt_secret(metadata.clone(), value)?;
        let key = secret_key(&metadata)?;
        let version_key = secret_version_key(&metadata)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(&stored)?);
        batch.put(&version_key, serde_json::to_vec(&stored)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::Missing { key },
                KeyCondition::Missing { key: version_key },
            ],
            batch,
            durability: self.durability,
        })
        .await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionSecretCreate,
            metadata.name.as_str(),
            now_unix_seconds,
        );
        Ok(metadata)
    }

    pub async fn function_secret_metadata(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let stored = self.stored_secret(tenant, name).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::CredentialRead,
            name.as_str(),
            now_unix_seconds,
        );
        Ok(stored.metadata)
    }

    pub async fn rotate_function_secret(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretIssue, CredentialAdminError> {
        let value = generate_function_secret();
        let metadata = self
            .rotate_function_secret_value(actor, tenant, name, &value, now_unix_seconds)
            .await?;
        Ok(FunctionSecretIssue { metadata, value })
    }

    /// Rotates to a caller-supplied value without returning that value.
    pub async fn rotate_function_secret_with_value(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        value: FunctionSecretValue,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        self.rotate_function_secret_value(actor, tenant, name, &value, now_unix_seconds)
            .await
    }

    async fn rotate_function_secret_value(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        value: &FunctionSecretValue,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let previous = self.stored_secret(tenant, name).await?;
        if previous.metadata.state != FunctionSecretState::Active {
            return Err(CredentialAdminError::InvalidSecretState);
        }
        let mut metadata = previous.metadata.clone();
        metadata.version = metadata
            .version
            .checked_add(1)
            .ok_or(CredentialAdminError::VersionExhausted)?;
        metadata.rotated_at_unix_seconds = Some(now_unix_seconds);
        let next = self.encrypt_secret(metadata.clone(), value)?;
        self.rotate_secret(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionSecretRotate,
            name.as_str(),
            now_unix_seconds,
        );
        Ok(metadata)
    }

    pub async fn retire_function_secret(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        now_unix_seconds: u64,
    ) -> Result<FunctionSecretMetadata, CredentialAdminError> {
        let organization = self.authorize(actor, tenant, now_unix_seconds).await?;
        let previous = self.stored_secret(tenant, name).await?;
        let mut next = previous.clone();
        next.metadata.state = FunctionSecretState::Retired;
        next.metadata.retired_at_unix_seconds = Some(now_unix_seconds);
        self.replace_secret(&previous, &next).await?;
        self.audit(
            actor,
            &organization,
            tenant,
            ControlAuditAction::FunctionSecretRetire,
            name.as_str(),
            now_unix_seconds,
        );
        Ok(next.metadata)
    }

    pub async fn resolve_function_secret(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
    ) -> Result<FunctionSecretValue, CredentialAdminError> {
        let stored = self.stored_secret(tenant, name).await?;
        if stored.metadata.state != FunctionSecretState::Active {
            return Err(CredentialAdminError::InvalidSecretState);
        }
        self.decrypt_secret(&stored)
    }

    pub async fn resolve_active_function_secret(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
    ) -> Result<ResolvedFunctionSecret, CredentialAdminError> {
        let stored = self.stored_secret(tenant, name).await?;
        if stored.metadata.state != FunctionSecretState::Active {
            return Err(CredentialAdminError::InvalidSecretState);
        }
        Ok(ResolvedFunctionSecret {
            metadata: stored.metadata.clone(),
            value: self.decrypt_secret(&stored)?,
        })
    }

    pub async fn resolve_function_secret_version(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        version: u64,
    ) -> Result<ResolvedFunctionSecret, CredentialAdminError> {
        let stored = self.stored_secret_version(tenant, name, version).await?;
        Ok(ResolvedFunctionSecret {
            metadata: stored.metadata.clone(),
            value: self.decrypt_secret(&stored)?,
        })
    }

    fn identity(&self, tenant: &TenantScope) -> Result<IdentityStore, CredentialAdminError> {
        Ok(IdentityStore::new(
            self.adapter.clone(),
            tenant,
            tenant,
            self.durability,
        )?)
    }

    async fn load_ring(
        &self,
        tenant: &TenantScope,
    ) -> Result<(Option<Vec<u8>>, ProjectSigningKeyRing), CredentialAdminError> {
        let key =
            ControlKeyspace::signing_key_ring_key(tenant.project_id(), tenant.environment_id())?;
        let stored = self.adapter.get(&key).await?;
        let records = stored
            .as_deref()
            .map(serde_json::from_slice::<Vec<ProjectSigningKeyRecord>>)
            .transpose()?
            .unwrap_or_default();
        Ok((
            stored,
            ProjectSigningKeyRing::from_records(tenant.clone(), records)?,
        ))
    }

    async fn save_ring(
        &self,
        tenant: &TenantScope,
        previous: Option<Vec<u8>>,
        ring: &ProjectSigningKeyRing,
    ) -> Result<(), CredentialAdminError> {
        let key =
            ControlKeyspace::signing_key_ring_key(tenant.project_id(), tenant.environment_id())?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(ring.records())?);
        let condition = previous.map_or_else(
            || KeyCondition::Missing { key: key.clone() },
            |value| KeyCondition::ValueEquals {
                key: key.clone(),
                value,
            },
        );
        self.apply(AtomicWrite {
            conditions: vec![condition],
            batch,
            durability: self.durability,
        })
        .await
    }

    fn encrypt_secret(
        &self,
        metadata: FunctionSecretMetadata,
        value: &FunctionSecretValue,
    ) -> Result<StoredFunctionSecret, CredentialAdminError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(&self.function_encryption_key.0)
            .map_err(|_| CredentialAdminError::Encryption)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: value.0.as_bytes(),
                    aad: &secret_aad(&metadata),
                },
            )
            .map_err(|_| CredentialAdminError::Encryption)?;
        Ok(StoredFunctionSecret {
            metadata,
            ciphertext,
            nonce,
        })
    }

    fn decrypt_secret(
        &self,
        stored: &StoredFunctionSecret,
    ) -> Result<FunctionSecretValue, CredentialAdminError> {
        decrypt_secret(&self.function_encryption_key, stored)
    }

    async fn stored_secret(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
    ) -> Result<StoredFunctionSecret, CredentialAdminError> {
        let key = ControlKeyspace::function_secret_key(
            tenant.project_id(),
            tenant.environment_id(),
            name,
        )?;
        let stored = self
            .adapter
            .get(&key)
            .await?
            .ok_or(CredentialAdminError::NotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<StoredFunctionSecret>(&bytes).map_err(Into::into)
            })?;
        if stored.metadata.tenant != *tenant || stored.metadata.name != *name {
            return Err(CredentialAdminError::CorruptRecord);
        }
        Ok(stored)
    }

    async fn stored_secret_version(
        &self,
        tenant: &TenantScope,
        name: &FunctionSecretName,
        version: u64,
    ) -> Result<StoredFunctionSecret, CredentialAdminError> {
        let key = ControlKeyspace::function_secret_version_key(
            tenant.project_id(),
            tenant.environment_id(),
            name,
            version,
        )?;
        let stored = self
            .adapter
            .get(&key)
            .await?
            .ok_or(CredentialAdminError::NotFound)
            .and_then(|bytes| {
                serde_json::from_slice::<StoredFunctionSecret>(&bytes).map_err(Into::into)
            })?;
        if stored.metadata.tenant != *tenant
            || stored.metadata.name != *name
            || stored.metadata.version != version
        {
            return Err(CredentialAdminError::CorruptRecord);
        }
        Ok(stored)
    }

    async fn rotate_secret(
        &self,
        previous: &StoredFunctionSecret,
        next: &StoredFunctionSecret,
    ) -> Result<(), CredentialAdminError> {
        let key = secret_key(&previous.metadata)?;
        let version_key = secret_version_key(&next.metadata)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        batch.put(&version_key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![
                KeyCondition::ValueEquals {
                    key,
                    value: serde_json::to_vec(previous)?,
                },
                KeyCondition::Missing { key: version_key },
            ],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn replace_secret(
        &self,
        previous: &StoredFunctionSecret,
        next: &StoredFunctionSecret,
    ) -> Result<(), CredentialAdminError> {
        let key = secret_key(&previous.metadata)?;
        let mut batch = WriteBatch::new();
        batch.put(&key, serde_json::to_vec(next)?);
        self.apply(AtomicWrite {
            conditions: vec![KeyCondition::ValueEquals {
                key,
                value: serde_json::to_vec(previous)?,
            }],
            batch,
            durability: self.durability,
        })
        .await
    }

    async fn apply(&self, write: AtomicWrite) -> Result<(), CredentialAdminError> {
        match self.adapter.compare_and_write(write).await? {
            CompareAndWriteResult::Applied => Ok(()),
            CompareAndWriteResult::Conflict { .. } => Err(CredentialAdminError::Conflict),
        }
    }

    async fn authorize(
        &self,
        actor: &DeveloperPrincipal,
        tenant: &TenantScope,
        now_unix_seconds: u64,
    ) -> Result<OrganizationId, CredentialAdminError> {
        let project = self
            .projects
            .get_project(tenant.project_id())
            .await?
            .ok_or(CredentialAdminError::NotFound)?;
        self.projects
            .get_environment(tenant.project_id(), tenant.environment_id())
            .await?
            .ok_or(CredentialAdminError::NotFound)?;
        let membership = self
            .organizations
            .get_membership(project.organization_id(), actor.identity_id())
            .await?;
        if membership.is_none_or(|membership| {
            !matches!(
                membership.role(),
                OrganizationRole::Owner | OrganizationRole::Administrator
            )
        }) {
            self.audit(
                actor,
                project.organization_id(),
                tenant,
                ControlAuditAction::CredentialRead,
                "authorization",
                now_unix_seconds,
            );
            return Err(CredentialAdminError::Forbidden);
        }
        Ok(project.organization_id().clone())
    }

    fn audit(
        &self,
        actor: &DeveloperPrincipal,
        organization: &OrganizationId,
        tenant: &TenantScope,
        action: ControlAuditAction,
        target: &str,
        at_unix_seconds: u64,
    ) {
        self.audit.record(ControlAuditEvent {
            organization_id: organization.clone(),
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
            at_unix_seconds,
        });
    }
}

fn generate_function_secret() -> FunctionSecretValue {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    FunctionSecretValue(format!("mako_fn.{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn secret_key(metadata: &FunctionSecretMetadata) -> Result<Vec<u8>, CredentialAdminError> {
    ControlKeyspace::function_secret_key(
        metadata.tenant.project_id(),
        metadata.tenant.environment_id(),
        &metadata.name,
    )
    .map_err(Into::into)
}

fn secret_version_key(metadata: &FunctionSecretMetadata) -> Result<Vec<u8>, CredentialAdminError> {
    ControlKeyspace::function_secret_version_key(
        metadata.tenant.project_id(),
        metadata.tenant.environment_id(),
        &metadata.name,
        metadata.version,
    )
    .map_err(Into::into)
}

fn secret_aad(metadata: &FunctionSecretMetadata) -> Vec<u8> {
    format!(
        "mako-function-secret\0{}\0{}\0{}\0{}",
        metadata.tenant.project_id().as_str(),
        metadata.tenant.environment_id().as_str(),
        metadata.name.as_str(),
        metadata.version
    )
    .into_bytes()
}

fn decrypt_secret(
    key: &FunctionSecretEncryptionKey,
    stored: &StoredFunctionSecret,
) -> Result<FunctionSecretValue, CredentialAdminError> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key.0).map_err(|_| CredentialAdminError::Decryption)?;
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&stored.nonce),
            Payload {
                msg: &stored.ciphertext,
                aad: &secret_aad(&stored.metadata),
            },
        )
        .map_err(|_| CredentialAdminError::Decryption)?;
    String::from_utf8(plaintext)
        .map(FunctionSecretValue)
        .map_err(|_| CredentialAdminError::Decryption)
}

#[derive(Debug)]
pub enum CredentialAdminError {
    UnsupportedDurability,
    NotFound,
    Forbidden,
    Conflict,
    CorruptRecord,
    InvalidSecretName,
    InvalidSecretValue,
    InvalidSecretState,
    VersionExhausted,
    Encryption,
    Decryption,
    Identity(IdentityStoreError),
    IdentityRecord(IdentityRecordError),
    Signing(SigningKeyError),
    Project(ProjectStoreError),
    Organization(OrganizationStoreError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for CredentialAdminError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedDurability => "credential durability is unsupported",
            Self::NotFound => "credential was not found",
            Self::Forbidden => "credential administration is forbidden",
            Self::Conflict => "credential changed concurrently",
            Self::CorruptRecord => "credential record is corrupt",
            Self::InvalidSecretName => "function secret name is invalid",
            Self::InvalidSecretValue => "function secret value is invalid",
            Self::InvalidSecretState => "function secret state is invalid",
            Self::VersionExhausted => "function secret version is exhausted",
            Self::Encryption => "function secret encryption failed",
            Self::Decryption => "function secret decryption failed",
            Self::Identity(_) | Self::IdentityRecord(_) => "project credential operation failed",
            Self::Signing(_) => "signing-key operation failed",
            Self::Project(_) => "credential project lookup failed",
            Self::Organization(_) => "credential organization lookup failed",
            Self::Keyspace(_) => "credential key is invalid",
            Self::Storage(_) => "credential storage operation failed",
            Self::Json(_) => "credential record is invalid",
        })
    }
}

impl Error for CredentialAdminError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for CredentialAdminError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        }
    };
}
error_from!(IdentityStoreError, Identity);
error_from!(IdentityRecordError, IdentityRecord);
error_from!(SigningKeyError, Signing);
error_from!(ProjectStoreError, Project);
error_from!(OrganizationStoreError, Organization);
error_from!(ControlKeyspaceError, Keyspace);
error_from!(StorageError, Storage);
error_from!(serde_json::Error, Json);

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_identity::{ProjectCredentialState, ServiceCredentialOperation};
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{EnvironmentRecord, MembershipRecord, OrganizationRecord, ProjectRecord};

    struct Audit;
    impl ControlAuditSink for Audit {
        fn record(&self, _: ControlAuditEvent) {}
    }

    async fn fixture() -> (CredentialAdminService, DeveloperPrincipal, TenantScope) {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let organizations =
            OrganizationStore::new(adapter.clone(), Durability::Memory).expect("organizations");
        let projects = ProjectStore::new(adapter.clone(), Durability::Memory).expect("projects");
        let organization = OrganizationId::parse("org_example00").expect("organization");
        let developer = crate::DeveloperIdentityId::parse("dev_example00").expect("developer");
        organizations
            .create_organization(
                &OrganizationRecord::new(organization.clone(), "Example", 1).expect("organization"),
                &MembershipRecord::new(
                    organization.clone(),
                    developer.clone(),
                    OrganizationRole::Owner,
                    1,
                ),
            )
            .await
            .expect("organization");
        let project = ProjectId::parse("prj_example00").expect("project");
        let environment = EnvironmentId::parse("env_example00").expect("environment");
        projects
            .create_project(
                &ProjectRecord::new(project.clone(), organization, "Mako", "local", 1)
                    .expect("project"),
            )
            .await
            .expect("project");
        projects
            .create_environment(
                &EnvironmentRecord::new(environment.clone(), project.clone(), "Development", 1)
                    .expect("environment"),
            )
            .await
            .expect("environment");
        let service = CredentialAdminService::new(
            adapter,
            Durability::Memory,
            projects,
            organizations,
            Arc::new(Audit),
            KeyEncryptionKey::generate(),
            FunctionSecretEncryptionKey::generate(),
        )
        .expect("service");
        (
            service,
            DeveloperPrincipal::for_test(developer, "owner@example.test"),
            TenantScope::new(project, environment),
        )
    }

    #[test]
    fn credentials_keys_and_function_secrets_rotate_without_persistent_plaintext() {
        futures::executor::block_on(async {
            let (service, actor, tenant) = fixture().await;
            let public = service
                .create_public_key(
                    &actor,
                    &tenant,
                    ProjectCredentialId::parse("public_initial").expect("id"),
                    2,
                )
                .await
                .expect("public");
            assert_eq!(
                format!("{:?}", public.credential),
                "ProjectCredential([REDACTED])"
            );
            let rotated = service
                .rotate_credential(
                    &actor,
                    &tenant,
                    public.metadata.id(),
                    ProjectCredentialId::parse("public_next").expect("id"),
                    30,
                    3,
                )
                .await
                .expect("rotate");
            assert_eq!(rotated.metadata.state(), ProjectCredentialState::Active);
            assert_eq!(
                service
                    .credential_metadata(&actor, &tenant, public.metadata.id(), 4)
                    .await
                    .expect("old")
                    .state(),
                ProjectCredentialState::Overlap
            );
            service
                .create_service_credential(
                    &actor,
                    &tenant,
                    ProjectCredentialId::parse("service_backend").expect("id"),
                    ServiceCredentialScope::new(
                        ["todos".to_owned()],
                        [ServiceCredentialOperation::Read],
                    )
                    .expect("scope"),
                    5,
                )
                .await
                .expect("service credential");

            service
                .initialize_signing_key(&actor, &tenant, 6)
                .await
                .expect("signing key");
            service
                .rotate_signing_key(&actor, &tenant, 30, 7)
                .await
                .expect("rotate signing key");
            assert_eq!(
                service
                    .signing_keys(&actor, &tenant, 8)
                    .await
                    .expect("keys")
                    .len(),
                2
            );

            let name = FunctionSecretName::parse("API_TOKEN").expect("name");
            let first = service
                .create_function_secret(&actor, &tenant, name.clone(), 9)
                .await
                .expect("secret");
            assert_eq!(
                format!("{:?}", first.value),
                "FunctionSecretValue([REDACTED])"
            );
            let second = service
                .rotate_function_secret(&actor, &tenant, &name, 10)
                .await
                .expect("rotate secret");
            assert_ne!(first.value.expose_once(), second.value.expose_once());
            assert_eq!(
                service
                    .resolve_function_secret_version(&tenant, &name, 1)
                    .await
                    .expect("resolve first version")
                    .value
                    .expose_for_runtime(),
                first.value.expose_once()
            );
            assert_eq!(
                service
                    .resolve_function_secret_version(&tenant, &name, 2)
                    .await
                    .expect("resolve second version")
                    .value
                    .expose_for_runtime(),
                second.value.expose_once()
            );
            assert_eq!(
                service
                    .resolve_function_secret(&tenant, &name)
                    .await
                    .expect("resolve")
                    .expose_for_runtime(),
                second.value.expose_once()
            );
            assert_eq!(
                service
                    .function_secret_metadata(&actor, &tenant, &name, 11)
                    .await
                    .expect("metadata")
                    .version(),
                2
            );
            service
                .retire_function_secret(&actor, &tenant, &name, 12)
                .await
                .expect("retire");
            assert!(
                service
                    .resolve_function_secret(&tenant, &name)
                    .await
                    .is_err()
            );
            assert_eq!(
                service
                    .resolve_function_secret_version(&tenant, &name, 2)
                    .await
                    .expect("deployed version survives retirement")
                    .value
                    .expose_for_runtime(),
                second.value.expose_once()
            );

            let service_key_name = FunctionSecretName::parse("MAKO_SERVICE_KEY").expect("name");
            let service_key_value = "mako_sk.service_edge.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            let metadata = service
                .create_function_secret_with_value(
                    &actor,
                    &tenant,
                    service_key_name.clone(),
                    FunctionSecretValue::parse(service_key_value).expect("service key value"),
                    13,
                )
                .await
                .expect("store attached service key");
            assert_eq!(metadata.version(), 1);
            assert_eq!(
                service
                    .resolve_active_function_secret(&tenant, &service_key_name)
                    .await
                    .expect("resolve attached service key")
                    .value
                    .expose_for_runtime(),
                service_key_value
            );
            assert!(
                !serde_json::to_string(&metadata)
                    .expect("metadata")
                    .contains(service_key_value)
            );
        });
    }
}
