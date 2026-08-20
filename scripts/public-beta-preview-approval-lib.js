import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

export const PREVIEW_KIND = "risk-accepted-public-preview";
export const PREVIEW_PERSISTENCE = "until-manually-paused";

const digestPattern = /^[a-f0-9]{64}$/;
const waivableBlockerPatterns = [
  /authenticated SMTP relay|certificate-expiry alert delivery/i,
  /latency exceed/i,
  /30-day|capacity, availability, and cost window|provider or facility evidence/i,
];
const safeguardNames = [
  "trustedHttpsAndHsts",
  "exactPublicRouteAllowlist",
  "applicationSecurity",
  "backupAndRecovery",
  "zeroAcknowledgedWriteLoss",
  "zeroIntegrityFailures",
  "serviceReadiness",
  "emergencyAdmissionStop",
];

export function normalizeBlockers(blockers) {
  assert(Array.isArray(blockers) && blockers.length > 0, "at least one active blocker is required");
  const normalized = [...new Set(blockers.map((value) => String(value).trim()))].sort();
  assert(
    normalized.every((value) => value.length > 0),
    "blockers cannot be empty",
  );
  assert(
    normalized.every((value) => waivableBlockerPatterns.some((pattern) => pattern.test(value))),
    "the active blocker set contains a non-waivable blocker",
  );
  return normalized;
}

export function computeBlockerDigest(blockers) {
  return sha256(stableStringify(normalizeBlockers(blockers)));
}

export function requiredConfirmation({ releaseDigest, planHash, blockerDigest }) {
  for (const [name, value] of Object.entries({ releaseDigest, planHash, blockerDigest })) {
    assert(digestPattern.test(value), `${name} must be a lowercase SHA-256 digest`);
  }
  return `ACCEPT_PERSISTENT_PUBLIC_PREVIEW_RISK:${releaseDigest}:${planHash}:${blockerDigest}`;
}

export function buildApproval({
  operatorIdentity,
  planHash,
  releaseDigest,
  blockers,
  acceptedAt,
  nonWaivableSafeguards,
  typedConfirmation,
}) {
  const normalizedBlockers = normalizeBlockers(blockers);
  const blockerDigest = computeBlockerDigest(normalizedBlockers);
  const approval = {
    schemaVersion: 2,
    kind: PREVIEW_KIND,
    persistence: PREVIEW_PERSISTENCE,
    operatorIdentity,
    planHash,
    releaseDigest,
    blockers: normalizedBlockers,
    blockerDigest,
    acceptedAt,
    expiresAt: null,
    nonWaivableSafeguards,
    typedConfirmation,
  };
  validateApproval(approval, {
    expectedPlanHash: planHash,
    expectedReleaseDigest: releaseDigest,
    expectedBlockers: normalizedBlockers,
    now: acceptedAt,
  });
  return approval;
}

export function validateApproval(
  approval,
  { expectedPlanHash, expectedReleaseDigest, expectedBlockers, now = new Date().toISOString() },
) {
  assert(
    approval && typeof approval === "object" && !Array.isArray(approval),
    "approval is required",
  );
  assert(approval.schemaVersion === 2, "approval schemaVersion must be 2");
  assert(approval.kind === PREVIEW_KIND, "approval kind is invalid");
  assert(approval.persistence === PREVIEW_PERSISTENCE, "approval persistence is invalid");
  assert(approval.expiresAt === null, "persistent approval must not expire");
  assertSafeIdentity(approval.operatorIdentity);
  assert(digestPattern.test(approval.planHash), "approval planHash is invalid");
  assert(digestPattern.test(approval.releaseDigest), "approval releaseDigest is invalid");
  assert(approval.planHash === expectedPlanHash, "approval plan binding does not match");
  assert(
    approval.releaseDigest === expectedReleaseDigest,
    "approval release binding does not match",
  );

  const blockers = normalizeBlockers(approval.blockers);
  const currentBlockers = normalizeBlockers(expectedBlockers);
  assert(
    stableStringify(blockers) === stableStringify(currentBlockers),
    "approval blocker set changed",
  );
  const blockerDigest = computeBlockerDigest(blockers);
  assert(approval.blockerDigest === blockerDigest, "approval blocker digest is invalid");

  const accepted = parseTimestamp(approval.acceptedAt, "acceptedAt");
  const evaluated = parseTimestamp(now, "now");
  assert(evaluated >= accepted, "approval is not active yet");

  assertSafeguards(approval.nonWaivableSafeguards);
  const expectedConfirmation = requiredConfirmation({
    releaseDigest: approval.releaseDigest,
    planHash: approval.planHash,
    blockerDigest,
  });
  assert(approval.typedConfirmation === expectedConfirmation, "typed confirmation does not match");
  return approval;
}

