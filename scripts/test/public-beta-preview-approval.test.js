import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";

import { evaluateHostedBenchmark } from "../public-beta-benchmark-gate-lib.js";
import {
  buildApproval,
  computeBlockerDigest,
  derivePreviewQualification,
  requiredConfirmation,
  validateApproval,
} from "../public-beta-preview-approval-lib.js";

const root = resolve(import.meta.dirname, "../..");

const planHash = "a".repeat(64);
const releaseDigest = "b".repeat(64);
const blockers = [
  "provide authenticated SMTP relay credentials and prove certificate-expiry alert delivery",
  "auth sign-in p95 latency exceeds its 200 ms single-region beta budget: 631 ms over 100 steady-state samples",
  "RxDB push batch p95 latency is unmeasured: 4 of 100 required steady-state samples",
];
const acceptedAt = "2026-08-09T08:00:00.000Z";
const safeguards = {
  trustedHttpsAndHsts: true,
  exactPublicRouteAllowlist: true,
  applicationSecurity: true,
  backupAndRecovery: true,
  zeroAcknowledgedWriteLoss: true,
  zeroIntegrityFailures: true,
  serviceReadiness: true,
  emergencyAdmissionStop: true,
};

function validApproval(overrides = {}) {
  const blockerDigest = computeBlockerDigest(blockers);
  return buildApproval({
    operatorIdentity: "operator@example.com",
    planHash,
    releaseDigest,
    blockers,
    acceptedAt,
    nonWaivableSafeguards: safeguards,
    typedConfirmation: requiredConfirmation({ releaseDigest, planHash, blockerDigest }),
    ...overrides,
  });
}

test("valid persistent approval binds operator, release, plan, and blockers", () => {
  const approval = validApproval();
  assert.equal(approval.persistence, "until-manually-paused");
  assert.equal(approval.expiresAt, null);
  assert.equal(
    validateApproval(approval, {
      expectedPlanHash: planHash,
      expectedReleaseDigest: releaseDigest,
      expectedBlockers: blockers,
      now: "2026-08-10T00:00:00.000Z",
    }),
    approval,
  );
});

test("persistent approval remains valid in the future and rejects an expiry", () => {
  const approval = validApproval();
  assert.equal(
    validateApproval(approval, {
      expectedPlanHash: planHash,
      expectedReleaseDigest: releaseDigest,
      expectedBlockers: blockers,
      now: "2126-08-23T08:00:00.000Z",
    }),
    approval,
  );
  assert.throws(
    () =>
      validateApproval(
        { ...approval, expiresAt: "2126-08-23T08:00:00.000Z" },
        {
          expectedPlanHash: planHash,
          expectedReleaseDigest: releaseDigest,
          expectedBlockers: blockers,
          now: acceptedAt,
        },
      ),
    /must not expire/,
  );
});

test("approval rejects release, plan, blocker, persistence, and confirmation drift", () => {
  const approval = validApproval();
  const common = {
    expectedPlanHash: planHash,
    expectedReleaseDigest: releaseDigest,
    expectedBlockers: blockers,
  };
  assert.throws(
    () =>
      validateApproval(
        { ...approval, persistence: "calendar-expiring" },
        { ...common, now: acceptedAt },
      ),
    /persistence/,
  );
  assert.throws(
    () =>
      validateApproval(approval, {
        ...common,
        expectedReleaseDigest: "c".repeat(64),
        now: acceptedAt,
      }),
    /release binding/,
  );
  assert.throws(
    () =>
      validateApproval(approval, {
        ...common,
        expectedPlanHash: "c".repeat(64),
        now: acceptedAt,
      }),
    /plan binding/,
  );
  assert.throws(
    () =>
      validateApproval(approval, {
        ...common,
        expectedBlockers: [...blockers, "another latency exceeds threshold"],
        now: acceptedAt,
      }),
    /blocker set changed/,
  );
  assert.throws(
    () =>
      validateApproval({ ...approval, typedConfirmation: "yes" }, { ...common, now: acceptedAt }),
    /typed confirmation/,
  );
});

test("approval rejects every failed non-waivable safeguard", () => {
  for (const name of Object.keys(safeguards)) {
    assert.throws(
      () => validApproval({ nonWaivableSafeguards: { ...safeguards, [name]: false } }),
      new RegExp(name),
    );
  }
});

test("approval rejects blocker categories that cannot be waived", () => {
  for (const blocker of [
    "acknowledged writes were lost",
    "hosted benchmark lost 1 acknowledged writes",
    "hosted benchmark found 1 integrity failures",
    "hosted benchmark failed 2 requests at target concurrency",
    "hosted benchmark capacity headroom at target load is 10 percent, below the required 30 percent",
    // The 30-day window and cost are no longer gate requirements, so they cannot be accepted.
    "complete the 30-day resource, capacity, availability, and cost window with provider or facility evidence",
    "record provider or facility cost evidence",
  ]) {
    assert.throws(
      () => validApproval({ blockers: [...blockers, blocker] }),
      /non-waivable blocker/,
    );
  }
});

test("preview qualification accepts exactly the blockers the hosted benchmark computes", () => {
  const gates = JSON.parse(readFileSync(resolve(root, "docs/release-gates.json"), "utf8"));
  const beta = gates.stages.find((stage) => stage.id === "single-region-beta");
  const benchmark = JSON.parse(
    readFileSync(resolve(root, gates.sources.publicBetaHostedBenchmark), "utf8"),
  );
  const gate = evaluateHostedBenchmark(benchmark, beta.thresholds);
  const qualification = derivePreviewQualification(root);
  assert.deepEqual(qualification.blockers, [...gate.blockers].sort());
  assert.equal(qualification.blockerDigest, computeBlockerDigest(gate.blockers));
  for (const blocker of qualification.blockers) {
    assert.doesNotMatch(blocker, /30-day|cost|backup age|recovery-point/i);
  }
  assert.equal(qualification.nonWaivableSafeguards.backupAndRecovery, true);
});
