import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
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
