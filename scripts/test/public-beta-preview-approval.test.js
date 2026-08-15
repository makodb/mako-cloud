import assert from "node:assert/strict";
import test from "node:test";

import {
  buildApproval,
  computeBlockerDigest,
  requiredConfirmation,
  validateApproval,
} from "../public-beta-preview-approval-lib.js";

const planHash = "a".repeat(64);
const releaseDigest = "b".repeat(64);
const blockers = [
  "provide authenticated SMTP relay credentials and prove certificate-expiry alert delivery",
  "measured auth and RxDB latency exceed the single-region beta budgets",
  "complete the 30-day capacity, availability, and cost window",
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
  assert.throws(
    () => validApproval({ blockers: [...blockers, "acknowledged writes were lost"] }),
    /non-waivable blocker/,
  );
});
