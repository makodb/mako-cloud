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

test("a restart that finds moved epochs clears without an open handle", async () => {
  // The person was away while their access changed, so the reset happens
  // before this run opened anything. Clearing through a collection handle
  // would be a no-op here and the previous generation's documents would
  // survive the restart -- the one thing a security reset exists to prevent.
  const contexts = [];
  const persistence = new MemoryReplicationSecurityStatePersistence();
  const hooks = (label) => ({
    pauseReplication: async () => contexts.push(`${label}:pause`),
    clearReplicatedCollection: async (context) => {
      contexts.push(`${label}:clear:running=${context.replicationRunning}`);
    },
    onSecurityReset: async () => contexts.push(`${label}:notify`),
    startReplication: async () => contexts.push(`${label}:start`),
  });
  const first = new MakoAuthorizationEpochCoordinator("scope", hooks("first"), { persistence });
  await first.initialize({ environment: 1, user: 1 });
  assert.deepEqual(contexts, [], "a first run has nothing to clear");

  // A new process, the same durable state, epochs that moved meanwhile.
  const second = new MakoAuthorizationEpochCoordinator("scope", hooks("second"), { persistence });
  const state = await second.initialize({ environment: 1, user: 4 });
  assert.deepEqual(contexts, [
    "second:clear:running=false",
    "second:notify",
  ]);
  assert.equal(state.generation, 1);
  assert.notEqual(state.replicationIdentifier, "scope");
});

test("two resets of one scope never overlap", async () => {
  // A live `authorization_epoch_changed` and the app's own epoch sync after a
  // write arrive together routinely. Overlapping resets clear a database the
  // other is replicating into, which RxDB reports as DB8.
  let clearing = 0;
  let overlapped = false;
  const settle = () => new Promise((resolve) => setTimeout(resolve, 5));
  const coordinator = new MakoAuthorizationEpochCoordinator(
    "scope",
    {
      pauseReplication: settle,
      clearReplicatedCollection: async () => {
        clearing += 1;
        if (clearing > 1) overlapped = true;
        await settle();
        clearing -= 1;
      },
      onSecurityReset: settle,
      startReplication: settle,
    },
    { persistence: new MemoryReplicationSecurityStatePersistence() },
  );
  await coordinator.initialize({ environment: 1, user: 1 });

  const results = await Promise.all([
    coordinator.handleMismatch({ environment: 2, user: 1 }),
    coordinator.handleMismatch({ environment: 2, user: 1 }),
    coordinator.handleMismatch({ environment: 2, user: 1 }),
  ]);
  assert.equal(overlapped, false, "resets must not overlap");
  // One reset, and every caller is told the same generation: a second reset
  // would hand two of them different replication identifiers.
  assert.deepEqual(
    results.map((result) => result.generation),
    [1, 1, 1],
  );
  assert.equal(new Set(results.map((result) => result.replicationIdentifier)).size, 1);
});

test("an initialize racing a mismatch resets once", async () => {
  // `open()` and a live epoch event are not ordered with respect to each
  // other; before the queue, initialize took no part in the coalescing and
  // the two ran their clears concurrently.
  let clearing = 0;
  let overlapped = false;
  const settle = () => new Promise((resolve) => setTimeout(resolve, 5));
  const persistence = new MemoryReplicationSecurityStatePersistence();
  await persistence.save({
    authorizationEpochs: { environment: 1, user: 1 },
    replicationIdentifier: "scope",
    generation: 0,
  });
  const coordinator = new MakoAuthorizationEpochCoordinator(
    "scope",
    {
      pauseReplication: settle,
      clearReplicatedCollection: async () => {
        clearing += 1;
        if (clearing > 1) overlapped = true;
        await settle();
        clearing -= 1;
      },
      onSecurityReset: settle,
      startReplication: settle,
    },
    { persistence },
  );
  const [initialized, mismatched] = await Promise.all([
    coordinator.initialize({ environment: 3, user: 1 }),
    coordinator.handleMismatch({ environment: 3, user: 1 }),
  ]);
  assert.equal(overlapped, false, "initialize and a mismatch must not both reset");
  assert.equal(initialized.replicationIdentifier, mismatched.replicationIdentifier);
  assert.equal(initialized.generation, 1);
});
