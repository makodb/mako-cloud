//! Reproducible single-process performance baseline for release qualification.

#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    fs,
    hint::black_box,
    io,
    num::{NonZeroU64, NonZeroUsize},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures::executor::block_on;
use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
use mako_control_plane::{
    AutomationPermission, AutomationScope, AutomationTokenId, AutomationTokenService,
    DeveloperIdentityId, EnvironmentRecord, ManagementAction, ManagementActor,
    ManagementAuthorizer, ManagementResourceScope, MembershipRecord, OrganizationId,
    OrganizationRecord, OrganizationRole, OrganizationStore, ProjectRecord, ProjectStore,
};
use mako_documents::{
    CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
    DocumentEngine, DocumentMutationAuthorizer, DocumentReadAuthorizer, DocumentValidator,
    EnvironmentSequencer, IndexActivationOutcome, IndexDefinition, IndexField, IndexKind,
    IndexName, IndexVersion, MutationAuthorizationContext, MutationAuthorizationDecision,
    MutationCommitOutcome, MutationId, MutationInput, PrimaryKeyDefinition,
    ReadAuthorizationContext, SchemaCompatibility, SchemaVersion, ScopedCollectionEngine,
};
use mako_edge_runtime::{
    FunctionInvocationTelemetry, FunctionStructuredLog, FunctionTelemetrySink, IsolatedWorker,
    RegionalWorkerSupervisor, SupervisorConfig, WorkerFactory, WorkerFailure, WorkerIdentity,
    WorkerRequest, WorkerResourceUsage, WorkerResponse, WorkerStartError, WorkerStartRequest,
    WorkerTerminationReason,
};
use mako_edge_runtime_protocol::{
    BundleFormat, DeploymentAddress, LoadDeploymentRequest, OutboundNetworkPolicy,
    PROTOCOL_VERSION, RuntimeLimits,
};
use mako_gateway::{
    AccessTokenVerificationConfig, GatewayAccessTokenVerifier, GatewayAuthorizationEpochResolver,
    GatewaySessionResolver,
};
use mako_identity::{
    AccessAuthorizationEpochs, AccessTokenConfig, AccessTokenInput, AccessTokenIssuer, AppUserId,
    KeyEncryptionKey, ProjectSigningKeyRing, SessionId,
};
use mako_policy::{
    DocumentOperation, PolicyEffect, PolicyEvaluationContext, PolicyEvaluator, PolicyRule,
    PolicyRuleId, PolicySet, PolicyState, PolicyVersion, SafeRequestMetadata, SubjectId,
    VerifiedIdentity, VerifiedRole,
};
use mako_storage::{Durability, KvAdapter, RocksDbAdapter, RocksDbConfig};
use mako_sync::{
    AuthenticatedReplicationContext, LiveStreamEvent, LiveStreamLimits, LiveStreamRequest,
    LiveStreamSession, PullRequest, PullService, PushOutcomeStatus, PushRequest, PushRow,
    PushService, ReplicationTokenCodec, ReplicationTokenKey,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::{Builder as TempDirBuilder, TempDir};

type AnyError = Box<dyn Error>;
type AnyResult<T> = Result<T, AnyError>;

const DEFAULT_ITERATIONS: usize = 25;
const DEFAULT_DATASET_DOCUMENTS: usize = 100;
const DEFAULT_LIVE_SUBSCRIBERS: usize = 25;
const MAX_ITERATIONS: usize = 1_000;
const MAX_DATASET_DOCUMENTS: usize = 900;
const MAX_LIVE_SUBSCRIBERS: usize = 1_000;

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    iterations: usize,
    dataset_documents: usize,
    live_subscribers: usize,
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkParameters {
    iterations: usize,
    dataset_documents: usize,
    live_subscribers: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkHost {
    operating_system: &'static str,
    architecture: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkReport {
    schema_version: u32,
    generated_at_unix_seconds: u64,
    profile: &'static str,
    storage_adapter: &'static str,
    durability: &'static str,
    parameters: BenchmarkParameters,
    host: BenchmarkHost,
    results: Vec<BenchmarkResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkResult {
    path: &'static str,
    samples: usize,
    operations_per_sample: usize,
    total_operations: usize,
    p50_microseconds: f64,
    p95_microseconds: f64,
    min_microseconds: f64,
    max_microseconds: f64,
    operations_per_second: f64,
}

struct DocumentFixture {
    collection: ScopedCollectionEngine,
    sequencer: EnvironmentSequencer,
    validator: DocumentValidator,
    _directory: TempDir,
}

#[derive(Clone, Copy)]
struct AllowAll;

impl DocumentMutationAuthorizer for AllowAll {
    fn authorize(&self, _: MutationAuthorizationContext<'_>) -> MutationAuthorizationDecision {
        MutationAuthorizationDecision::allow("benchmark_allow")
    }
}

impl DocumentReadAuthorizer for AllowAll {
    fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
        MutationAuthorizationDecision::allow("benchmark_allow")
    }
}

#[derive(Clone, Copy)]
struct DenyAllReads;

impl DocumentReadAuthorizer for DenyAllReads {
    fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
        MutationAuthorizationDecision::deny("benchmark_deny")
    }
}

struct AuthState;

impl GatewaySessionResolver for AuthState {
    fn is_active(&self, _: &TenantScope, _: &AppUserId, _: &SessionId, _: u64) -> bool {
        true
    }
}

impl GatewayAuthorizationEpochResolver for AuthState {
    fn current_epochs(&self, _: &TenantScope, _: &AppUserId) -> Option<AccessAuthorizationEpochs> {
        Some(benchmark_epochs())
    }
}

#[derive(Default)]
struct NoopTelemetry;

impl FunctionTelemetrySink for NoopTelemetry {
    fn record_invocation(&self, _: FunctionInvocationTelemetry) {}

    fn record_log(&self, _: FunctionStructuredLog) {}
}

#[derive(Default)]
struct InProcessWorkerFactory;

#[async_trait]
impl WorkerFactory for InProcessWorkerFactory {
    async fn start(
        &self,
        request: WorkerStartRequest,
    ) -> Result<Arc<dyn IsolatedWorker>, WorkerStartError> {
        Ok(Arc::new(InProcessWorker {
            identity: WorkerIdentity {
                deployment: request.manifest.deployment,
                region: request.region,
                generation: request.generation,
            },
        }))
    }
}

struct InProcessWorker {
    identity: WorkerIdentity,
}

#[async_trait]
impl IsolatedWorker for InProcessWorker {
    fn identity(&self) -> &WorkerIdentity {
        &self.identity
    }

    async fn invoke(&self, _: WorkerRequest) -> Result<WorkerResponse, WorkerFailure> {
        Ok(WorkerResponse {
            status: 200,
            headers: Vec::new(),
            body_chunks: vec![b"ok".to_vec()],
            logs: Vec::new(),
            usage: WorkerResourceUsage::default(),
        })
    }

    async fn terminate(&self, _: WorkerTerminationReason) {}
}

fn main() -> AnyResult<()> {
    if cfg!(debug_assertions) {
        return Err(benchmark_error(
            "performance baselines must be compiled with --release",
        ));
    }
    let config = BenchmarkConfig::from_environment_and_args()?;
    let results = vec![
        benchmark_write(&config)?,
        benchmark_pull(&config, false)?,
        benchmark_pull(&config, true)?,
        benchmark_push_conflict(&config)?,
        benchmark_live_fanout(&config)?,
        benchmark_auth(&config)?,
        benchmark_policy(&config)?,
        benchmark_index_build(&config)?,
        benchmark_control_plane(&config)?,
        benchmark_edge_cold(&config)?,
        benchmark_edge_warm(&config)?,
    ];

    let report = BenchmarkReport {
        schema_version: 1,
        generated_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        profile: "release",
        storage_adapter: "local-rocksdb-optimistic-transactiondb",
        durability: "sync",
        parameters: BenchmarkParameters {
            iterations: config.iterations,
            dataset_documents: config.dataset_documents,
            live_subscribers: config.live_subscribers,
        },
        host: BenchmarkHost {
            operating_system: env::consts::OS,
            architecture: env::consts::ARCH,
        },
        results,
    };
    let encoded = serde_json::to_string_pretty(&report)?;
    if let Some(output) = config.output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&output, format!("{encoded}\n"))?;
        println!("wrote benchmark report to {}", output.display());
    } else {
        println!("{encoded}");
    }
    Ok(())
}

