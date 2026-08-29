//! Mail the data plane wants sent to application users -- magic links,
//! verification, recovery, invitations -- held here until the control plane,
//! which owns the only mail transport, takes them.
//!
//! An intent is written durably at request time and stays until the control
//! plane has leased it, stored it in its own outbox, and acknowledged it. A
//! lease that expires without an acknowledgement makes the intent visible
//! again, so delivery is at least once and the control plane's outbox
//! deduplicates by intent id.
use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};

use futures::executor::block_on;
use mako_api::TenantScope;
use mako_internal_rpc::{
    ApplicationMailAcknowledgeRequest, ApplicationMailAcknowledgeResponse,
    ApplicationMailDrainRequest, ApplicationMailDrainResponse, ApplicationMailIntent,
    InternalCaller, InternalReplayGuard as _, InternalRoute, application_mail_scope,
};
use mako_service_runtime::{
    HttpApiError, HttpMethod, HttpRequest, HttpResponse, HttpRouter, RouteRegistrationError,
};
use mako_storage::{
    AtomicWrite, CompareAndWriteResult, Durability, KeyCondition, KvAdapter, ScanDirection,
    ScanRequest, TenantKeyspace, WriteBatch,
};
use serde::{Deserialize, Serialize};

use crate::{DataPlaneGraph, auth_http};

const DOMAIN: &[u8] = b"mako:application-mail:v1";
const MAX_DRAIN: u64 = 200;
const MAX_LEASE_SECONDS: u64 = 600;
const MAX_VARIABLE_BYTES: usize = 4096;
const MAX_VARIABLES: usize = 16;

pub const KIND_VERIFICATION: &str = "verification";
pub const KIND_RECOVERY: &str = "recovery";
pub const KIND_INVITATION: &str = "invitation";
pub const KIND_MAGIC_LINK: &str = "magic_link";

/// What an intent looks like at rest: the intent plus its lease.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredIntent {
    intent: ApplicationMailIntent,
    leased_until_unix_seconds: Option<u64>,
    drains: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationMailError {
    Invalid(&'static str),
    Storage,
}

/// The node-wide outbox of application mail intents.
#[derive(Clone)]
pub struct ApplicationMailOutbox {
    adapter: Arc<dyn KvAdapter>,
    durability: Durability,
}

impl std::fmt::Debug for ApplicationMailOutbox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ApplicationMailOutbox")
    }
}

impl ApplicationMailOutbox {
    #[must_use]
    pub fn new(adapter: Arc<dyn KvAdapter>, durability: Durability) -> Self {
        Self {
            adapter,
            durability,
        }
    }

    /// Composes and writes an intent; the id is derived from the tenant and a
    /// caller-chosen token so a retried request never enqueues twice.
    pub async fn enqueue(
        &self,
        tenant: &TenantScope,
        kind: &str,
        recipient: &str,
        variables: BTreeMap<String, String>,
        dedupe_token: &str,
        now_unix_seconds: u64,
    ) -> Result<ApplicationMailIntent, ApplicationMailError> {
        if !matches!(
            kind,
            KIND_VERIFICATION | KIND_RECOVERY | KIND_INVITATION | KIND_MAGIC_LINK
        ) {
            return Err(ApplicationMailError::Invalid("unknown mail kind"));
        }
        if recipient.is_empty() || recipient.len() > 254 || recipient.chars().any(char::is_control)
        {
            return Err(ApplicationMailError::Invalid("recipient is invalid"));
        }
        if variables.len() > MAX_VARIABLES
            || variables.iter().any(|(name, value)| {
                name.is_empty()
                    || name.len() > 64
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                    || value.len() > MAX_VARIABLE_BYTES
                    || value
                        .chars()
                        .any(|character| character.is_control() && character != '\n')
            })
        {
            return Err(ApplicationMailError::Invalid("variables are invalid"));
        }
        let id = format!(
            "aml_{}",
            &blake3::hash(
                format!(
                    "{}\0{}\0{kind}\0{dedupe_token}",
                    tenant.project_id(),
                    tenant.environment_id()
                )
                .as_bytes()
            )
            .to_hex()[..32]
        );
        let intent = ApplicationMailIntent {
            id: id.clone(),
            project_id: tenant.project_id().as_str().to_owned(),
            environment_id: tenant.environment_id().as_str().to_owned(),
            kind: kind.to_owned(),
            recipient: recipient.to_owned(),
            variables,
            created_at_unix_seconds: now_unix_seconds,
        };
        let key =
            TenantKeyspace::system_key(DOMAIN, &id).map_err(|_| ApplicationMailError::Storage)?;
        let stored = StoredIntent {
            intent: intent.clone(),
            leased_until_unix_seconds: None,
            drains: 0,
        };
        let mut batch = WriteBatch::new();
        batch.put(
            &key,
            serde_json::to_vec(&stored).map_err(|_| ApplicationMailError::Storage)?,
        );
        match self
            .adapter
            .compare_and_write(AtomicWrite {
                conditions: vec![KeyCondition::Missing { key }],
                batch,
                durability: self.durability,
            })
            .await
            .map_err(|_| ApplicationMailError::Storage)?
        {
            // Already queued by an earlier attempt of the same request: fine.
            CompareAndWriteResult::Applied | CompareAndWriteResult::Conflict { .. } => Ok(intent),
        }
    }

