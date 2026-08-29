import assert from "node:assert/strict";
import test from "node:test";

import {
  DexieReplicationStatePersistence,
  DexieReplicationStateStore,
  MakoAuthClient,
  MakoAuthorizationEpochCoordinator,
  MakoLivePullStream,
  MakoReplicationError,
  MakoReplicationRecoveryCoordinator,
  MakoRxdbConfigurationError,
  MemoryReplicationStateStore,
  createMakoPullHandler,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

const scope = { projectId: "prj_abcdefgh", environmentId: "env_abcdefgh", collectionId: "todos" };

function fakeFetch(streamUrls) {
  return async (input, init = {}) => {
    const url = String(input);
    if (url.endsWith("/auth/signin")) {
      return Response.json({
        accessToken: "access-token",
        refreshToken: "refresh-token",
        expiresIn: 600,
        user: { id: "usr_abcdefgh", email: "u@example.test", status: "active", authorizationEpoch: 1 },
      });
    }
    if (url.includes("/replication/pull")) {
      const body = JSON.parse(init.body);
      return Response.json({
        documents: [{ id: `doc-after-${body.checkpoint ?? "start"}`, _deleted: false }],
        checkpoint: `mcp1.after-${body.checkpoint ?? "start"}`,
      });
    }
    if (url.includes("/replication/stream")) {
      streamUrls.push(url);
      return new Promise(() => undefined); // an open stream that never answers
    }
    throw new Error(`unexpected request ${url}`);
  };
}

async function hooks(actions, prefix) {
  return {
    pauseReplication: async () => actions.push(`${prefix}pause`),
    clearReplicatedCollection: async () => actions.push(`${prefix}clear`),
    onSecurityReset: async (event) => actions.push(`${prefix}notify:${event.replicationIdentifier}`),
    startReplication: async (identifier) => actions.push(`${prefix}start:${identifier}`),
  };
}

test("namespaces state per collection and validates what it loads", async () => {
  const store = new MemoryReplicationStateStore();
  const durable = new DexieReplicationStatePersistence(scope, { store });
  assert.equal(durable.namespace, "mako:prj_abcdefgh:env_abcdefgh:todos");
  assert.equal(await durable.checkpoint.load(), null);
  assert.equal(await durable.security.load(), null);
  assert.equal(await durable.recovery.load(), null);

  await store.set(`${durable.namespace}:checkpoint`, { token: 42 });
  await store.set(`${durable.namespace}:security`, { generation: -1 });
  await store.set(`${durable.namespace}:recovery`, { kind: "unknown" });
  assert.equal(await durable.checkpoint.load(), null);
  assert.equal(await durable.security.load(), null);
  assert.equal(await durable.recovery.load(), null);

  const other = new DexieReplicationStatePersistence({ ...scope, collectionId: "accounts" }, { store });
  await other.checkpoint.save({ token: "mcp1.accounts" });
  assert.equal(await durable.checkpoint.load(), null);
  assert.deepEqual(await other.checkpoint.load(), { token: "mcp1.accounts" });
  await other.clear();
  assert.deepEqual(
    store.keys().filter((key) => key.includes(":accounts:")),
    [],
  );
});

test("a restart resumes from the persisted checkpoint and security generation", async () => {
  const store = new MemoryReplicationStateStore();
  const streamUrls = [];
  const fetch = fakeFetch(streamUrls);
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("u@example.test", "password");

  // First run: pull twice, then the live stream advances the checkpoint once more.
  const firstRun = new DexieReplicationStatePersistence(scope, { store });
  const actions = [];
  const security = new MakoAuthorizationEpochCoordinator("todos", await hooks(actions, ""), {
    persistence: firstRun.security,
    identifierFactory: (base, generation) => `${base}:security-${generation}:fixed`,
  });
  const initial = await security.initialize({ environment: 1, user: 1 });
  assert.deepEqual(initial, {
    authorizationEpochs: { environment: 1, user: 1 },
    replicationIdentifier: "todos",
    generation: 0,
  });
  const reset = await security.handleMismatch({ environment: 2, user: 1 });
  assert.equal(reset.generation, 1);
  assert.deepEqual(actions, [
    "pause",
    "clear",
    "notify:todos:security-1:fixed",
    "start:todos:security-1:fixed",
  ]);

  const pull = createMakoPullHandler(config(), auth, { fetch, checkpoints: firstRun.checkpoint });
  const first = await pull(undefined, 100);
  assert.deepEqual(first.checkpoint, { token: "mcp1.after-start" });
  assert.deepEqual(await firstRun.checkpoint.load(), { token: "mcp1.after-start" });
  const second = await pull(first.checkpoint, 100);
  assert.deepEqual(await firstRun.checkpoint.load(), second.checkpoint);

  // Second run over the same store: nothing is pulled from the beginning.
  const secondRun = new DexieReplicationStatePersistence(scope, { store });
  const resumed = new MakoAuthorizationEpochCoordinator("todos", await hooks(actions, "again:"), {
    persistence: secondRun.security,
  });
  const resumedState = await resumed.initialize({ environment: 2, user: 1 });
  assert.deepEqual(resumedState, reset, "the persisted generation and identifier are reused");
  assert.equal(actions.length, 4, "resuming under the same epochs performs no reset");

  const checkpoint = await secondRun.checkpoint.load();
  assert.deepEqual(checkpoint, { token: "mcp1.after-mcp1.after-start" });
  const live = new MakoLivePullStream(config(), auth, {
    fetch,
    checkpoints: secondRun.checkpoint,
    reconnectMinimumDelayMs: 10,
  });
  live.start(checkpoint ?? undefined);
  await new Promise((resolve) => setTimeout(resolve, 20));
  live.close();
  assert.equal(streamUrls.length, 1);
  assert.equal(
    new URL(streamUrls[0]).searchParams.get("checkpoint"),
    "mcp1.after-mcp1.after-start",
    "the live stream resumes from the persisted checkpoint",
  );
});

test("a security reset clears the persisted checkpoint and recovery state", async () => {
  const store = new MemoryReplicationStateStore();
  const durable = new DexieReplicationStatePersistence(scope, { store });
  await durable.checkpoint.save({ token: "mcp1.stale" });
  const recovery = new MakoReplicationRecoveryCoordinator(
    {
      pauseReplication: async () => undefined,
      onSchemaMigrationRequired: async () => undefined,
      onFullResyncRequired: async () => undefined,
    },
    { persistence: durable.recovery },
  );
  await recovery.handleResyncReason("stream_gap");
  assert.deepEqual(await durable.recovery.load(), { kind: "full_resync_required", reason: "stream_gap" });

  const actions = [];
  const security = new MakoAuthorizationEpochCoordinator("todos", await hooks(actions, ""), {
    persistence: durable.security,
    identifierFactory: (base, generation) => `${base}:g${generation}`,
  });
  await security.initialize({ environment: 1, user: 1 });
  const reset = await security.handleMismatch({ environment: 1, user: 2 });
  assert.equal(await durable.checkpoint.load(), null, "the checkpoint is cleared");
  assert.equal(await durable.recovery.load(), null, "the recovery state is cleared");
  assert.deepEqual(await durable.security.load(), reset, "the new generation is persisted");
  assert.deepEqual(actions, ["pause", "clear", "notify:todos:g1", "start:todos:g1"]);
  assert.deepEqual(store.keys(), [`${durable.namespace}:security`]);
});

test("a restart under changed epochs clears durable data before adopting them", async () => {
  const store = new MemoryReplicationStateStore();
  const durable = new DexieReplicationStatePersistence(scope, { store });
  await durable.checkpoint.save({ token: "mcp1.old-authorization" });
  await durable.security.save({
    authorizationEpochs: { environment: 1, user: 1 },
    replicationIdentifier: "todos",
    generation: 0,
  });
  const actions = [];
  const security = new MakoAuthorizationEpochCoordinator("todos", await hooks(actions, ""), {
    persistence: durable.security,
    identifierFactory: (base, generation) => `${base}:g${generation}`,
  });
  const state = await security.initialize({ environment: 1, user: 3 });
  assert.deepEqual(state, {
    authorizationEpochs: { environment: 1, user: 3 },
    replicationIdentifier: "todos:g1",
    generation: 1,
  });
  assert.deepEqual(
    actions,
    ["clear", "notify:todos:g1"],
    "nothing runs yet, so there is nothing to pause and the caller starts replication",
  );
  assert.equal(await durable.checkpoint.load(), null);
  assert.deepEqual(await durable.security.load(), state);
});

test("recovery state survives a restart and markActive persists the return to normal", async () => {
  const store = new MemoryReplicationStateStore();
  const durable = new DexieReplicationStatePersistence(scope, { store });
  const events = [];
  const hooksFor = (prefix) => ({
    pauseReplication: async () => events.push(`${prefix}pause`),
    onSchemaMigrationRequired: async (state) =>
      events.push(`${prefix}schema:${state.requiredSchemaVersion}`),
    onFullResyncRequired: async (state) => events.push(`${prefix}resync:${state.reason}`),
  });
  const first = new MakoReplicationRecoveryCoordinator(hooksFor(""), { persistence: durable.recovery });
  assert.deepEqual(await first.initialize(), { kind: "active" });
  await first.handleError(
    new MakoReplicationError({
      code: "schema_mismatch",
      message: "migration required",
      requestId: "req_schema",
      retry: { kind: "never" },
      details: { requiredSchemaVersion: 4 },
    }),
  );
  const second = new MakoReplicationRecoveryCoordinator(hooksFor("again:"), {
    persistence: durable.recovery,
  });
  assert.deepEqual(await second.initialize(), {
    kind: "schema_migration_required",
    requiredSchemaVersion: 4,
  });
  assert.deepEqual(events, ["pause", "schema:4"], "restoring does not re-fire hooks");
  await second.markActive();
  assert.deepEqual(await durable.recovery.load(), { kind: "active" });
  const third = new MakoReplicationRecoveryCoordinator(hooksFor("third:"), {
    persistence: durable.recovery,
  });
  assert.deepEqual(await third.initialize(), { kind: "active" });
});

test("the Dexie store fails closed without IndexedDB and accepts an injected implementation", () => {
  assert.throws(() => new DexieReplicationStateStore(), MakoRxdbConfigurationError);
  assert.throws(() => new DexieReplicationStatePersistence(scope), MakoRxdbConfigurationError);
  const store = new DexieReplicationStateStore({
    databaseName: "test-state",
    indexedDB: { open: () => undefined, cmp: () => 0 },
    IDBKeyRange: { bound: () => undefined, lowerBound: () => undefined, upperBound: () => undefined, only: () => undefined },
  });
  assert.equal(store.databaseName, "test-state");
  store.close();
});

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    ...scope,
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "browser",
  });
}
