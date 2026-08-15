import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoReplicationError,
  MakoReplicationRecoveryCoordinator,
} from "../dist/node/index.js";

test("surfaces migration and full-resync states through application hooks", async () => {
  const events = [];
  const recovery = new MakoReplicationRecoveryCoordinator({
    pauseReplication: async () => events.push("pause"),
    onSchemaMigrationRequired: async (state) =>
      events.push(`schema:${state.requiredSchemaVersion}`),
    onFullResyncRequired: async (state) => events.push(`resync:${state.reason}`),
  });
  await recovery.handleError(
    new MakoReplicationError({
      code: "schema_mismatch",
      message: "migration required",
      requestId: "req_schema",
      retry: { kind: "never" },
      details: { requiredSchemaVersion: "4" },
    }),
  );
  assert.deepEqual(recovery.state, {
    kind: "schema_migration_required",
    requiredSchemaVersion: 4,
  });
  await recovery.handleResyncReason("checkpoint_expired");
  assert.deepEqual(recovery.state, {
    kind: "full_resync_required",
    reason: "checkpoint_expired",
  });
  assert.deepEqual(events, ["pause", "schema:4", "pause", "resync:checkpoint_expired"]);
});
