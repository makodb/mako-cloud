//! Carry function log lines into the retained telemetry store.
//!
//! The runtime supervisor keeps each deployment's recent log lines in a
//! bounded in-memory buffer that a container restart empties. This collector
//! walks the deployed functions off the request path, reads each one's buffer
//! forward, scrubs every line, and emits it as a `ProjectLog` record over the
//! same bounded pipeline every other signal travels -- which is what gives a
//! log line retention, tenant scoping, and a query surface.

use std::{num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use mako_api::{ObservabilityPayload, ObservabilityRecord, TenantScope};
use mako_audit::TelemetryRedactor;
use mako_control_plane::{
    ControlKeyspace, FunctionAdminError, FunctionAdminService, FunctionLogEntry, FunctionLogQuery,
    FunctionName, FunctionRecord, OrganizationStore, ProjectStore,
};
use mako_storage::{Durability, KvAdapter, ScanDirection, ScanRequest, WriteBatch};
use mako_telemetry_client::TelemetryEmitter;
use serde::{Deserialize, Serialize};

/// One supervisor page. Small enough that a pass stays cheap, large enough
/// that a chatty function drains in a few pages.
const PAGE_LIMIT: usize = 500;
/// Pages read per function per pass. Together with the page size this covers
/// more than the supervisor can buffer for one function, so a pass that
/// starts from the beginning always reaches the newest lines.
const MAX_PAGES_PER_PASS: usize = 24;
/// Lines older than this are left where they are. The telemetry store
/// refuses records outside its retention window, and one such record poisons
/// the whole batch it travels in; a day covers every honest delay.
const MAX_LOG_AGE_MILLISECONDS: u64 = 24 * 60 * 60 * 1_000;
/// How many organizations, projects, environments, or functions one pass
/// visits per listing. Platform maintenance must be bounded like everything
/// else.
const LISTING_LIMIT: usize = 256;

/// What the collector remembers about one function between passes.
///
/// The supervisor's cursors are positions into an in-memory buffer, so they
/// die with it; the timestamp high-water mark is what survives on this side
/// and keeps a re-read buffer from being re-emitted. Timestamps carry whole
/// milliseconds and a chatty function prints several lines in one, so the
/// mark also counts how many lines at that exact millisecond were already
/// emitted -- the supervisor serves entries in stable order, which makes the
/// pair a resumable position.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct CollectorState {
    high_water_unix_milliseconds: u64,
    emitted_at_high_water: u64,
}

/// The one thing the collector needs from the function service: a page of
/// supervisor logs with the member-facing secret-value redaction already
/// applied. Narrow so tests can script it.
#[async_trait]
pub(crate) trait CollectedLogSource: Send + Sync {
    async fn collect_logs(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        query: &FunctionLogQuery,
    ) -> Result<mako_control_plane::FunctionLogPage, FunctionAdminError>;
}

#[async_trait]
impl CollectedLogSource for FunctionAdminService {
    async fn collect_logs(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        query: &FunctionLogQuery,
    ) -> Result<mako_control_plane::FunctionLogPage, FunctionAdminError> {
        Self::collect_logs(self, tenant, name, query).await
    }
}

pub(crate) struct FunctionLogCollector {
    adapter: Arc<dyn KvAdapter>,
    organizations: OrganizationStore,
    projects: ProjectStore,
    functions: Arc<dyn CollectedLogSource>,
    emitter: Arc<TelemetryEmitter>,
    redactor: Arc<TelemetryRedactor>,
}

impl FunctionLogCollector {
    pub(crate) fn new(
        adapter: Arc<dyn KvAdapter>,
        organizations: OrganizationStore,
        projects: ProjectStore,
        functions: Arc<dyn CollectedLogSource>,
        emitter: Arc<TelemetryEmitter>,
        redactor: Arc<TelemetryRedactor>,
    ) -> Self {
        Self {
            adapter,
            organizations,
            projects,
            functions,
            emitter,
            redactor,
        }
    }