impl BenchmarkConfig {
    fn from_environment_and_args() -> AnyResult<Self> {
        let iterations =
            positive_environment("MAKO_BENCH_ITERATIONS", DEFAULT_ITERATIONS, MAX_ITERATIONS)?;
        let dataset_documents = positive_environment(
            "MAKO_BENCH_DATASET_DOCUMENTS",
            DEFAULT_DATASET_DOCUMENTS,
            MAX_DATASET_DOCUMENTS,
        )?;
        let live_subscribers = positive_environment(
            "MAKO_BENCH_LIVE_SUBSCRIBERS",
            DEFAULT_LIVE_SUBSCRIBERS,
            MAX_LIVE_SUBSCRIBERS,
        )?;
        let mut arguments = env::args().skip(1);
        let mut output = None;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--output" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| benchmark_error("--output requires a path"))?;
                    if output.replace(PathBuf::from(value)).is_some() {
                        return Err(benchmark_error("--output may only be supplied once"));
                    }
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: mako-benchmarks [--output PATH]\n\
                         Environment: MAKO_BENCH_ITERATIONS, \
                         MAKO_BENCH_DATASET_DOCUMENTS, MAKO_BENCH_LIVE_SUBSCRIBERS"
                    );
                    std::process::exit(0);
                }
                _ => return Err(benchmark_error(format!("unknown argument: {argument}"))),
            }
        }
        Ok(Self {
            iterations,
            dataset_documents,
            live_subscribers,
            output,
        })
    }
}

