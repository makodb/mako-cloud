import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoAuthError,
  MakoAuthenticationRequiredError,
  MemoryAuthSessionPersistence,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

test("automatically rotates an expiring access token once for concurrent callers", async () => {
  let refreshes = 0;
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session("old-access", "old-refresh", 1));
    }
    refreshes += 1;
    return Response.json(session("new-access", "new-refresh", 300));
  };
  const auth = new MakoAuthClient(config(), { fetch, now: () => 1_000 });
  await auth.signInWithPassword("user@example.test", "password");
  assert.deepEqual(await Promise.all([auth.validAccessToken(), auth.validAccessToken()]), [
    "new-access",
    "new-access",
  ]);
  assert.equal(refreshes, 1);
  assert.equal(auth.authenticationRequired, false);
});

test("clears revoked or unavailable refresh state and requires authentication", async () => {
  const persistence = new MemoryAuthSessionPersistence();
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session("old-access", "old-refresh", 1));
    }
    return Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "unauthenticated",
          message: "session revoked",
          requestId: "req_revoked",
          retry: { kind: "never" },
        },
      },
      { status: 401 },
    );
  };
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 1_000 });
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(() => auth.validAccessToken(), MakoAuthenticationRequiredError);
  assert.equal(auth.authenticationRequired, true);
  assert.equal(auth.accessToken(), null);
  assert.equal(await persistence.load(), null);
});

test("coalesces concurrent refreshes across entry points and starts a new one afterwards", async () => {
  const refreshBodies = [];
  let released;
  const gate = new Promise((resolve) => {
    released = resolve;
  });
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session("access-0", "refresh-0", 1));
    }
    refreshBodies.push(JSON.parse(init.body).refreshToken);
    const attempt = refreshBodies.length;
    if (attempt === 1) {
      await gate;
    }
    return Response.json(session(`access-${attempt}`, `refresh-${attempt}`, 300));
  };
  const auth = new MakoAuthClient(config(), { fetch, now: () => 1_000 });
  await auth.signInWithPassword("user@example.test", "password");

  const concurrent = Promise.all([auth.refreshSession(), auth.validAccessToken()]);
  released();
  const [refreshed, token] = await concurrent;
  assert.deepEqual(refreshBodies, ["refresh-0"]);
  assert.equal(refreshed.accessToken, "access-1");
  assert.equal(token, "access-1");
  assert.equal(refreshed.user.id, auth.currentSession().user.id);

  const later = await auth.refreshSession();
  assert.deepEqual(refreshBodies, ["refresh-0", "refresh-1"]);
  assert.equal(later.accessToken, "access-2");
});

test("keeps the persisted session when a refresh cannot reach the service", async () => {
  const persistence = new MemoryAuthSessionPersistence();
  const events = [];
  let offline = true;
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session("old-access", "old-refresh", 1));
    }
    if (offline) {
      throw new TypeError("fetch failed");
    }
    return Response.json(session("new-access", "new-refresh", 300));
  };
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 1_000 });
  const unsubscribe = auth.subscribe((event) => events.push(event.kind));
  await auth.signInWithPassword("user@example.test", "password");

  await assert.rejects(
    () => auth.refreshSession(),
    (error) =>
      error instanceof MakoAuthError &&
      !(error instanceof MakoAuthenticationRequiredError) &&
      error.code === "refresh_unavailable" &&
      error.retryable === true,
  );
  assert.equal(auth.refreshUnavailable, true);
  assert.equal(auth.authenticationRequired, false);
  assert.equal(auth.currentSession().accessToken, "old-access");
  assert.equal((await persistence.load()).refreshToken, "old-refresh");

  offline = false;
  assert.equal((await auth.refreshSession()).accessToken, "new-access");
  assert.equal(auth.refreshUnavailable, false);
  assert.deepEqual(events, ["session", "refresh_unavailable", "session", "refresh_recovered"]);

  unsubscribe();
  await auth.refreshSession();
  assert.equal(events.length, 4);
});

