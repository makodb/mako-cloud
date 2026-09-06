import assert from "node:assert/strict";
import { test } from "node:test";

import {
  counted,
  isAdjustment,
  isCounted,
  isHidden,
  isTransferLeg,
} from "../functions/shared/exclusions.ts";

/**
 * One definition of what counts as money moving. The tests are about the
 * edges of that definition, because every total in the app leans on it.
 */

test("a plain transaction counts; a transfer leg, a balance update, or a hidden one does not", () => {
  assert.equal(isCounted({}), true);
  assert.equal(isCounted({ hidden: false, adjustment: false, transfer_id: "" }), true);
  assert.equal(isCounted({ transfer_id: "tr_1" }), false);
  assert.equal(isCounted({ adjustment: true }), false);
  assert.equal(isCounted({ hidden: true }), false);
});

test("each exclusion has its own name, and an empty transfer id is no pairing", () => {
  assert.equal(isTransferLeg({ transfer_id: "tr_1" }), true);
  assert.equal(isTransferLeg({ transfer_id: "" }), false);
  assert.equal(isTransferLeg({}), false);
  assert.equal(isAdjustment({ adjustment: true }), true);
  assert.equal(isAdjustment({}), false);
  assert.equal(isHidden({ hidden: true }), true);
  assert.equal(isHidden({}), false);
});

test("counted keeps the order and drops only what is excluded", () => {
  const list = [
    { id: "a" },
    { id: "b", transfer_id: "tr_1" },
    { id: "c", hidden: true },
    { id: "d", adjustment: true },
    { id: "e" },
  ];
  assert.deepEqual(
    counted(list).map((entry) => entry.id),
    ["a", "e"],
  );
});
