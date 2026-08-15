#!/usr/bin/env node

import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const gates = await readJson("docs/release-gates.json");
const storage = await readJson(gates.sources?.storageQualification);
const performance = await readJson(gates.sources?.performanceBaseline);
const betaPlan = await readJson(gates.sources?.publicBetaPlan);
const betaManifest = await readJson(gates.sources?.publicBetaReleaseManifest);
const betaDeployment = await readJson(gates.sources?.publicBetaDeployment);
const betaStorage = await readJson(gates.sources?.publicBetaStorage);
const betaRecovery = await readJson(gates.sources?.publicBetaRecovery);
const betaRollback = await readJson(gates.sources?.publicBetaRollback);
const betaMeasurement = await readJson(gates.sources?.publicBetaMeasurementWindow);
const betaStreaming = await readJson(gates.sources?.publicBetaStreamingQualification);
const betaBenchmark = await readJson(gates.sources?.publicBetaHostedBenchmark);
const betaPreview = await readJson(gates.sources?.publicBetaPreviewAdmission);
const betaMail = await readJson(gates.sources?.publicBetaMailQualification);
const betaDeveloperRegistration = await readJson(gates.sources?.publicBetaDeveloperRegistration);

assert(gates.schemaVersion === 1, "release-gates schemaVersion must be 1");
await access(resolve(root, gates.sources?.traceability ?? ""));
const stages = new Map(gates.stages?.map((stage) => [stage.id, stage]) ?? []);
assert(stages.size === 3, "exactly three release stages are required");

const internal = requireStage("internal", "pass");
const internalThresholds = internal.thresholds;
const internalObserved = internal.observed;
assert(
  internalObserved.acknowledgedWriteLoss === storage.evidence.acknowledgedWriteLoss &&
    internalObserved.acknowledgedWriteLoss === internalThresholds.acknowledgedWriteLoss,
  "internal acknowledged-write evidence is inconsistent",
);
assert(
  internalObserved.integrityFailures === storage.evidence.integrityFailures &&
    internalObserved.integrityFailures === internalThresholds.integrityFailures,
  "internal integrity evidence is inconsistent",
);
assert(
  internalObserved.storageSoakIterations === storage.evidence.soakIterations &&
    internalObserved.storageSoakIterations >= internalThresholds.minimumStorageSoakIterations,
  "internal storage soak is below threshold",
);
assert(
  internalObserved.securitySuitesPassed >= internalThresholds.requiredSecuritySuites,
  "internal security suite count is below threshold",
);
for (const evidence of internal.securityEvidence ?? []) await access(resolve(root, evidence));
assertAtMost(
  internalObserved.backupAgeSeconds,
  internalThresholds.maximumBackupAgeSeconds,
  "backup age",
);
assertAtMost(
  internalObserved.recoveryPointExposureSeconds,
  internalThresholds.maximumRecoveryPointExposureSeconds,
  "recovery-point exposure",
);
assertAtMost(
  internalObserved.sameVolumeRecoverySeconds,
  internalThresholds.maximumSameVolumeRecoverySeconds,
  "same-volume recovery",
);
assertAtMost(
  internalObserved.replacementRestoreSeconds,
  internalThresholds.maximumReplacementRestoreSeconds,
  "replacement restore",
);
assert(
  internalObserved.availableStorageBytes === storage.storage.availableBytes &&
    internalObserved.availableStorageBytes >= internalThresholds.minimumAvailableStorageBytes,
  "internal capacity is below threshold",
);
assertAtMost(
  internalObserved.incrementalMonthlyCashCostUsd,
  internalThresholds.maximumIncrementalMonthlyCashCostUsd,
  "internal incremental cash cost",
);
assert(
  internalObserved.costMeasurementScope?.includes("incremental cash spend only"),
  "internal cost scope must exclude unmeasured allocated cost explicitly",
);
assert(performance.results?.length === 11, "internal performance evidence must cover 11 paths");
assert(internal.blockers?.length === 0, "a passing internal gate cannot have blockers");

