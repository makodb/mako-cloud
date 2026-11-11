#!/usr/bin/env node

import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { requireMarkdownTarget } from "./markdown-anchors-lib.js";
import { evaluateHostedBenchmark, GATED_LATENCY_PATHS } from "./public-beta-benchmark-gate-lib.js";

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
for (const evidence of internal.securityEvidence ?? []) {
  await requireMarkdownTarget(root, evidence, "internal security evidence");
}
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

// The single-region beta gate is decided by the hosted benchmark alone. A calendar measurement
// window, backup age, recovery-point exposure and cost are not gate inputs.
const beta = requireStage("single-region-beta", ["passed", "blocked"]);
assertNonNegativeThresholds(beta.thresholds, "single-region beta");
const betaObserved = beta.observed;
for (const field of [
  "maximumBackupAgeSeconds",
  "maximumRecoveryPointExposureSeconds",
  "maximumMonthlyFixedInfrastructureUsd",
  "maximumCostPerMillionReplicationOperationsUsd",
]) {
  assert(!(field in beta.thresholds), `beta gate must not require ${field}`);
}
for (const field of [
  "measurementWindow",
  "backupAgeSeconds",
  "recoveryPointExposureSeconds",
  "monthlyFixedInfrastructureUsd",
  "costPerMillionReplicationOperationsUsd",
]) {
  assert(!(field in betaObserved), `beta gate must not observe ${field}`);
}

// The recorded result is the computed one: passed exactly when no benchmark blocker remains, and
// blocked with exactly the failing or unmeasured benchmark paths otherwise. Alert delivery is
// asserted as qualified below, so an alert-delivery blocker would be stale.
const benchmarkGate = evaluateHostedBenchmark(betaBenchmark, beta.thresholds);
assert(
  beta.status === benchmarkGate.status,
  `single-region beta is recorded as ${beta.status} but its hosted benchmark is ${benchmarkGate.status}`,
);
assert(
  Array.isArray(beta.blockers) &&
    JSON.stringify([...beta.blockers].sort()) ===
      JSON.stringify([...benchmarkGate.blockers].sort()),
  `single-region beta blockers differ from the hosted benchmark; expected ${JSON.stringify(
    benchmarkGate.blockers,
    null,
    2,
  )}`,
);

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
assert(
  betaPreview.qualifiedBetaStatus === benchmarkGate.status,
  "preview evidence records another beta gate result",
);
assert(
  betaObserved.publicPreview?.status === betaPreview.status,
  "preview status differs from evidence",
);
assert(
  betaObserved.publicPreview?.qualifiedBetaStatus === benchmarkGate.status,
  "release gate records another beta gate result for preview",
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
  betaObserved.capacityHeadroomPercent ===
    betaBenchmark.target?.capacityHeadroomAtTargetLoadPercent,
  "beta capacity headroom differs from hosted evidence",
);
for (const { path } of GATED_LATENCY_PATHS) {
  const measured = benchmarkGate.latency[path];
  assert(
    betaObserved.endToEndLatency?.[path] === measured.p95,
    `beta ${path} latency differs from hosted evidence`,
  );
  assert(
    betaObserved.latencySamples?.[path] === measured.samples,
    `beta ${path} sample count differs from hosted evidence`,
  );
}
assert(
  betaObserved.onlineBackupsCompletedDuringRun === benchmarkGate.onlineBackupsCompleted,
  "beta online backup count differs from hosted evidence",
);
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
  `validated internal release pass, ${beta.status} single-region beta, and unsupported multi-region gate`,
);

function requireStage(id, statuses) {
  const allowed = Array.isArray(statuses) ? statuses : [statuses];
  const stage = stages.get(id);
  assert(stage !== undefined, `missing ${id} release stage`);
  assert(allowed.includes(stage.status), `${id} must be ${allowed.join(" or ")}`);
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