test("clears the session only when the service definitively refuses the credential", async () => {
  const persistence = new MemoryAuthSessionPersistence();
  const events = [];
  const responses = [
    () => Response.json(session("old-access", "old-refresh", 300)),
    () =>
      apiError(503, "unavailable", "session refresh is unavailable", {
        kind: "after_delay",
        afterMs: 1_000,
      }),
    () =>
      apiError(429, "rate_limited", "authentication request rate exceeded", {
        kind: "after_delay",
        afterMs: 500,
      }),
    () =>
      apiError(401, "unauthenticated", "refresh credential replay revoked the session", {
        kind: "never",
      }),
  ];
  const fetch = async () => responses.shift()();
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 1_000 });
  auth.subscribe((event) => events.push(event.kind));
  await auth.signInWithPassword("user@example.test", "password");

  for (const _transient of [503, 429]) {
    await assert.rejects(
      () => auth.refreshSession(),
      (error) => error.code === "refresh_unavailable" && error.retryable === true,
    );
    assert.equal(auth.refreshUnavailable, true);
    assert.notEqual(await persistence.load(), null);
  }

  await assert.rejects(() => auth.refreshSession(), MakoAuthenticationRequiredError);
  assert.equal(auth.authenticationRequired, true);
  assert.equal(auth.refreshUnavailable, false);
  assert.equal(auth.currentSession(), null);
  assert.equal(await persistence.load(), null);
  assert.deepEqual(events, ["session", "refresh_unavailable", "signed_out"]);
});

test("serves a still-valid token while refresh is unavailable and stays signed in when it expires", async () => {
  const persistence = new MemoryAuthSessionPersistence();
  let clock = 1_000;
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session("old-access", "old-refresh", 60));
    }
    throw new TypeError("fetch failed");
  };
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => clock });
  await auth.signInWithPassword("user@example.test", "password");

  clock = 40_000;
  assert.equal(await auth.validAccessToken(), "old-access");
  assert.equal(auth.refreshUnavailable, true);
  assert.equal(auth.authenticationRequired, false);

  clock = 120_000;
  await assert.rejects(
    () => auth.validAccessToken(),
    (error) =>
      error instanceof MakoAuthError &&
      !(error instanceof MakoAuthenticationRequiredError) &&
      error.retryable === true,
  );
  assert.equal(auth.authenticationRequired, false);
  assert.equal((await persistence.load()).refreshToken, "old-refresh");
});

test("an offline restart keeps the restored session instead of signing the user out", async () => {
  const persistence = new MemoryAuthSessionPersistence();
  await persistence.save({
    accessToken: "stale-access",
    refreshToken: "stored-refresh",
    expiresAtUnixMilliseconds: 500,
    user: {
      id: "usr_abcdefgh",
      email: "user@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  });
  const fetch = async () => {
    throw new TypeError("fetch failed");
  };
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 1_000 });
  assert.equal((await auth.restoreSession()).user.id, "usr_abcdefgh");
  await assert.rejects(
    () => auth.validAccessToken(),
    (error) => error.code === "refresh_unavailable" && error.retryable === true,
  );
  assert.equal(auth.authenticationRequired, false);
  assert.equal(auth.refreshUnavailable, true);
  assert.equal(auth.currentSession().accessToken, "stale-access");
  assert.equal((await persistence.load()).refreshToken, "stored-refresh");
});

function apiError(status, code, message, retry) {
  return Response.json(
    { apiVersion: "v1", error: { code, message, requestId: "req_test", retry } },
    { status },
  );
}

function session(accessToken, refreshToken, expiresIn) {
  return {
    accessToken,
    refreshToken,
    expiresIn,
    user: {
      id: "usr_abcdefgh",
      email: "user@example.test",
      status: "active",
      authorizationEpoch: 1,
    },
  };
}

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  });
}