    /// Visit every deployed function once and emit what is new. Returns how
    /// many log records were emitted. Failures skip the function rather than
    /// the pass: one broken tenant must not silence every other tenant's
    /// logs.
    pub(crate) async fn collect_once(&self, now_unix_milliseconds: u64) -> usize {
        let limit = NonZeroUsize::new(LISTING_LIMIT).expect("listing limit");
        let mut emitted = 0;
        let Ok(organizations) = self.organizations.all_organizations(limit).await else {
            return 0;
        };
        // Bounded coverage must not be silent coverage: a deployment that
        // outgrows a listing bound needs an operator to know logs stopped
        // being complete, not to discover it during an incident.
        if organizations.len() >= LISTING_LIMIT {
            eprintln!(
                "function log collection reached its organization listing bound: class=coverage limit={LISTING_LIMIT}"
            );
        }
        for organization in organizations {
            let Ok(projects) = self.projects.list_projects(organization.id(), limit).await else {
                continue;
            };
            for project in projects {
                let Ok(environments) = self.projects.list_environments(project.id(), limit).await
                else {
                    continue;
                };
                for environment in environments {
                    let tenant = TenantScope::new(project.id().clone(), environment.id().clone());
                    for function in self.deployed_functions(&tenant, limit).await {
                        emitted += self
                            .collect_for_function(&tenant, function.name(), now_unix_milliseconds)
                            .await;
                    }
                }
            }
        }
        emitted
    }

