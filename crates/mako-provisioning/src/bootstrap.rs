use std::{collections::BTreeMap, fmt, sync::Arc};

use async_trait::async_trait;
use mako_storage::{Durability, KvAdapter, TenantKeyspace, WriteBatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::{
    ProvisioningBackend, ProvisioningComponent, ProvisioningFailure, ProvisioningOperation,
    ProvisioningResource,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisioningBackendConfig {
    pub management_origin: Url,
    pub data_origin: Url,
    pub function_origin: Url,
    pub default_quotas: BTreeMap<String, u64>,
}

impl ProvisioningBackendConfig {
    pub fn new(
        management_origin: &str,
        data_origin: &str,
        function_origin: &str,
        default_quotas: BTreeMap<String, u64>,
    ) -> Result<Self, ProvisioningFailure> {
        let management_origin = secure_origin(management_origin)?;
        let data_origin = secure_origin(data_origin)?;
        let function_origin = secure_origin(function_origin)?;
        if default_quotas.is_empty() || default_quotas.values().any(|value| *value == 0) {
            return Err(failure(
                "invalid_quota_defaults",
                "default quotas must contain positive values",
                false,
            ));
        }
        Ok(Self {
            management_origin,
            data_origin,
            function_origin,
            default_quotas,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProvisionedEndpointSet {
    pub management: String,
    pub auth: String,
    pub replication: String,
    pub functions: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisionedResourceState {
    Active,
    Suspended,
    Deleted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ActivationRecord {
    state: ProvisionedResourceState,
    ready_components: Vec<ProvisioningComponent>,
    endpoints: Option<ProvisionedEndpointSet>,
}

#[derive(Clone)]
pub struct KvProvisioningBackend {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
    config: ProvisioningBackendConfig,
}

impl fmt::Debug for KvProvisioningBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KvProvisioningBackend")
            .field("durability", &self.durability)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl KvProvisioningBackend {
    pub fn new(
        adapter: Arc<dyn KvAdapter>,
        durability: Durability,
        config: ProvisioningBackendConfig,
    ) -> Result<Self, ProvisioningFailure> {
        if adapter.capabilities().strongest_durability < durability {
            return Err(failure(
                "unsupported_durability",
                "storage cannot satisfy provisioning durability",
                false,
            ));
        }
        Ok(Self {
            adapter,
            durability,
            config,
        })
    }

    pub async fn exposed_endpoints(
        &self,
        resource: &ProvisioningResource,
    ) -> Result<Option<ProvisionedEndpointSet>, ProvisioningFailure> {
        let Some(record) = self.activation(resource).await? else {
            return Ok(None);
        };
        if record.state != ProvisionedResourceState::Active
            || record.ready_components.len() != ProvisioningComponent::ORDERED.len()
        {
            return Ok(None);
        }
        Ok(record.endpoints)
    }

    pub async fn state(
        &self,
        resource: &ProvisioningResource,
    ) -> Result<Option<ProvisionedResourceState>, ProvisioningFailure> {
        Ok(self.activation(resource).await?.map(|record| record.state))
    }

    async fn provision_component(
        &self,
        resource: &ProvisioningResource,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure> {
        let key = component_key(resource, component)?;
        let value = serde_json::to_vec(&component_configuration(component, &self.config))
            .map_err(serialization_failure)?;
        if let Some(existing) = self.adapter.get(&key).await.map_err(storage_failure)? {
            if existing == value {
                return Ok(());
            }
            return Err(failure(
                "component_configuration_conflict",
                "provisioned component configuration differs from the requested defaults",
                false,
            ));
        }
        let mut batch = WriteBatch::new();
        batch.put(key, value);
        self.adapter
            .write(batch, self.durability)
            .await
            .map_err(storage_failure)
    }

    async fn remove_component(
        &self,
        resource: &ProvisioningResource,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure> {
        let mut batch = WriteBatch::with_capacity(2);
        batch.delete(component_key(resource, component)?);
        batch.delete(activation_key(resource)?);
        self.adapter
            .write(batch, self.durability)
            .await
            .map_err(storage_failure)
    }

    async fn activate(&self, resource: &ProvisioningResource) -> Result<(), ProvisioningFailure> {
        let ready = self.ready_components(resource).await?;
        if ready.len() != ProvisioningComponent::ORDERED.len() {
            return Err(failure(
                "resource_not_ready",
                "required provisioning components are not ready",
                true,
            ));
        }
        self.write_activation(
            resource,
            ActivationRecord {
                state: ProvisionedResourceState::Active,
                ready_components: ready,
                endpoints: Some(self.endpoints(resource)?),
            },
        )
        .await
    }

    async fn write_state(
        &self,
        resource: &ProvisioningResource,
        state: ProvisionedResourceState,
    ) -> Result<(), ProvisioningFailure> {
        let ready_components = self.ready_components(resource).await?;
        self.write_activation(
            resource,
            ActivationRecord {
                state,
                ready_components,
                endpoints: None,
            },
        )
        .await
    }

    async fn write_activation(
        &self,
        resource: &ProvisioningResource,
        record: ActivationRecord,
    ) -> Result<(), ProvisioningFailure> {
        let mut batch = WriteBatch::new();
        batch.put(
            activation_key(resource)?,
            serde_json::to_vec(&record).map_err(serialization_failure)?,
        );
        self.adapter
            .write(batch, self.durability)
            .await
            .map_err(storage_failure)
    }

    async fn activation(
        &self,
        resource: &ProvisioningResource,
    ) -> Result<Option<ActivationRecord>, ProvisioningFailure> {
        self.adapter
            .get(&activation_key(resource)?)
            .await
            .map_err(storage_failure)?
            .map(|value| serde_json::from_slice(&value).map_err(serialization_failure))
            .transpose()
    }

    async fn ready_components(
        &self,
        resource: &ProvisioningResource,
    ) -> Result<Vec<ProvisioningComponent>, ProvisioningFailure> {
        let mut ready = Vec::new();
        for component in ProvisioningComponent::ORDERED {
            if self
                .adapter
                .get(&component_key(resource, component)?)
                .await
                .map_err(storage_failure)?
                .is_some()
            {
                ready.push(component);
            }
        }
        Ok(ready)
    }

    fn endpoints(
        &self,
        resource: &ProvisioningResource,
    ) -> Result<ProvisionedEndpointSet, ProvisioningFailure> {
        let path = resource_path(resource);
        Ok(ProvisionedEndpointSet {
            management: self
                .config
                .management_origin
                .join(&format!("v1/{path}"))
                .map_err(endpoint_failure)?
                .to_string(),
            auth: self
                .config
                .data_origin
                .join(&format!("v1/{path}/auth/"))
                .map_err(endpoint_failure)?
                .to_string(),
            replication: self
                .config
                .data_origin
                .join(&format!("v1/{path}/collections/"))
                .map_err(endpoint_failure)?
                .to_string(),
            functions: self
                .config
                .function_origin
                .join(&format!("{path}/functions/v1/"))
                .map_err(endpoint_failure)?
                .to_string(),
        })
    }
}

#[async_trait]
impl ProvisioningBackend for KvProvisioningBackend {
    async fn apply_step(
        &self,
        resource: &ProvisioningResource,
        operation: ProvisioningOperation,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure> {
        match operation {
            ProvisioningOperation::Create | ProvisioningOperation::Restore => {
                self.provision_component(resource, component).await?;
                if component == ProvisioningComponent::Observability {
                    self.activate(resource).await?;
                }
            }
            ProvisioningOperation::Suspend => {
                if component == ProvisioningComponent::Observability {
                    self.write_state(resource, ProvisionedResourceState::Suspended)
                        .await?;
                }
            }
            ProvisioningOperation::Delete => {
                self.remove_component(resource, component).await?;
                if component == ProvisioningComponent::Observability {
                    self.write_state(resource, ProvisionedResourceState::Deleted)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn compensate_step(
        &self,
        resource: &ProvisioningResource,
        operation: ProvisioningOperation,
        component: ProvisioningComponent,
    ) -> Result<(), ProvisioningFailure> {
        match operation {
            ProvisioningOperation::Create | ProvisioningOperation::Restore => {
                self.remove_component(resource, component).await
            }
            ProvisioningOperation::Suspend => {
                if component == ProvisioningComponent::Observability {
                    self.activate(resource).await?;
                }
                Ok(())
            }
            ProvisioningOperation::Delete => {
                self.provision_component(resource, component).await?;
                if self.ready_components(resource).await?.len()
                    == ProvisioningComponent::ORDERED.len()
                {
                    self.activate(resource).await?;
                }
                Ok(())
            }
        }
    }
}

fn component_configuration(
    component: ProvisioningComponent,
    config: &ProvisioningBackendConfig,
) -> Value {
    match component {
        ProvisioningComponent::Storage => json!({
            "namespace": "allocated",
            "durability": "adapter_required"
        }),
        ProvisioningComponent::Identity => json!({
            "signingKeySet": "created",
            "projectCredentials": "created",
            "rawSecretsStored": false
        }),
        ProvisioningComponent::Policy => json!({
            "mode": "default_deny",
            "authorizationEpoch": 1
        }),
        ProvisioningComponent::Replication => json!({
            "routes": "registered",
            "checkpointSigning": "enabled"
        }),
        ProvisioningComponent::Functions => json!({
            "metadataNamespace": "created",
            "activeDeployments": 0
        }),
        ProvisioningComponent::Quotas => json!({ "limits": config.default_quotas }),
        ProvisioningComponent::Observability => json!({
            "structuredLogs": true,
            "metrics": true,
            "audit": true
        }),
    }
}

fn component_key(
    resource: &ProvisioningResource,
    component: ProvisioningComponent,
) -> Result<Vec<u8>, ProvisioningFailure> {
    TenantKeyspace::system_key(
        format!("provisioning/resources/{}", resource_key(resource)),
        component_name(component),
    )
    .map_err(key_failure)
}

fn activation_key(resource: &ProvisioningResource) -> Result<Vec<u8>, ProvisioningFailure> {
    TenantKeyspace::system_key(
        format!("provisioning/resources/{}", resource_key(resource)),
        "activation",
    )
    .map_err(key_failure)
}

fn resource_key(resource: &ProvisioningResource) -> String {
    match resource {
        ProvisioningResource::Project(project) => format!("project/{}", project.as_str()),
        ProvisioningResource::Environment(scope) => format!(
            "environment/{}/{}",
            scope.project_id().as_str(),
            scope.environment_id().as_str()
        ),
    }
}

fn resource_path(resource: &ProvisioningResource) -> String {
    match resource {
        ProvisioningResource::Project(project) => format!("projects/{}", project.as_str()),
        ProvisioningResource::Environment(scope) => format!(
            "projects/{}/environments/{}",
            scope.project_id().as_str(),
            scope.environment_id().as_str()
        ),
    }
}

const fn component_name(component: ProvisioningComponent) -> &'static str {
    match component {
        ProvisioningComponent::Storage => "storage",
        ProvisioningComponent::Identity => "identity",
        ProvisioningComponent::Policy => "policy",
        ProvisioningComponent::Replication => "replication",
        ProvisioningComponent::Functions => "functions",
        ProvisioningComponent::Quotas => "quotas",
        ProvisioningComponent::Observability => "observability",
    }
}

fn secure_origin(value: &str) -> Result<Url, ProvisioningFailure> {
    let mut url = Url::parse(value).map_err(endpoint_failure)?;
    let local_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if url.scheme() != "https" && !local_http {
        return Err(endpoint_failure(()));
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn failure(code: &str, diagnostic: &str, retryable: bool) -> ProvisioningFailure {
    ProvisioningFailure::new(code, diagnostic, retryable).expect("static failure is valid")
}

fn storage_failure(_error: mako_storage::StorageError) -> ProvisioningFailure {
    failure(
        "storage_unavailable",
        "provisioning storage operation failed",
        true,
    )
}

fn key_failure(_error: mako_storage::KeyCodecError) -> ProvisioningFailure {
    failure(
        "invalid_resource_key",
        "provisioning resource key could not be encoded",
        false,
    )
}

fn serialization_failure(_error: serde_json::Error) -> ProvisioningFailure {
    failure(
        "invalid_resource_record",
        "provisioning resource metadata is invalid",
        false,
    )
}

fn endpoint_failure<T>(_error: T) -> ProvisioningFailure {
    failure(
        "invalid_endpoint_configuration",
        "hosted endpoint configuration is invalid",
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mako_api::TenantScope;
    use mako_storage::MemoryAdapter;

    use super::*;
    use crate::{
        Provisioner, ProvisioningOperation, ProvisioningState, ProvisioningStore,
        ProvisioningWorkflowId,
    };

    fn config() -> ProvisioningBackendConfig {
        ProvisioningBackendConfig::new(
            "https://api.example.test",
            "https://data.example.test",
            "https://functions.example.test",
            BTreeMap::from([
                ("storage_bytes".to_owned(), 1_000_000),
                ("replication_requests_per_minute".to_owned(), 1_000),
            ]),
        )
        .expect("config")
    }

    #[test]
    fn activates_only_after_every_resource_is_ready_and_hides_suspended_endpoints() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let backend = KvProvisioningBackend::new(adapter.clone(), Durability::Memory, config())
                .expect("backend");
            let provisioner = Provisioner::new(
                ProvisioningStore::new(adapter, Durability::Memory).expect("store"),
            );
            let resource = ProvisioningResource::Environment(
                TenantScope::require(Some("prj_example00"), Some("env_example00")).expect("scope"),
            );
            assert!(
                backend
                    .exposed_endpoints(&resource)
                    .await
                    .expect("endpoints")
                    .is_none()
            );
            let create_id = ProvisioningWorkflowId::parse("wf_bootstrap00").expect("workflow id");
            provisioner
                .enqueue(
                    create_id.clone(),
                    resource.clone(),
                    ProvisioningOperation::Create,
                    10,
                )
                .await
                .expect("enqueue");
            let active = provisioner
                .run(&create_id, &backend, 11)
                .await
                .expect("provision");
            assert_eq!(active.state(), ProvisioningState::Active);
            let endpoints = backend
                .exposed_endpoints(&resource)
                .await
                .expect("endpoints")
                .expect("active endpoints");
            assert!(endpoints.auth.contains("env_example00"));

            let suspend_id = ProvisioningWorkflowId::parse("wf_suspend000").expect("workflow id");
            provisioner
                .enqueue(
                    suspend_id.clone(),
                    resource.clone(),
                    ProvisioningOperation::Suspend,
                    12,
                )
                .await
                .expect("enqueue suspend");
            provisioner
                .run(&suspend_id, &backend, 13)
                .await
                .expect("suspend");
            assert_eq!(
                backend.state(&resource).await.expect("state"),
                Some(ProvisionedResourceState::Suspended)
            );
            assert!(
                backend
                    .exposed_endpoints(&resource)
                    .await
                    .expect("endpoints")
                    .is_none()
            );
        });
    }

    #[test]
    fn default_policy_and_secret_metadata_are_safe_and_idempotent() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let backend = KvProvisioningBackend::new(adapter.clone(), Durability::Memory, config())
                .expect("backend");
            let resource = ProvisioningResource::Project(
                mako_api::ProjectId::parse("prj_example00").expect("project"),
            );
            backend
                .apply_step(
                    &resource,
                    ProvisioningOperation::Create,
                    ProvisioningComponent::Policy,
                )
                .await
                .expect("policy");
            backend
                .apply_step(
                    &resource,
                    ProvisioningOperation::Create,
                    ProvisioningComponent::Policy,
                )
                .await
                .expect("idempotent policy");
            let policy = adapter
                .get(&component_key(&resource, ProvisioningComponent::Policy).expect("policy key"))
                .await
                .expect("read")
                .expect("policy record");
            assert_eq!(
                serde_json::from_slice::<Value>(&policy).expect("json")["mode"],
                "default_deny"
            );
            let identity = component_configuration(ProvisioningComponent::Identity, &config());
            assert_eq!(identity["rawSecretsStored"], false);
        });
    }
}
