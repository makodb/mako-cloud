import assert from "node:assert/strict";
import test from "node:test";

import {
  BrowserAuthSessionPersistence,
  MakoAuthClient,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

const scope = { projectId: "prj_abcdefgh", environmentId: "env_abcdefgh" };
const session = {
  accessToken: "access-one",
  refreshToken: "refresh-one",
  expiresAtUnixMilliseconds: 5_000,
  user: { id: "usr_abcdefgh", email: "user@example.test", status: "active", authorizationEpoch: 1 },
};

function fakeStorage(entries = new Map()) {
  return {
    entries,
    getItem: (key) => entries.get(key) ?? null,
    setItem: (key, value) => void entries.set(key, String(value)),
    removeItem: (key) => void entries.delete(key),
  };
}

test("round-trips a session through storage under the scoped key and clears it", async () => {
  const storage = fakeStorage();
  const persistence = new BrowserAuthSessionPersistence(scope, { storage });
  assert.equal(persistence.key, "mako.auth.session.prj_abcdefgh.env_abcdefgh");
  assert.equal(persistence.durable, true);
  assert.equal(await persistence.load(), null);
  await persistence.save(session);
  assert.deepEqual(JSON.parse(storage.entries.get(persistence.key)), session);

  const reloaded = new BrowserAuthSessionPersistence(scope, { storage });
  assert.deepEqual(await reloaded.load(), session);
  await reloaded.clear();
  assert.equal(storage.entries.size, 0);
  assert.equal(await persistence.load(), null);

  const custom = new BrowserAuthSessionPersistence(scope, { storage, key: "app.session" });
  await custom.save(session);
  assert.equal(storage.entries.has("app.session"), true);
});

test("drops a malformed stored value instead of trusting it", async () => {
  const storage = fakeStorage(
    new Map([["mako.auth.session.prj_abcdefgh.env_abcdefgh", '{"accessToken":"x"}']]),
  );
  const persistence = new BrowserAuthSessionPersistence(scope, { storage });
  assert.equal(await persistence.load(), null);
  assert.equal(storage.entries.size, 0);
  storage.entries.set(persistence.key, "not json");
  assert.equal(await persistence.load(), null);
});

test("falls back to memory and never throws when storage is unavailable", async () => {
  const persistence = new BrowserAuthSessionPersistence(scope, { storage: null });
  assert.equal(persistence.durable, false);
  await persistence.save(session);
  assert.deepEqual(await persistence.load(), session);
  await persistence.clear();
  assert.equal(await persistence.load(), null);

  const failing = {
    getItem: () => {
      throw new Error("SecurityError");
    },
    setItem: () => {
      throw new Error("QuotaExceededError");
    },
    removeItem: () => {
      throw new Error("SecurityError");
    },
  };
  const degraded = new BrowserAuthSessionPersistence(scope, { storage: failing });
  assert.equal(await degraded.load(), null);
  assert.equal(degraded.durable, false);
  const quota = new BrowserAuthSessionPersistence(scope, { storage: failing });
  await quota.save(session);
  assert.equal(quota.durable, false);
  assert.deepEqual(await quota.load(), session);
  await quota.clear();
  assert.equal(await quota.load(), null);
});

test("uses globalThis.localStorage when present and degrades when it is absent", async () => {
  const storage = fakeStorage();
  globalThis.localStorage = storage;
  try {
    const persistence = new BrowserAuthSessionPersistence(scope);
    assert.equal(persistence.durable, true);
    await persistence.save(session);
    assert.equal(storage.entries.has(persistence.key), true);
  } finally {
    delete globalThis.localStorage;
  }
  const withoutStorage = new BrowserAuthSessionPersistence(scope);
  assert.equal(withoutStorage.durable, false);
  await withoutStorage.save(session);
  assert.deepEqual(await withoutStorage.load(), session);
});

test("restores an auth client session from browser storage across reloads", async () => {
  const storage = fakeStorage();
  const fetch = async () =>
    Response.json({ accessToken: "a", refreshToken: "r", expiresIn: 600, user: session.user });
  const first = new MakoAuthClient(config(), {
    fetch,
    persistence: new BrowserAuthSessionPersistence(scope, { storage }),
    now: () => 0,
  });
  await first.signInWithPassword("user@example.test", "password");
  const second = new MakoAuthClient(config(), {
    fetch,
    persistence: new BrowserAuthSessionPersistence(scope, { storage }),
    now: () => 0,
  });
  assert.equal((await second.restoreSession()).accessToken, "a");
  assert.equal(second.authenticationRequired, false);
});

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: scope.projectId,
    environmentId: scope.environmentId,
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "browser",
  });
}