    /// The functions of one environment that have an active deployment; a
    /// function nothing deployed has no worker and therefore no logs.
    async fn deployed_functions(
        &self,
        tenant: &TenantScope,
        limit: NonZeroUsize,
    ) -> Vec<FunctionRecord> {
        let Ok(range) =
            ControlKeyspace::functions_range(tenant.project_id(), tenant.environment_id())
        else {
            return Vec::new();
        };
        let Ok(entries) = self
            .adapter
            .scan(ScanRequest::new(range, ScanDirection::Forward, limit))
            .await
        else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|entry| serde_json::from_slice::<FunctionRecord>(&entry.value).ok())
            .filter(|function| function.active_version().is_some())
            .collect()
    }

    /// Read one function's supervisor buffer forward and emit every line
    /// newer than the persisted mark, scrubbed. The mark only advances past
    /// a line once the emitter has actually kept it: when the buffer sheds,
    /// the pass stops and the next one resumes from the same position.
    pub(crate) async fn collect_for_function(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        now_unix_milliseconds: u64,
    ) -> usize {
        let Ok(state_key) = ControlKeyspace::function_log_state_key(
            tenant.project_id(),
            tenant.environment_id(),
            name.as_str(),
        ) else {
            return 0;
        };
        let mut state = match self.adapter.get(&state_key).await {
            Ok(stored) => stored
                .as_deref()
                .and_then(|bytes| serde_json::from_slice::<CollectorState>(bytes).ok())
                .unwrap_or_default(),
            // If the mark cannot be read, emitting anyway would duplicate
            // everything the buffer holds; skipping loses nothing durable.
            Err(_) => return 0,
        };
        let started = (
            state.high_water_unix_milliseconds,
            state.emitted_at_high_water,
        );
        let freshness_floor = now_unix_milliseconds.saturating_sub(MAX_LOG_AGE_MILLISECONDS);

        let mut emitted = 0;
        let mut seen_at_high_water = 0_u64;
        let mut cursor: Option<String> = None;
        'pass: for _page in 0..MAX_PAGES_PER_PASS {
            let query = FunctionLogQuery {
                cursor: cursor.clone(),
                limit: PAGE_LIMIT,
            };
            let Ok(page) = self.functions.collect_logs(tenant, name, &query).await else {
                break;
            };
            for entry in &page.items {
                let at = entry.timestamp_unix_milliseconds;
                if at < state.high_water_unix_milliseconds {
                    continue;
                }
                if at == state.high_water_unix_milliseconds {
                    seen_at_high_water += 1;
                    if seen_at_high_water <= state.emitted_at_high_water {
                        continue;
                    }
                }
                // A line the store's retention would refuse must stay
                // behind: one such record poisons the whole batch it
                // travels in. The mark walks over it so it is never
                // revisited.
                if at < freshness_floor {
                    advance(&mut state, &mut seen_at_high_water, at);
                    continue;
                }
                match self.log_record(tenant, name, entry) {
                    Some(record) => {
                        if !self.emitter.try_record(record) {
                            // The buffer shed it; stopping here means the
                            // mark still points at this line and the next
                            // pass retries it.
                            break 'pass;
                        }
                        emitted += 1;
                        advance(&mut state, &mut seen_at_high_water, at);
                    }
                    None => advance(&mut state, &mut seen_at_high_water, at),
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        if (
            state.high_water_unix_milliseconds,
            state.emitted_at_high_water,
        ) != started
        {
            let mut batch = WriteBatch::new();
            if let Ok(value) = serde_json::to_vec(&state) {
                batch.put(&state_key, value);
                // Memory durability on purpose: losing the mark in a crash
                // costs at most a re-emitted buffer, which the same mark
                // bounds again on the next boot, and a log pass must not pay
                // for an fsync per chatty function.
                let _ = self.adapter.write(batch, Durability::Memory).await;
            }
        }
        emitted
    }

    /// One supervisor entry as the telemetry record it becomes, scrubbed and
    /// shaped to the wire contract's bounds -- or nothing, when the entry has
    /// no text worth storing.
    fn log_record(
        &self,
        tenant: &TenantScope,
        name: &FunctionName,
        entry: &FunctionLogEntry,
    ) -> Option<ObservabilityRecord> {
        // Every admitted invocation already produces a function metric;
        // storing its lifecycle echo as a log line would couple retained log
        // volume to request rate for functions that never print anything.
        if entry.message == "invocation_completed" {
            return None;
        }
        let message = self.redactor.scrub_log_text(&entry.message).into_string();
        let message = truncated(message.trim(), 4_096);
        if message.is_empty() {
            return None;
        }
        let level = truncated(entry.level.trim(), 32);
        let correlation_id = truncated(entry.correlation_id.trim(), 128);
        Some(ObservabilityRecord {
            tenant: tenant.clone(),
            timestamp_unix_milliseconds: entry.timestamp_unix_milliseconds,
            payload: ObservabilityPayload::ProjectLog {
                source: truncated(&format!("function:{}", name.as_str()), 128),
                level: if level.is_empty() {
                    "info".to_owned()
                } else {
                    level
                },
                message,
                correlation_id: if correlation_id.is_empty() {
                    "unattributed".to_owned()
                } else {
                    correlation_id
                },
            },
        })
    }
}

/// Move the mark to this line's position.
fn advance(state: &mut CollectorState, seen_at_high_water: &mut u64, at: u64) {
    if at == state.high_water_unix_milliseconds {
        state.emitted_at_high_water = *seen_at_high_water;
    } else {
        state.high_water_unix_milliseconds = at;
        state.emitted_at_high_water = 1;
        *seen_at_high_water = 1;
    }
}

/// At most `limit` bytes, cut on a character boundary.
fn truncated(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};
    use mako_control_plane::FunctionLogPage;
    use mako_storage::MemoryAdapter;
    use std::sync::Mutex;

    use super::*;

    struct ScriptedLogs {
        pages: Mutex<Vec<FunctionLogPage>>,
    }

    #[async_trait]
    impl CollectedLogSource for ScriptedLogs {
        async fn collect_logs(
            &self,
            _: &TenantScope,
            _: &FunctionName,
            _: &FunctionLogQuery,
        ) -> Result<FunctionLogPage, FunctionAdminError> {
            let mut pages = self.pages.lock().expect("pages");
            if pages.is_empty() {
                return Err(FunctionAdminError::InvalidLogQuery);
            }
            Ok(pages.remove(0))
        }
    }

    fn entry(timestamp: u64, message: &str) -> FunctionLogEntry {
        FunctionLogEntry {
            timestamp_unix_milliseconds: timestamp,
            level: "info".to_owned(),
            message: message.to_owned(),
            correlation_id: "req_logs0001".to_owned(),
            version: 3,
            region: "local".to_owned(),
        }
    }

    fn collector(pages: Vec<FunctionLogPage>) -> (FunctionLogCollector, Arc<dyn KvAdapter>) {
        let adapter: Arc<dyn KvAdapter> = Arc::new(MemoryAdapter::new());
        let collector = FunctionLogCollector::new(
            Arc::clone(&adapter),
            OrganizationStore::new(Arc::clone(&adapter), Durability::Memory).expect("orgs"),
            ProjectStore::new(Arc::clone(&adapter), Durability::Memory).expect("projects"),
            Arc::new(ScriptedLogs {
                pages: Mutex::new(pages),
            }),
            Arc::new(TelemetryEmitter::new(
                "127.0.0.1:1".parse().expect("address"),
                "0123456789abcdef0123456789abcdef",
                "mako.test",
            )),
            Arc::new(TelemetryRedactor::new(std::iter::empty::<&str>()).expect("redactor")),
        );
        (collector, adapter)
    }

    fn scope() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_logs00000001").expect("project"),
            EnvironmentId::parse("env_logs00000001").expect("environment"),
        )
    }

    const NOW: u64 = 200_000_000;

    /// The supervisor's buffer is re-read from the start every pass, so the
    /// persisted mark is the only thing standing between one printed line
    /// and many stored copies of it -- including lines that share a
    /// millisecond, which real chatty functions produce constantly.
    #[test]
    fn a_second_pass_emits_only_lines_beyond_the_mark_even_within_one_millisecond() {
        block_on(async {
            let name = FunctionName::parse("checkout").expect("name");
            let (collector, _adapter) = collector(vec![
                FunctionLogPage {
                    items: vec![
                        entry(NOW - 3_000, "started"),
                        entry(NOW - 3_000, "same millisecond"),
                        entry(NOW - 2_000, "finished"),
                    ],
                    next_cursor: None,
                },
                FunctionLogPage {
                    items: vec![
                        entry(NOW - 3_000, "started"),
                        entry(NOW - 3_000, "same millisecond"),
                        entry(NOW - 2_000, "finished"),
                        entry(NOW - 2_000, "second at that instant"),
                        entry(NOW - 1_000, "one more for alice@example.com"),
                    ],
                    next_cursor: None,
                },
            ]);

            assert_eq!(
                collector.collect_for_function(&scope(), &name, NOW).await,
                3,
                "a line sharing a millisecond with its predecessor was dropped"
            );
            assert_eq!(
                collector.collect_for_function(&scope(), &name, NOW).await,
                2,
                "already-emitted lines were emitted again, or new ones missed"
            );
            assert_eq!(collector.emitter.buffered(), 5);
        });
    }

    /// A supervisor that cannot answer costs this pass nothing durable: the
    /// mark stays where it was and the next pass starts over.
    #[test]
    fn a_failing_source_emits_nothing_and_keeps_the_mark() {
        block_on(async {
            let name = FunctionName::parse("checkout").expect("name");
            let (collector, _adapter) = collector(vec![FunctionLogPage {
                items: vec![entry(NOW - 1_000, "kept")],
                next_cursor: None,
            }]);
            assert_eq!(
                collector.collect_for_function(&scope(), &name, NOW).await,
                1
            );
            // The scripted source now fails; nothing new is emitted.
            assert_eq!(
                collector.collect_for_function(&scope(), &name, NOW).await,
                0
            );
            assert_eq!(collector.emitter.buffered(), 1);
        });
    }

    /// A line the telemetry store's retention would refuse must never enter
    /// a batch -- it would poison every record travelling with it -- and a
    /// lifecycle echo of an invocation must not be stored as a log line.
    #[test]
    fn stale_lines_and_invocation_echoes_are_walked_over_without_emission() {
        block_on(async {
            let name = FunctionName::parse("checkout").expect("name");
            let stale = NOW - MAX_LOG_AGE_MILLISECONDS - 5_000;
            let (collector, _adapter) = collector(vec![FunctionLogPage {
                items: vec![
                    entry(stale, "printed a week ago"),
                    entry(NOW - 2_000, "invocation_completed"),
                    entry(NOW - 1_000, "fresh"),
                ],
                next_cursor: None,
            }]);
            assert_eq!(
                collector.collect_for_function(&scope(), &name, NOW).await,
                1,
                "only the fresh printed line should be emitted"
            );
            assert_eq!(collector.emitter.buffered(), 1);
        });
    }

    /// Empty lines vanish; oversized fields are cut to the wire contract's
    /// bounds instead of poisoning the batch they travel in.
    #[test]
    fn records_are_shaped_to_the_wire_contract() {
        block_on(async {
            let name = FunctionName::parse("checkout").expect("name");
            let (collector, _adapter) = collector(Vec::new());
            assert!(
                collector
                    .log_record(&scope(), &name, &entry(1, "   "))
                    .is_none(),
                "a blank line was kept"
            );
            let record = collector
                .log_record(&scope(), &name, &entry(1, &"x".repeat(9_000)))
                .expect("record");
            let ObservabilityPayload::ProjectLog {
                message,
                source,
                level,
                ..
            } = record.payload
            else {
                panic!("expected a project log");
            };
            assert_eq!(message.len(), 4_096);
            assert_eq!(source, "function:checkout");
            assert_eq!(level, "info");
        });
    }
}