fn positive_environment(name: &str, default: usize, maximum: usize) -> AnyResult<usize> {
    match env::var(name) {
        Ok(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| benchmark_error(format!("{name} must be a positive integer")))?;
            if !(1..=maximum).contains(&parsed) {
                return Err(benchmark_error(format!(
                    "{name} must be between 1 and {maximum}"
                )));
            }
            Ok(parsed)
        }
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn benchmark_write(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let fixture = document_fixture("write")?;
    let mut lease = block_on(
        fixture
            .sequencer
            .lease(non_zero_u64(config.iterations, "write iterations")?),
    )?;
    measure("write", config.iterations, 1, |sample| {
        let position = lease
            .issue()
            .ok_or_else(|| benchmark_error("write sequence lease exhausted"))?;
        let document = fixture
            .validator
            .validate_create(benchmark_document(&format!("write-{sample:08}"), sample))?;
        block_on(async {
            let outcome = fixture
                .collection
                .create_document(MutationInput {
                    mutation_id: MutationId::parse(format!("benchmark-write-{sample:08}"))?,
                    commit_position: CommitPosition::new(position)?,
                    document,
                    durability: Durability::Sync,
                })
                .await?;
            if !matches!(outcome, MutationCommitOutcome::Applied(_)) {
                return Err(benchmark_error("write did not apply"));
            }
            fixture.sequencer.recover_high_water().await?;
            Ok(())
        })
    })
}

fn benchmark_pull(config: &BenchmarkConfig, hidden: bool) -> AnyResult<BenchmarkResult> {
    let fixture = document_fixture(if hidden { "hidden-pull" } else { "pull" })?;
    seed_documents(&fixture, config.dataset_documents, "pull")?;
    let context = replication_context();
    let token_key = ReplicationTokenKey::from_bytes([7; 32]);
    let service = PullService::new(
        &fixture.collection,
        ReplicationTokenCodec::new(&token_key),
        SchemaVersion::new(1)?,
    );
    let request = PullRequest {
        checkpoint: None,
        schema_version: 1,
        batch_size: config.dataset_documents,
    };
    let path = if hidden { "hidden-change-scan" } else { "pull" };
    measure(path, config.iterations, 1, |_| {
        block_on(async {
            let response = if hidden {
                service
                    .initial_pull(&context, &request, &DenyAllReads)
                    .await?
            } else {
                service.initial_pull(&context, &request, &AllowAll).await?
            };
            let expected = if hidden { 0 } else { config.dataset_documents };
            if response.documents.len() != expected {
                return Err(benchmark_error(
                    "pull returned an unexpected document count",
                ));
            }
            black_box(response.checkpoint);
            Ok(())
        })
    })
}

fn benchmark_push_conflict(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let fixture = document_fixture("push-conflict")?;
    let stale_revision = block_on(async {
        let mut lease = fixture.sequencer.lease(NonZeroU64::new(2).unwrap()).await?;
        let first_position = lease
            .issue()
            .ok_or_else(|| benchmark_error("missing initial position"))?;
        let first = fixture
            .collection
            .create_document(MutationInput {
                mutation_id: MutationId::parse("benchmark-conflict-initial")?,
                commit_position: CommitPosition::new(first_position)?,
                document: fixture
                    .validator
                    .validate_create(benchmark_document("conflict-doc", 1))?,
                durability: Durability::Sync,
            })
            .await?;
        let MutationCommitOutcome::Applied(first) = first else {
            return Err(benchmark_error("initial conflict document did not apply"));
        };
        let second_position = lease
            .issue()
            .ok_or_else(|| benchmark_error("missing update position"))?;
        let updated = fixture
            .collection
            .update_document(
                first.revision.clone(),
                MutationInput {
                    mutation_id: MutationId::parse("benchmark-conflict-update")?,
                    commit_position: CommitPosition::new(second_position)?,
                    document: fixture
                        .validator
                        .validate_create(benchmark_document("conflict-doc", 2))?,
                    durability: Durability::Sync,
                },
            )
            .await?;
        if !matches!(updated, MutationCommitOutcome::Applied(_)) {
            return Err(benchmark_error("conflict setup update did not apply"));
        }
        fixture.sequencer.recover_high_water().await?;
        Ok(first.revision)
    })?;
    let context = replication_context();
    let service = PushService::new(
        &fixture.collection,
        &fixture.sequencer,
        &fixture.validator,
        Durability::Sync,
    );
    let requests = (0..config.iterations)
        .map(|sample| PushRequest {
            schema_version: 1,
            rows: vec![PushRow {
                mutation_id: format!("benchmark-conflict-{sample:08}"),
                assumed_master_state: Some(json!({"_rev": stale_revision.as_str()})),
                new_document_state: benchmark_document("conflict-doc", sample + 10),
            }],
        })
        .collect::<Vec<_>>();
    measure("push-conflict", config.iterations, 1, |sample| {
        block_on(async {
            let response = service
                .push(&context, &requests[sample], &AllowAll, &AllowAll)
                .await?;
            if response.outcomes.len() != 1
                || response.outcomes[0].status != PushOutcomeStatus::Conflict
                || response.outcomes[0].master_state.is_none()
            {
                return Err(benchmark_error("push did not return a readable conflict"));
            }
            black_box(response);
            Ok(())
        })
    })
}

fn benchmark_live_fanout(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let fixture = document_fixture("live-fanout")?;
    let context = replication_context();
    let token_key = ReplicationTokenKey::from_bytes([8; 32]);
    let request = LiveStreamRequest {
        schema_version: 1,
        checkpoint: None,
        cursor: None,
    };
    let limits = LiveStreamLimits::new(
        NonZeroUsize::new(16).unwrap(),
        NonZeroUsize::new(16).unwrap(),
    );
    let mut sessions = block_on(async {
        let mut sessions = Vec::with_capacity(config.live_subscribers);
        for _ in 0..config.live_subscribers {
            sessions.push(
                LiveStreamSession::open(
                    &fixture.collection,
                    ReplicationTokenCodec::new(&token_key),
                    context.clone(),
                    &AllowAll,
                    SchemaVersion::new(1)?,
                    &request,
                    limits,
                )
                .await?,
            );
        }
        AnyResult::Ok(sessions)
    })?;
    let mut lease = block_on(
        fixture
            .sequencer
            .lease(non_zero_u64(config.iterations, "live iterations")?),
    )?;
    let mut samples = Vec::with_capacity(config.iterations);
    for sample in 0..config.iterations {
        let position = lease
            .issue()
            .ok_or_else(|| benchmark_error("live sequence lease exhausted"))?;
        block_on(async {
            fixture
                .collection
                .create_document(MutationInput {
                    mutation_id: MutationId::parse(format!("benchmark-live-{sample:08}"))?,
                    commit_position: CommitPosition::new(position)?,
                    document: fixture.validator.validate_create(benchmark_document(
                        &format!("live-{sample:08}"),
                        sample,
                    ))?,
                    durability: Durability::Sync,
                })
                .await?;
            fixture.sequencer.recover_high_water().await?;
            AnyResult::Ok(())
        })?;
        let started = Instant::now();
        block_on(async {
            for session in &mut sessions {
                session.poll().await?;
                let events = session.drain_events().collect::<Vec<_>>();
                if events.len() != 1
                    || !matches!(
                        &events[0],
                        LiveStreamEvent::Documents { documents, .. } if documents.len() == 1
                    )
                {
                    return Err(benchmark_error("live session did not receive one document"));
                }
                black_box(events);
            }
            AnyResult::Ok(())
        })?;
        samples.push(started.elapsed().as_nanos());
    }
    summarize("live-fan-out", samples, config.live_subscribers)
}

fn benchmark_auth(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let tenant = benchmark_tenant();
    let encryption_key = KeyEncryptionKey::from_bytes([31; 32]);
    let mut ring = ProjectSigningKeyRing::new(tenant.clone());
    ring.create_initial(&encryption_key, 1)?;
    let token = AccessTokenIssuer::new(
        &ring,
        &encryption_key,
        AccessTokenConfig::new("https://auth.benchmark.test", "mako-data", 300)?,
    )
    .issue(
        AccessTokenInput {
            tenant: tenant.clone(),
            user_id: AppUserId::parse("usr_benchmark0")?,
            role: "member".to_owned(),
            session_id: SessionId::parse("ses_benchmark0")?,
            authorization_epochs: benchmark_epochs(),
            trusted_claims: Map::new(),
        },
        100,
    )?;
    let jwks = ring.jwks();
    let state = AuthState;
    let verifier = GatewayAccessTokenVerifier::new(
        &jwks,
        &state,
        &state,
        AccessTokenVerificationConfig::new("https://auth.benchmark.test", "mako-data", 30)?,
    );
    measure("auth", config.iterations, 1, |_| {
        let identity = verifier.verify(token.expose_for_authorization_header(), &tenant, 101)?;
        black_box(identity);
        Ok(())
    })
}

fn benchmark_policy(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let scope = benchmark_collection_scope();
    let rules = [
        PolicyRule::new(
            PolicyRuleId::parse("member-read")?,
            PolicyEffect::Allow,
            [DocumentOperation::Read],
            "identity.role == \"member\" && old.owner_id == identity.user_id",
        )?,
        PolicyRule::new(
            PolicyRuleId::parse("active-document")?,
            PolicyEffect::Allow,
            [DocumentOperation::Read],
            "old.blocked == false",
        )?,
        PolicyRule::new(
            PolicyRuleId::parse("blocked-document")?,
            PolicyEffect::Deny,
            [DocumentOperation::Read],
            "old.blocked == true",
        )?,
    ];
    let policy = PolicySet::new(
        scope.clone(),
        PolicyVersion::new(1)?,
        PolicyState::Validated,
        rules,
        [],
    )?;
    let compiled = mako_policy::PolicyCompiler::default()
        .compile(
            &policy,
            &json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "owner_id": {"type": "string"},
                    "blocked": {"type": "boolean"}
                }
            }),
        )?
        .into_compiled()
        .ok_or_else(|| benchmark_error("benchmark policy did not compile"))?;
    let context = PolicyEvaluationContext::new(
        scope,
        DocumentOperation::Read,
        VerifiedIdentity::user(
            SubjectId::parse("usr_benchmark0")?,
            VerifiedRole::parse("member")?,
            json!({"plan": "beta"}),
        )?,
        Some(json!({
            "id": "policy-document",
            "owner_id": "usr_benchmark0",
            "blocked": false
        })),
        None,
        SafeRequestMetadata::empty(),
    )?;
    let evaluator = PolicyEvaluator;
    measure("policy", config.iterations, 1, |_| {
        let decision = evaluator.evaluate(&compiled, &context);
        if !decision.is_allowed() || decision.evaluated_rules() != 3 {
            return Err(benchmark_error("benchmark policy was not allowed"));
        }
        black_box(decision);
        Ok(())
    })
}

