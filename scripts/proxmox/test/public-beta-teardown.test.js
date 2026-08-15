import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { buildTeardownPlan } from "../public-beta-teardown-lib.js";

const fixture = JSON.parse(
  await readFile(new URL("./fixtures/public-beta-teardown.json", import.meta.url), "utf8"),
);

test("teardown planning is confirmation-bound and non-executing", () => {
  const plan = buildTeardownPlan(fixture, "2026-08-09T00:00:00Z");
  assert.match(plan.planHash, /^[0-9a-f]{64}$/);
  assert.equal(plan.executionSupported, false);
  assert.equal(plan.vm.id, 124);
  assert.equal(plan.admission.edgeFilterInstalled, true);
  assert.equal(Object.keys(plan.requiredTypedConfirmations).length, 5);
  for (const confirmation of Object.values(plan.requiredTypedConfirmations)) {
    assert.ok(confirmation.endsWith(plan.planHash));
  }
});

test("teardown planning refuses to precede the admission stop", () => {
  assert.throws(
    () =>
      buildTeardownPlan({ ...fixture, admission: { ...fixture.admission, serviceActive: false } }),
    /admission stop must be active/,
  );
});