    /// Leases up to `limit` unleased (or lease-expired) intents.
    pub async fn drain(
        &self,
        now_unix_seconds: u64,
        lease_seconds: u64,
        limit: u64,
    ) -> Result<Vec<ApplicationMailIntent>, ApplicationMailError> {
        let lease_seconds = lease_seconds.clamp(1, MAX_LEASE_SECONDS);
        let limit = usize::try_from(limit.clamp(1, MAX_DRAIN)).unwrap_or(1);
        let range = TenantKeyspace::system_domain_range(DOMAIN)
            .map_err(|_| ApplicationMailError::Storage)?;
        let entries = self
            .adapter
            .scan(ScanRequest {
                range,
                direction: ScanDirection::Forward,
                limit: NonZeroUsize::new(limit.saturating_mul(4).max(limit)).expect("limit"),
            })
            .await
            .map_err(|_| ApplicationMailError::Storage)?;
        let mut leased = Vec::new();
        for entry in entries {
            if leased.len() >= limit {
                break;
            }
            let stored: StoredIntent = match serde_json::from_slice(&entry.value) {
                Ok(stored) => stored,
                Err(_) => continue,
            };
            if stored
                .leased_until_unix_seconds
                .is_some_and(|until| until > now_unix_seconds)
            {
                continue;
            }
            let next = StoredIntent {
                intent: stored.intent.clone(),
                leased_until_unix_seconds: Some(now_unix_seconds.saturating_add(lease_seconds)),
                drains: stored.drains.saturating_add(1),
            };
            let mut batch = WriteBatch::new();
            batch.put(
                &entry.key,
                serde_json::to_vec(&next).map_err(|_| ApplicationMailError::Storage)?,
            );
            let applied = self
                .adapter
                .compare_and_write(AtomicWrite {
                    conditions: vec![KeyCondition::ValueEquals {
                        key: entry.key.clone(),
                        value: entry.value.clone(),
                    }],
                    batch,
                    durability: self.durability,
                })
                .await
                .map_err(|_| ApplicationMailError::Storage)?;
            if matches!(applied, CompareAndWriteResult::Applied) {
                leased.push(stored.intent);
            }
        }
        Ok(leased)
    }

    /// Forgets intents the control plane holds durably.
    pub async fn acknowledge(&self, ids: &[String]) -> Result<u64, ApplicationMailError> {
        let mut acknowledged = 0;
        for id in ids.iter().take(usize::try_from(MAX_DRAIN).unwrap_or(200)) {
            let key = TenantKeyspace::system_key(DOMAIN, id)
                .map_err(|_| ApplicationMailError::Storage)?;
            let Some(current) = self
                .adapter
                .get(&key)
                .await
                .map_err(|_| ApplicationMailError::Storage)?
            else {
                continue;
            };
            let mut batch = WriteBatch::new();
            batch.delete(&key);
            if matches!(
                self.adapter
                    .compare_and_write(AtomicWrite {
                        conditions: vec![KeyCondition::ValueEquals {
                            key,
                            value: current,
                        }],
                        batch,
                        durability: self.durability,
                    })
                    .await
                    .map_err(|_| ApplicationMailError::Storage)?,
                CompareAndWriteResult::Applied
            ) {
                acknowledged += 1;
            }
        }
        Ok(acknowledged)
    }

    /// How many intents wait, for readiness and tests.
    pub async fn pending(&self) -> Result<u64, ApplicationMailError> {
        let range = TenantKeyspace::system_domain_range(DOMAIN)
            .map_err(|_| ApplicationMailError::Storage)?;
        self.adapter
            .count_keys(range)
            .await
            .map_err(|_| ApplicationMailError::Storage)
    }
}

// ---- internal routes -------------------------------------------------------

pub fn add_application_mail_routes(
    router: &mut HttpRouter,
    graph: Arc<DataPlaneGraph>,
) -> Result<(), RouteRegistrationError> {
    let drain_graph = Arc::clone(&graph);
    router.add_route(
        HttpMethod::Post,
        InternalRoute::ApplicationMailDrain.path(),
        move |request| handle_drain(&drain_graph, &request),
    )?;
    router.add_route(
        HttpMethod::Post,
        InternalRoute::ApplicationMailAcknowledge.path(),
        move |request| handle_acknowledge(&graph, &request),
    )
}