fn benchmark_index_build(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    measure_prepared(
        "index-build",
        config.iterations,
        1,
        |_| {
            let fixture = document_fixture("index-build")?;
            seed_documents(&fixture, config.dataset_documents, "index")?;
            Ok(fixture)
        },
        |_, fixture| {
            block_on(async {
                let name = IndexName::parse("benchmark-value")?;
                let version = IndexVersion::new(1)?;
                fixture
                    .collection
                    .create_index(
                        IndexDefinition::new_building(
                            CollectionId::parse("benchdocs")?,
                            name.clone(),
                            version,
                            IndexKind::NonUnique,
                            [IndexField::ascending("value")?],
                        )?,
                        Durability::Sync,
                    )
                    .await?;
                let page_size = NonZeroUsize::new(config.dataset_documents + 1).unwrap();
                let backfill = fixture
                    .collection
                    .backfill_index(&name, version, page_size, Durability::Sync)
                    .await?;
                if !backfill.backfill_complete() {
                    return Err(benchmark_error("index backfill did not finish"));
                }
                let catch_up = fixture
                    .collection
                    .catch_up_index(&name, version, page_size, Durability::Sync)
                    .await?;
                if catch_up.caught_up_position() < backfill.captured_position() {
                    return Err(benchmark_error("index catch-up did not reach the fence"));
                }
                let outcome = fixture
                    .collection
                    .activate_index(&name, version, Durability::Sync)
                    .await?;
                if !matches!(outcome, IndexActivationOutcome::Active(_)) {
                    return Err(benchmark_error("index did not activate"));
                }
                Ok(())
            })
        },
    )
}