export function derivePreviewQualification(repositoryRoot) {
  const readJson = (path) => JSON.parse(readFileSync(resolve(repositoryRoot, path), "utf8"));
  const gates = readJson("docs/release-gates.json");
  const plan = readJson(gates.sources.publicBetaPlan);
  const manifest = readJson(gates.sources.publicBetaReleaseManifest);
  const deployment = readJson(gates.sources.publicBetaDeployment);
  const streaming = readJson(gates.sources.publicBetaStreamingQualification);
  const benchmark = readJson(gates.sources.publicBetaHostedBenchmark);
  const hosted = readJson("docs/evidence/public-beta-hosted-qualification.json");
  const admissionStop = readJson("docs/evidence/public-beta-admission-stop.json");
  const beta = gates.stages.find((stage) => stage.id === "single-region-beta");

  assert(beta?.status === "blocked", "risk acceptance is only valid for a blocked beta gate");
  const blockers = normalizeBlockers(beta.blockers);
  assert(manifest.planHash === plan.planHash, "release manifest uses another plan");
  assert(beta.observed?.deployment?.planHash === plan.planHash, "release gate uses another plan");

  // An approval attests that the non-waivable safeguards were measured, and on
  // which release. It is not scoped to the release that happens to be deployed
  // now: admission stopped being release-bound, so requiring the evidence to
  // describe the current build would reimpose that binding here and make every
  // redeploy an outage again.
  //
  // What still has to hold is that the evidence is mutually coherent -- every
  // artifact describing one and the same release, not a mix of runs.
  const measuredRelease = beta.observed?.deployment?.releaseDigest;
  assert(digestPattern.test(measuredRelease ?? ""), "release gate records no measured release");
  assert(deployment.release?.digest === measuredRelease, "deployment evidence is stale");
  assert(streaming.releaseDigest === measuredRelease, "streaming evidence is stale");
  assert(benchmark.releaseDigest === measuredRelease, "benchmark evidence is stale");
  assert(hosted.releaseDigest === measuredRelease, "hosted security evidence is stale");

  const safeguards = {
    trustedHttpsAndHsts:
      streaming.passed === true &&
      streaming.external?.https?.tlsVerified === true &&
      typeof streaming.external?.https?.hsts === "string" &&
      streaming.external.https.hsts.includes("max-age="),
    exactPublicRouteAllowlist:
      streaming.external?.internalRouteStatus === 404 &&
      streaming.external?.unknownRouteStatus === 404,
    applicationSecurity: hosted.passed === true,
    backupAndRecovery:
      beta.observed?.deploymentStorageQualification?.offVmBackupVerified === true &&
      beta.observed?.recoveryDrill?.passed === true &&
      beta.observed?.recoveryDrill?.backupVerified === true,
    zeroAcknowledgedWriteLoss: benchmark.workload?.acknowledgedWriteLoss === 0,
    zeroIntegrityFailures: benchmark.workload?.integrityFailures === 0,
    serviceReadiness:
      benchmark.passed === true && benchmark.concurrentLoad?.requiredHeadroom?.failedRequests === 0,
    emergencyAdmissionStop:
      admissionStop.status === "passed" &&
      admissionStop.proxmoxGate?.enabled === true &&
      admissionStop.proxmoxGate?.active === true,
  };
  assertSafeguards(safeguards);

  return {
    planHash: plan.planHash,
    releaseDigest: measuredRelease,
    blockers,
    blockerDigest: computeBlockerDigest(blockers),
    nonWaivableSafeguards: safeguards,
  };
}

export function stableStringify(value) {
  if (Array.isArray(value)) return `[${value.map((item) => stableStringify(item)).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function assertSafeguards(safeguards) {
  assert(
    safeguards && typeof safeguards === "object" && !Array.isArray(safeguards),
    "non-waivable safeguards are required",
  );
  for (const name of safeguardNames) {
    assert(safeguards[name] === true, `non-waivable safeguard failed: ${name}`);
  }
}

function assertSafeIdentity(value) {
  assert(typeof value === "string", "operator identity is required");
  assert(value.length >= 3 && value.length <= 254, "operator identity length is invalid");
  assert(!/[\r\n\0]/.test(value), "operator identity contains control characters");
}

function parseTimestamp(value, name) {
  assert(typeof value === "string" && value.endsWith("Z"), `${name} must be a UTC timestamp`);
  const parsed = Date.parse(value);
  assert(Number.isFinite(parsed), `${name} is invalid`);
  return parsed;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
