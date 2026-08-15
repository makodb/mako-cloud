import assert from "node:assert/strict";
import test from "node:test";

import {
  MANAGEMENT_OPERATIONS,
  OPERATOR_OPERATIONS,
} from "../../../packages/management-sdk/dist/index.js";
import {
  CONSOLE_MANAGEMENT_OPERATIONS,
  CONSOLE_OPERATOR_OPERATIONS,
} from "../dist/index.js";

test("console management inventory is exactly the public SDK inventory", () => {
  assert.deepEqual(CONSOLE_MANAGEMENT_OPERATIONS, MANAGEMENT_OPERATIONS);
});

test("operator console inventory is exactly the separate operator SDK inventory", () => {
  assert.deepEqual(CONSOLE_OPERATOR_OPERATIONS, OPERATOR_OPERATIONS);
});