fn benchmark_control_plane(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let directory = benchmark_directory("control-plane")?;
    let adapter = rocks_adapter(directory.path().join("rocksdb"))?;
    let organizations = OrganizationStore::new(adapter.clone(), Durability::Sync)?;
    let projects = ProjectStore::new(adapter.clone(), Durability::Sync)?;
    let organization_id = OrganizationId::parse("org_benchmark0")?;
    let developer_id = DeveloperIdentityId::parse("dev_benchmark0")?;
    let project_id = ProjectId::parse("prj_benchmark0")?;
    let environment_id = EnvironmentId::parse("env_benchmark0")?;
    block_on(async {
        organizations
            .create_organization(
                &OrganizationRecord::new(organization_id.clone(), "Benchmark", 1)?,
                &MembershipRecord::new(
                    organization_id.clone(),
                    developer_id.clone(),
                    OrganizationRole::Owner,
                    1,
                ),
            )
            .await?;
        projects
            .create_project(&ProjectRecord::new(
                project_id.clone(),
                organization_id.clone(),
                "Benchmark",
                "local",
                1,
            )?)
            .await?;
        projects
            .create_environment(&EnvironmentRecord::new(
                environment_id.clone(),
                project_id.clone(),
                "Benchmark",
                1,
            )?)
            .await?;
        AnyResult::Ok(())
    })?;
    let tokens = AutomationTokenService::new(adapter, Durability::Sync)?;
    let principal = block_on(async {
        let issue = tokens
            .issue(
                AutomationTokenId::parse("atm_benchmark0")?,
                "benchmark",
                AutomationScope::new(
                    organization_id.clone(),
                    Some(project_id.clone()),
                    Some(environment_id.clone()),
                    [AutomationPermission::EnvironmentRead],
                )?,
                developer_id,
                1,
                1_000,
            )
            .await?;
        Ok::<_, AnyError>(tokens.authenticate(&issue.secret, 2).await?)
    })?;
    let resource = ManagementResourceScope {
        organization_id,
        project_id: Some(project_id),
        environment_id: Some(environment_id),
    };
    let authorizer = ManagementAuthorizer::new(organizations, projects);
    measure("control-plane", config.iterations, 1, |_| {
        block_on(async {
            let actor = authorizer
                .authorize(
                    ManagementActor::Automation(&principal),
                    ManagementAction::EnvironmentRead,
                    &resource,
                )
                .await?;
            black_box(actor);
            Ok(())
        })
    })
}