const beta = requireStage("single-region-beta", "blocked");
assertNonNegativeThresholds(beta.thresholds, "single-region beta");
const betaObserved = beta.observed;
for (const field of ["monthlyFixedInfrastructureUsd", "costPerMillionReplicationOperationsUsd"]) {
  assert(beta.observed?.[field] === null, `beta ${field} must remain null until measured`);
}
assert(betaPlan.identity?.fqdn === "cloud-test.makodb.com", "beta plan has the wrong FQDN");
assert(betaPlan.identity?.ipv4Address === "130.245.173.11", "beta plan has the wrong address");
assert(betaPlan.proxmox?.vmId === 124, "beta plan has the wrong VM identifier");
assert(betaObserved.deployment?.vmId === betaPlan.proxmox.vmId, "beta gate uses another VM");
assert(
  betaObserved.deployment?.address === betaPlan.identity.ipv4Address,
  "beta gate uses another address",
);
assert(
  betaObserved.deployment?.publicOrigin === `https://${betaPlan.identity.fqdn}`,
  "beta gate uses another origin",
);
assert(betaObserved.deployment?.planHash === betaPlan.planHash, "beta gate uses another plan");
assert(betaManifest.planHash === betaPlan.planHash, "beta release uses another plan");
assert(
  betaObserved.deployment?.releaseDigest === betaManifest.releaseDigest,
  "beta gate uses another release",
);
assert(
  betaObserved.deployment?.runtimeDigest === betaManifest.runtime?.digest,
  "beta gate uses another runtime",
);
assert(
  betaDeployment.release?.digest === betaManifest.releaseDigest,
  "deployment evidence is stale",
);
assert(
  betaDeployment.networkAdmission?.unrestrictedPublicAdmissionApproved === false,
  "deployment claims unrestricted approval",
);
assert(
  betaObserved.httpsRoute?.origin === "https://cloud-test.makodb.com",
  "HTTPS gate has the wrong origin",
);
assert(
  betaObserved.httpsRoute?.trustedCertificateVerified === true,
  "trusted certificate qualification is missing",
);
assert(
  betaObserved.httpsRoute?.publicAdmissionEnabled === false,
  "risk-accepted preview must not be recorded as qualified-beta admission",
);
assert(betaPreview.label === "risk-accepted-public-preview", "preview label is invalid");
assert(betaPreview.qualifiedBetaStatus === "blocked", "preview claims a passing beta gate");
assert(
  betaObserved.publicPreview?.status === betaPreview.status,
  "preview status differs from evidence",
);
assert(
  betaObserved.publicPreview?.qualifiedBetaStatus === "blocked",
  "release gate treats preview as qualified beta",
);
assert(betaPreview.planHash === betaPlan.planHash, "preview evidence uses another plan");
assert(
  betaPreview.releaseDigest === betaManifest.releaseDigest,
  "preview evidence uses another release",
);
if (betaPreview.status === "active") {
  assert(betaPreview.admissionMode === "risk_accepted_preview", "active preview mode is wrong");
  assert(
    betaObserved.httpsRoute?.publicAdmissionMode === "risk_accepted_preview",
    "release gate omits active preview admission",
  );
  assert(
    betaPreview.approvalEvidence && betaPreview.guardEvidence,
    "active preview omits approval or guard evidence",
  );
  const previewApproval = await readJson(betaPreview.approvalEvidence);
  assert(
    previewApproval.kind === "risk-accepted-public-preview",
    "preview approval kind is invalid",
  );
  assert(previewApproval.planHash === betaPlan.planHash, "preview approval uses another plan");
  assert(
    previewApproval.releaseDigest === betaManifest.releaseDigest,
    "preview approval uses another release",
  );
  assert(
    JSON.stringify([...previewApproval.blockers].sort()) ===
      JSON.stringify([...beta.blockers].sort()),
    "preview approval does not name every active blocker",
  );
  assert(previewApproval.schemaVersion === 2, "preview approval schema is stale");
  assert(
    previewApproval.persistence === "until-manually-paused" && previewApproval.expiresAt === null,
    "preview approval is not persistent until manually paused",
  );
} else {
  assert(betaPreview.status === "not-enabled", "preview status is invalid");
  assert(betaPreview.admissionMode === "pre_gate", "inactive preview is not source restricted");
  assert(
    betaObserved.httpsRoute?.publicAdmissionMode === "pre_gate",
    "inactive preview gate has the wrong admission mode",
  );
  assert(betaPreview.approvalEvidence === null, "inactive preview retains an approval");
}
assert(
  betaStorage.storageContract?.topology === "mixed-local-sqlite-rocksdb",
  "beta storage topology is wrong",
);
assert(
  betaStorage.storageContract?.controlPlane?.engine === "sqlite" &&
    betaStorage.storageContract?.controlPlane?.authority === "control" &&
    betaStorage.storageContract?.dataPlane?.engine === "rocksdb" &&
    betaStorage.storageContract?.dataPlane?.authority === "tenant" &&
    betaStorage.storageContract?.edgeGateway?.engine === "rocksdb" &&
    betaStorage.storageContract?.telemetryQuery?.engine === "rocksdb",
  "beta storage engine ownership is wrong",
);
assert(
  new Set([
    betaStorage.storageContract?.controlPlane?.path,
    betaStorage.storageContract?.dataPlane?.path,
    betaStorage.storageContract?.edgeGateway?.path,
    betaStorage.storageContract?.telemetryQuery?.path,
  ]).size === 4,
  "beta storage paths are not isolated",
);
assert(
  betaStorage.storageContract?.durability === "synchronous-fsync",
  "beta storage durability evidence is wrong",
);
assert(
  betaStorage.backup?.authenticatedWriteVerified === true,
  "off-VM backup write is unverified",
);
assert(
  betaStorage.backup?.controlSqlite?.remotelyReverified === true &&
    betaStorage.backup?.tenantRocksDb?.remotelyReverified === true,
  "mixed storage backup evidence is incomplete",
);
assert(
  betaObserved.deploymentStorageQualification?.passed === true,
  "beta storage qualification is absent",
);
assert(
  betaObserved.deploymentStorageQualification?.synchronousDurability === true,
  "beta storage durability is unverified",
);
assert(betaRecovery.status === "passed", "beta recovery drill did not pass");
assert(
  betaRollback.invariants?.allFourServicesReady === true &&
    betaRollback.operations?.some((operation) => operation.kind === "rollback"),
  "beta release rollback did not pass",
);
assert(betaObserved.recoveryDrill?.passed === true, "beta gate omits the recovery drill");
assert(
  betaMeasurement.vmId === 124 && betaMeasurement.planHash === betaPlan.planHash,
  "measurement window identity is wrong",
);
assert(
  betaMeasurement.releaseDigest === betaManifest.releaseDigest,
  "measurement window uses another release",
);
assert(
  betaObserved.measurementWindow?.status === betaMeasurement.status,
  "measurement status differs from evidence",
);
for (const field of ["startedAt", "expectedCompleteAt", "completedAt", "completeDays"]) {
  assert(
    betaObserved.measurementWindow?.[field] === betaMeasurement[field],
    `measurement ${field} differs from evidence`,
  );
}
assert(betaMeasurement.completedAt === null, "an incomplete 30-day window is marked complete");
assert(
  Object.values(betaMeasurement.observed ?? {}).some((value) => value === null),
  "unproven measurement values were inferred",
);
assert(
  betaStreaming.passed === true &&
    betaStreaming.releaseDigest === betaManifest.releaseDigest &&
    betaStreaming.planHash === betaPlan.planHash &&
    betaStreaming.external?.https?.tlsVerified === true &&
    betaStreaming.external?.https?.hsts,
  "trusted HTTPS and streaming qualification is stale or incomplete",
);
assert(
  betaBenchmark.passed === true &&
    betaBenchmark.releaseDigest === betaManifest.releaseDigest &&
    betaBenchmark.planHash === betaPlan.planHash,
  "hosted benchmark is stale or incomplete",
);
assert(
  betaBenchmark.workload?.acknowledgedWriteLoss === beta.thresholds.acknowledgedWriteLoss &&
    betaBenchmark.workload?.integrityFailures === beta.thresholds.integrityFailures,
  "hosted benchmark lost acknowledged data or found integrity failures",
);
assert(
  betaBenchmark.target?.capacityHeadroomAtTargetLoadPercent >=
    beta.thresholds.minimumCapacityHeadroomPercent &&
    betaObserved.capacityHeadroomPercent ===
      betaBenchmark.target?.capacityHeadroomAtTargetLoadPercent &&
    betaBenchmark.concurrentLoad?.target?.failedRequests === 0 &&
    betaBenchmark.concurrentLoad?.requiredHeadroom?.failedRequests === 0,
  "hosted benchmark does not prove required target-load headroom",
);
const measuredLatency = {
  auth: betaBenchmark.latencyMilliseconds?.authSignIn?.p95,
  pull: Math.max(
    betaBenchmark.latencyMilliseconds?.rxdbPullBeforeRecovery,
    betaBenchmark.latencyMilliseconds?.rxdbPullAfterRecovery,
  ),
  push: betaBenchmark.latencyMilliseconds?.rxdbPushBatch?.p95,
  liveDelivery: betaBenchmark.latencyMilliseconds?.rxdbLiveFirstEvent,
  functionWarm: betaBenchmark.latencyMilliseconds?.warmFunction,
  functionCold: betaBenchmark.latencyMilliseconds?.coldFunction,
};
for (const [name, value] of Object.entries(measuredLatency)) {
  assert(
    betaObserved.endToEndLatency?.[name] === value,
    `beta ${name} latency differs from hosted evidence`,
  );
}
assert(
  betaObserved.developerRegistrationAndMail?.status === "passed" &&
    betaObserved.developerRegistrationAndMail?.ordinarySelfServiceLifecycleVerified === true &&
    betaObserved.developerRegistrationAndMail?.authenticatedTransactionalMailVerified === true &&
    betaObserved.developerRegistrationAndMail?.certificateExpiryAlertDelivered === true &&
    betaMail.status === "passed" &&
    betaMail.operatorAlertDelivery?.certificateExpiryFixtureDelivered === true &&
    betaDeveloperRegistration.status === "passed",
  "beta developer registration or authenticated mail evidence is incomplete",
);
assert(
  beta.blockers?.length === 2 &&
    beta.blockers.some((blocker) => blocker.includes("latency exceed")) &&
    beta.blockers.some((blocker) => blocker.includes("30-day")) &&
    !beta.blockers.some((blocker) => blocker.includes("certificate-expiry")),
  "blocked beta gate must retain the two unresolved environment blockers only",
);

