#!/usr/bin/env node

import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const report = await readJson("docs/rollback-qualification.json");
const releaseGates = await readJson("docs/release-gates.json");
const hostedBeta = await readJson("docs/evidence/public-beta-release-rollback.json");
const guide = await readFile(resolve(root, "docs/rollback-qualification.md"), "utf8");
const runner = await readFile(resolve(root, report.runner ?? ""), "utf8");
const required = ["service", "policy", "function", "schema", "signing-key", "storage-adapter"];

assert(report.schemaVersion === 1, "rollback qualification schemaVersion must be 1");
assert(/^\d{4}-\d{2}-\d{2}$/.test(report.recordedAt), "recordedAt must be an ISO date");
assert(report.scope === "local-automated", "the report must identify its local automated scope");
assert(report.externalBetaEligible === false, "local rollback evidence cannot enable beta traffic");
assert(report.drills?.length === required.length, "exactly six rollback drills are required");

const drills = new Map(report.drills.map((drill) => [drill.id, drill]));
for (const id of required) {
  const drill = drills.get(id);
  assert(drill !== undefined, `missing ${id} rollback drill`);
  assert(drill.status === "pass", `${id} rollback drill must pass`);
  assert(drill.procedure?.length > 40, `${id} rollback procedure is incomplete`);
  assert(guide.includes(`## ${heading(id)}`), `guide is missing the ${id} procedure`);
  assert(drill.evidence?.length > 0, `${id} rollback evidence is missing`);
  for (const reference of drill.evidence) {
    const separator = reference.indexOf("::");
    const path = separator === -1 ? reference : reference.slice(0, separator);
    const testName = separator === -1 ? undefined : reference.slice(separator + 2);
    await access(resolve(root, path));
    if (testName !== undefined) {
      const source = await readFile(resolve(root, path), "utf8");
      assert(
        source.includes(testName.split("::").at(-1)),
        `${id} evidence test is absent from ${path}`,
      );
    }
  }
}

for (const commandFragment of [
  "exercise-service-rollback.js",
  "retryable_failure_is_compensated_and_can_resume_idempotently",
  "draft_validation_testing_activation_and_rollback_report_epochs",
  "deploy_promote_rollback_test_logs_and_delete_follow_safe_lifecycle",
  "compatible_publication_succeeds_and_incompatible_change_requires_migration",
  "encrypted_keys_rotate_with_jwks_overlap_and_retirement",
  "format_compatible_previous_binary_rollback_uses_the_same_nonempty_volume",
]) {
  assert(runner.includes(commandFragment), `rollback runner omits ${commandFragment}`);
}

const beta = releaseGates.stages?.find((stage) => stage.id === "single-region-beta");
assert(beta?.status === "blocked", "single-region beta must remain blocked after local drills");
assert(
  beta.observed?.recoveryDrill?.releaseRollbackVerified === true &&
    hostedBeta.vmId === 124 &&
    hostedBeta.invariants?.allFourServicesReady === true &&
    hostedBeta.operations?.some((operation) => operation.kind === "rollback"),
  "beta must bind its completed operator-observed rollback drill",
);
assert(
  !beta.blockers?.some((blocker) => blocker.includes("operator-observed")),
  "completed operator-observed rollback remains incorrectly blocked",
);

console.log(
  "validated 6 local rollback drills and the hosted operator drill; beta remains blocked",
);

function heading(id) {
  return {
    service: "Service",
    policy: "Policy",
    function: "Function",
    schema: "Schema",
    "signing-key": "Signing key",
    "storage-adapter": "Storage adapter",
  }[id];
}

async function readJson(path) {
  assert(typeof path === "string" && path !== "", "JSON evidence path is required");
  return JSON.parse(await readFile(resolve(root, path), "utf8"));
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
