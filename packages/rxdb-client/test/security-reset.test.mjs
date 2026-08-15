import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthorizationEpochCoordinator,
  MemoryReplicationSecurityStatePersistence,
} from "../dist/node/index.js";

test("pauses, securely clears, notifies, and restarts under a new identifier", async () => {
  const actions = [];
  const persistence = new MemoryReplicationSecurityStatePersistence();
  const coordinator = new MakoAuthorizationEpochCoordinator(
    "todos-user-replication",
    {
      pauseReplication: async () => actions.push("pause"),
      clearReplicatedCollection: async () => actions.push("clear"),
      onSecurityReset: async (event) => actions.push(`notify:${event.replicationIdentifier}`),
      startReplication: async (identifier) => actions.push(`start:${identifier}`),
    },
    {
      persistence,
      identifierFactory: (base, generation) => `${base}:security-${generation}:fixed`,
    },
  );
  await coordinator.initialize({ environment: 1, user: 2 });
  const reset = await coordinator.handleMismatch({ environment: 2, user: 2 });
  assert.equal(reset.replicationIdentifier, "todos-user-replication:security-1:fixed");
  assert.deepEqual(actions, [
    "pause",
    "clear",
    "notify:todos-user-replication:security-1:fixed",
    "start:todos-user-replication:security-1:fixed",
  ]);
  assert.deepEqual(await persistence.load(), reset);

  await coordinator.handleMismatch({ environment: 2, user: 2 });
  assert.equal(actions.length, 4, "the same epoch must not reset twice");
});
