use std::{error::Error, fmt, sync::Arc};

use mako_api::TenantScope;
use mako_control_plane::{
    ControlKeyspace, ControlKeyspaceError, CredentialAdminError, FunctionAdminError,
    FunctionConfiguration, FunctionName, FunctionRecord, FunctionSecretEncryptionKey,
    FunctionSecretName, FunctionSecretResolver, FunctionSecretValue, FunctionState,
    FunctionVersionRecord, FunctionVersionState,
};
use mako_storage::{KvAdapter, StorageError};

#[derive(Clone)]
pub struct FunctionResolutionService {
    adapter: Arc<dyn KvAdapter>,
    secrets: FunctionSecretResolver,
}

impl fmt::Debug for FunctionResolutionService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionResolutionService")
            .finish_non_exhaustive()
    }
}

impl FunctionResolutionService {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        secret_key: FunctionSecretEncryptionKey,
    ) -> Result<Self, FunctionResolutionError> {
        let secrets = FunctionSecretResolver::new(Arc::clone(&adapter), secret_key)?;
        Ok(Self { adapter, secrets })
    }

    pub async fn resolve(
        &self,
        tenant: &TenantScope,
        function_name: &str,
        region: &str,
    ) -> Result<ResolvedFunctionConfiguration, FunctionResolutionError> {
        if region.is_empty()
            || region.len() > 64
            || region.trim() != region
            || region.chars().any(char::is_control)
        {
            return Err(FunctionResolutionError::InvalidRequest);
        }
        let name = FunctionName::parse(function_name)?;
        let function_key =
            ControlKeyspace::function_key(tenant.project_id(), tenant.environment_id(), &name)?;
        let function: FunctionRecord = self.read(&function_key).await?;
        if function.tenant() != tenant || function.name() != &name {
            return Err(FunctionResolutionError::CorruptRecord);
        }
        if function.state() != FunctionState::Active {
            return Err(FunctionResolutionError::Inactive);
        }
        if !function
            .configuration()
            .regions
            .iter()
            .any(|configured| configured == region)
        {
            return Err(FunctionResolutionError::RegionUnavailable);
        }
        let version = function
            .active_version()
            .ok_or(FunctionResolutionError::Inactive)?;
        let version_key = ControlKeyspace::function_version_key(
            tenant.project_id(),
            tenant.environment_id(),
            &name,
            version,
        )?;
        let deployed: FunctionVersionRecord = self.read(&version_key).await?;
        if deployed.tenant() != tenant
            || deployed.function_name() != &name
            || deployed.version() != version
        {
            return Err(FunctionResolutionError::CorruptRecord);
        }
        if deployed.state() != FunctionVersionState::Healthy {
            return Err(FunctionResolutionError::Inactive);
        }

        let mut secrets = Vec::with_capacity(deployed.secret_versions().len());
        for reference in deployed.secret_versions() {
            let resolved = self
                .secrets
                .resolve_version(tenant, &reference.name, reference.version)
                .await?;
            secrets.push(ResolvedFunctionSecretValue {
                name: reference.name.clone(),
                version: reference.version,
                value: resolved.value,
            });
        }

        Ok(ResolvedFunctionConfiguration {
            function_name: name,
            version,
            bundle_digest: deployed.bundle_digest().to_owned(),
            entrypoint: deployed.entrypoint().to_owned(),
            runtime_version: deployed.runtime_version().to_owned(),
            configuration: deployed.configuration().clone(),
            secrets,
        })
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<T, FunctionResolutionError> {
        self.adapter
            .get(key)
            .await?
            .ok_or(FunctionResolutionError::NotFound)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(Into::into))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFunctionConfiguration {
    pub function_name: FunctionName,
    pub version: u64,
    pub bundle_digest: String,
    pub entrypoint: String,
    pub runtime_version: String,
    pub configuration: FunctionConfiguration,
    pub secrets: Vec<ResolvedFunctionSecretValue>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedFunctionSecretValue {
    pub name: FunctionSecretName,
    pub version: u64,
    value: FunctionSecretValue,
}

impl ResolvedFunctionSecretValue {
    #[must_use]
    pub fn expose_to_runtime(&self) -> &str {
        self.value.expose_for_runtime()
    }
}

impl fmt::Debug for ResolvedFunctionSecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedFunctionSecretValue")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug)]
pub enum FunctionResolutionError {
    InvalidRequest,
    NotFound,
    Inactive,
    RegionUnavailable,
    CorruptRecord,
    Function(FunctionAdminError),
    Credential(CredentialAdminError),
    Keyspace(ControlKeyspaceError),
    Storage(StorageError),
    Json(serde_json::Error),
}

impl fmt::Display for FunctionResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "function resolution request is invalid",
            Self::NotFound => "function was not found",
            Self::Inactive => "function deployment is not active",
            Self::RegionUnavailable => "function is not deployed in this region",
            Self::CorruptRecord => "function deployment record is corrupt",
            Self::Function(_) => "function name is invalid",
            Self::Credential(_) => "function secret resolution failed",
            Self::Keyspace(_) => "function resolution key is invalid",
            Self::Storage(_) => "function resolution storage failed",
            Self::Json(_) => "function deployment record is invalid",
        })
    }
}

impl Error for FunctionResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Function(error) => Some(error),
            Self::Credential(error) => Some(error),
            Self::Keyspace(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<FunctionAdminError> for FunctionResolutionError {
    fn from(error: FunctionAdminError) -> Self {
        Self::Function(error)
    }
}

impl From<CredentialAdminError> for FunctionResolutionError {
    fn from(error: CredentialAdminError) -> Self {
        Self::Credential(error)
    }
}

impl From<ControlKeyspaceError> for FunctionResolutionError {
    fn from(error: ControlKeyspaceError) -> Self {
        Self::Keyspace(error)
    }
}

impl From<StorageError> for FunctionResolutionError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for FunctionResolutionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