const multiRegion = requireStage("multi-region", "unsupported");
assertNonNegativeThresholds(multiRegion.thresholds, "multi-region");
assert(
  multiRegion.observed?.replicatedStorageBackend === null,
  "multi-region cannot name a replicated backend while local RocksDB is the production topology",
);
assert(
  multiRegion.blockers?.some((blocker) => blocker.includes("local RocksDB")),
  "multi-region must identify the single-owner RocksDB blocker",
);

console.log(
  "validated internal release pass, blocked single-region beta, and unsupported multi-region gate",
);

function requireStage(id, status) {
  const stage = stages.get(id);
  assert(stage !== undefined, `missing ${id} release stage`);
  assert(stage.status === status, `${id} must be ${status}`);
  return stage;
}

function assertNonNegativeThresholds(value, context) {
  for (const [key, entry] of Object.entries(value ?? {})) {
    if (typeof entry === "object") assertNonNegativeThresholds(entry, `${context}.${key}`);
    else assert(Number.isFinite(entry) && entry >= 0, `${context}.${key} must be non-negative`);
  }
}

function assertAtMost(observed, maximum, name) {
  assert(Number.isFinite(observed) && observed <= maximum, `${name} exceeds its threshold`);
}

async function readJson(path) {
  assert(typeof path === "string" && path !== "", "JSON evidence path is required");
  return JSON.parse(await readFile(resolve(root, path), "utf8"));
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