fn verify(
    graph: &DataPlaneGraph,
    route: InternalRoute,
    request: &HttpRequest,
    now: u64,
) -> Result<(), HttpApiError> {
    let verified = graph
        .internal_authenticator(InternalCaller::ControlPlane)
        .verify(route, request, now)
        .map_err(|_| auth_http::unauthenticated(request, "internal request is not authorized"))?;
    if verified.tenant != application_mail_scope() {
        return Err(auth_http::unauthenticated(
            request,
            "internal request scope is invalid",
        ));
    }
    block_on(async {
        let guard = graph
            .internal_replay_guard(&verified.tenant, &verified.tenant)
            .map_err(|_| auth_http::unavailable(request, "replay protection is unavailable"))?;
        guard
            .claim(&verified, now)
            .await
            .map_err(|_| auth_http::unavailable(request, "replay protection is unavailable"))
            .and_then(|decision| {
                if matches!(decision, mako_internal_rpc::GuardDecision::Duplicate) {
                    Err(auth_http::invalid(request, "internal request was replayed"))
                } else {
                    Ok(())
                }
            })
    })
}

fn handle_drain(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = auth_http::now_unix_seconds(request.request_id())?;
    verify(graph, InternalRoute::ApplicationMailDrain, request, now)?;
    let body: ApplicationMailDrainRequest = auth_http::parse_json(request)?;
    let intents = block_on(
        graph
            .application_mail()
            .drain(now, body.lease_seconds, body.limit),
    )
    .map_err(|_| auth_http::unavailable(request, "application mail is unavailable"))?;
    auth_http::json(request, 200, &ApplicationMailDrainResponse { intents })
}

fn handle_acknowledge(
    graph: &Arc<DataPlaneGraph>,
    request: &HttpRequest,
) -> Result<HttpResponse, HttpApiError> {
    let now = auth_http::now_unix_seconds(request.request_id())?;
    verify(
        graph,
        InternalRoute::ApplicationMailAcknowledge,
        request,
        now,
    )?;
    let body: ApplicationMailAcknowledgeRequest = auth_http::parse_json(request)?;
    let acknowledged = block_on(graph.application_mail().acknowledge(&body.ids))
        .map_err(|_| auth_http::unavailable(request, "application mail is unavailable"))?;
    auth_http::json(
        request,
        200,
        &ApplicationMailAcknowledgeResponse { acknowledged },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mako_api::{EnvironmentId, ProjectId};
    use mako_storage::MemoryAdapter;

    use super::*;

    fn tenant(environment: &str) -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_mailtest0").expect("project"),
            EnvironmentId::parse(environment).expect("environment"),
        )
    }

    #[test]
    fn intents_are_leased_once_per_window_and_forgotten_when_acknowledged() {
        futures::executor::block_on(async {
            let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
            let outbox = ApplicationMailOutbox::new(adapter, Durability::Memory);
            let variables = BTreeMap::from([("link".to_owned(), "https://app/x#t".to_owned())]);
            let first = outbox
                .enqueue(
                    &tenant("env_a0000001"),
                    KIND_MAGIC_LINK,
                    "a@app.test",
                    variables.clone(),
                    "req-1",
                    100,
                )
                .await
                .expect("enqueue");
            let again = outbox
                .enqueue(
                    &tenant("env_a0000001"),
                    KIND_MAGIC_LINK,
                    "a@app.test",
                    variables.clone(),
                    "req-1",
                    101,
                )
                .await
                .expect("retry");
            assert_eq!(first.id, again.id, "a retried request enqueues once");
            outbox
                .enqueue(
                    &tenant("env_b0000001"),
                    KIND_VERIFICATION,
                    "b@app.test",
                    variables.clone(),
                    "req-2",
                    100,
                )
                .await
                .expect("second tenant");
            assert_eq!(outbox.pending().await.expect("pending"), 2);

            let leased = outbox.drain(200, 30, 10).await.expect("drain");
            assert_eq!(leased.len(), 2, "both tenants drain in one call");
            assert!(
                outbox
                    .drain(210, 30, 10)
                    .await
                    .expect("drain again")
                    .is_empty(),
                "leased intents hide"
            );
            let redrained = outbox.drain(231, 30, 10).await.expect("after lease");
            assert_eq!(
                redrained.len(),
                2,
                "an expired lease makes them visible again"
            );

            let acknowledged = outbox
                .acknowledge(&[first.id.clone(), "aml_missing".to_owned()])
                .await
                .expect("ack");
            assert_eq!(acknowledged, 1);
            assert_eq!(outbox.pending().await.expect("pending"), 1);
            assert!(matches!(
                outbox
                    .enqueue(
                        &tenant("env_a0000001"),
                        "newsletter",
                        "a@app.test",
                        BTreeMap::new(),
                        "x",
                        1
                    )
                    .await,
                Err(ApplicationMailError::Invalid(_))
            ));
            assert!(matches!(
                outbox
                    .enqueue(
                        &tenant("env_a0000001"),
                        KIND_RECOVERY,
                        "a@app.test",
                        BTreeMap::from([("Bad-Name".to_owned(), "v".to_owned())]),
                        "x",
                        1
                    )
                    .await,
                Err(ApplicationMailError::Invalid(_))
            ));
        });
    }
}