fn benchmark_edge_cold(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    measure_prepared(
        "edge-cold",
        config.iterations,
        1,
        |_| Ok((edge_supervisor()?, edge_deployment(), edge_bundle())),
        |sample, (supervisor, manifest, bundle)| {
            block_on(async {
                let address = manifest.deployment.clone();
                supervisor
                    .load(manifest, bundle, Vec::new(), sample as u64 + 1)
                    .await?;
                let response = supervisor
                    .invoke(&address, edge_request(sample), sample as u64 + 1)
                    .await?;
                if response.status != 200 {
                    return Err(benchmark_error("cold edge invocation failed"));
                }
                black_box(response);
                Ok(())
            })
        },
    )
}

fn benchmark_edge_warm(config: &BenchmarkConfig) -> AnyResult<BenchmarkResult> {
    let supervisor = edge_supervisor()?;
    let manifest = edge_deployment();
    let address = manifest.deployment.clone();
    block_on(supervisor.load(manifest, edge_bundle(), Vec::new(), 1))?;
    measure("edge-warm", config.iterations, 1, |sample| {
        block_on(async {
            let response = supervisor
                .invoke(&address, edge_request(sample), sample as u64 + 2)
                .await?;
            if response.status != 200 {
                return Err(benchmark_error("warm edge invocation failed"));
            }
            black_box(response);
            Ok(())
        })
    })
}

fn measure(
    path: &'static str,
    iterations: usize,
    operations_per_sample: usize,
    mut operation: impl FnMut(usize) -> AnyResult<()>,
) -> AnyResult<BenchmarkResult> {
    let mut samples = Vec::with_capacity(iterations);
    for sample in 0..iterations {
        let started = Instant::now();
        operation(sample)?;
        samples.push(started.elapsed().as_nanos());
    }
    summarize(path, samples, operations_per_sample)
}

fn measure_prepared<T>(
    path: &'static str,
    iterations: usize,
    operations_per_sample: usize,
    mut prepare: impl FnMut(usize) -> AnyResult<T>,
    mut operation: impl FnMut(usize, T) -> AnyResult<()>,
) -> AnyResult<BenchmarkResult> {
    let mut samples = Vec::with_capacity(iterations);
    for sample in 0..iterations {
        let prepared = prepare(sample)?;
        let started = Instant::now();
        operation(sample, prepared)?;
        samples.push(started.elapsed().as_nanos());
    }
    summarize(path, samples, operations_per_sample)
}

fn summarize(
    path: &'static str,
    mut samples: Vec<u128>,
    operations_per_sample: usize,
) -> AnyResult<BenchmarkResult> {
    if samples.is_empty() || operations_per_sample == 0 {
        return Err(benchmark_error("benchmark samples must be non-empty"));
    }
    samples.sort_unstable();
    let total_nanoseconds = samples.iter().copied().sum::<u128>();
    let total_operations = samples
        .len()
        .checked_mul(operations_per_sample)
        .ok_or_else(|| benchmark_error("benchmark operation count overflowed"))?;
    let operations_per_second =
        total_operations as f64 * 1_000_000_000_f64 / total_nanoseconds.max(1) as f64;
    Ok(BenchmarkResult {
        path,
        samples: samples.len(),
        operations_per_sample,
        total_operations,
        p50_microseconds: nanoseconds_to_microseconds(percentile(&samples, 50)),
        p95_microseconds: nanoseconds_to_microseconds(percentile(&samples, 95)),
        min_microseconds: nanoseconds_to_microseconds(samples[0]),
        max_microseconds: nanoseconds_to_microseconds(samples[samples.len() - 1]),
        operations_per_second,
    })
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let rank = samples
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(samples.len() - 1);
    samples[rank]
}

fn nanoseconds_to_microseconds(value: u128) -> f64 {
    value as f64 / 1_000_f64
}

fn document_fixture(label: &str) -> AnyResult<DocumentFixture> {
    let directory = benchmark_directory(label)?;
    let adapter = rocks_adapter(directory.path().join("rocksdb"))?;
    let engine = DocumentEngine::new(adapter);
    let tenant = benchmark_tenant();
    let collection = engine.scope_collection(&tenant, benchmark_collection_scope())?;
    let sequencer = engine.scope_sequencer(&tenant, &tenant, Durability::Sync)?;
    let validator = DocumentValidator::compile(&benchmark_collection_metadata()?)?;
    Ok(DocumentFixture {
        collection,
        sequencer,
        validator,
        _directory: directory,
    })
}

fn rocks_adapter(path: impl AsRef<Path>) -> AnyResult<Arc<dyn KvAdapter>> {
    Ok(Arc::new(RocksDbAdapter::open(RocksDbConfig::new(
        path.as_ref(),
    ))?))
}

fn benchmark_directory(label: &str) -> AnyResult<TempDir> {
    let root = PathBuf::from(".playwright-tmp").join("benchmarks");
    fs::create_dir_all(&root)?;
    Ok(TempDirBuilder::new()
        .prefix(&format!("{label}-"))
        .tempdir_in(root)?)
}

fn benchmark_collection_metadata() -> AnyResult<CollectionMetadata> {
    Ok(CollectionMetadata::new(
        CollectionId::parse("benchdocs")?,
        CollectionMetadataVersion::new(1)?,
        SchemaVersion::new(1)?,
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "value": {"type": "integer"},
                "title": {"type": "string"},
                "owner_id": {"type": "string"},
                "blocked": {"type": "boolean"}
            },
            "required": ["id", "value", "title", "owner_id", "blocked"],
            "additionalProperties": false
        }),
        PrimaryKeyDefinition::field("id")?,
        SchemaCompatibility::Compatible,
        CollectionLifecycle::Active,
    )?)
}

fn benchmark_document(id: &str, value: usize) -> Value {
    json!({
        "id": id,
        "value": value,
        "title": format!("document-{value:08}"),
        "owner_id": "usr_benchmark0",
        "blocked": false
    })
}

fn seed_documents(fixture: &DocumentFixture, count: usize, prefix: &str) -> AnyResult<()> {
    block_on(async {
        let mut lease = fixture
            .sequencer
            .lease(non_zero_u64(count, "seed document count")?)
            .await?;
        for index in 0..count {
            let position = lease
                .issue()
                .ok_or_else(|| benchmark_error("seed sequence lease exhausted"))?;
            let outcome = fixture
                .collection
                .create_document(MutationInput {
                    mutation_id: MutationId::parse(format!("benchmark-{prefix}-seed-{index:08}"))?,
                    commit_position: CommitPosition::new(position)?,
                    document: fixture.validator.validate_create(benchmark_document(
                        &format!("{prefix}-{index:08}"),
                        index,
                    ))?,
                    durability: Durability::Sync,
                })
                .await?;
            if !matches!(outcome, MutationCommitOutcome::Applied(_)) {
                return Err(benchmark_error("seed document did not apply"));
            }
        }
        fixture.sequencer.recover_high_water().await?;
        Ok(())
    })
}

fn replication_context() -> AuthenticatedReplicationContext {
    AuthenticatedReplicationContext::new(
        benchmark_tenant(),
        CollectionId::parse("benchdocs").unwrap(),
        AppUserId::parse("usr_benchmark0").unwrap(),
        SessionId::parse("ses_benchmark0").unwrap(),
        "member",
        benchmark_epochs(),
        "req_benchmark0",
    )
    .unwrap()
}

fn benchmark_epochs() -> AccessAuthorizationEpochs {
    AccessAuthorizationEpochs {
        environment: 1,
        user: 1,
    }
}

fn benchmark_tenant() -> TenantScope {
    TenantScope::new(
        ProjectId::parse("prj_benchmark0").unwrap(),
        EnvironmentId::parse("env_benchmark0").unwrap(),
    )
}

fn benchmark_collection_scope() -> CollectionScope {
    CollectionScope::new(
        benchmark_tenant(),
        CollectionId::parse("benchdocs").unwrap(),
    )
}

fn edge_supervisor() -> AnyResult<RegionalWorkerSupervisor> {
    Ok(RegionalWorkerSupervisor::new(
        SupervisorConfig {
            region: "local".to_owned(),
            max_project_concurrency: 16,
            max_invocations_per_worker: u64::MAX,
            max_worker_age_milliseconds: 60_000,
        },
        Arc::new(InProcessWorkerFactory),
        Arc::new(NoopTelemetry),
    )?)
}

fn edge_deployment() -> LoadDeploymentRequest {
    LoadDeploymentRequest {
        protocol_version: PROTOCOL_VERSION,
        deployment: DeploymentAddress {
            tenant: benchmark_tenant(),
            function_name: "benchmark".to_owned(),
            version: 1,
        },
        bundle_digest: edge_bundle_digest(),
        bundle_format: BundleFormat::Prebuilt,
        entrypoint: "index.ts".to_owned(),
        runtime_release: "v1.74.3".to_owned(),
        limits: RuntimeLimits {
            cpu_milliseconds: 100,
            wall_milliseconds: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            request_bytes: 64 * 1024,
            response_bytes: 64 * 1024,
            concurrency: 16,
            outbound_network: OutboundNetworkPolicy::DenyAll,
        },
        verify_jwt: true,
        secret_versions: Vec::new(),
    }
}

fn edge_bundle() -> Arc<[u8]> {
    Arc::from(b"benchmark bundle".as_slice())
}

fn edge_bundle_digest() -> String {
    format!("sha256:{:x}", Sha256::digest(b"benchmark bundle"))
}

fn edge_request(sample: usize) -> WorkerRequest {
    WorkerRequest {
        request_id: format!("req_edge_{sample:08}"),
        trace_id: format!("trc_edge_{sample:08}"),
        method: "GET".to_owned(),
        path_and_query: "/benchmark".to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn non_zero_u64(value: usize, name: &str) -> AnyResult<NonZeroU64> {
    let value =
        u64::try_from(value).map_err(|_| benchmark_error(format!("{name} does not fit in u64")))?;
    NonZeroU64::new(value).ok_or_else(|| benchmark_error(format!("{name} must be positive")))
}

fn benchmark_error(message: impl Into<String>) -> AnyError {
    Box::new(io::Error::other(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_uses_nearest_rank_percentiles_and_fanout_operations() {
        let result = summarize("test", vec![1_000, 5_000, 2_000, 4_000, 3_000], 10).unwrap();
        assert_eq!(result.p50_microseconds, 3.0);
        assert_eq!(result.p95_microseconds, 5.0);
        assert_eq!(result.total_operations, 50);
        assert!(result.operations_per_second > 0.0);
    }
}
