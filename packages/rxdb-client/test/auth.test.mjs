import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MemoryAuthSessionPersistence,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

const user = {
  id: "usr_abcdefgh",
  email: "user@example.test",
  status: "active",
  authorizationEpoch: 1,
};

test("signs in, rotates refresh state, restores, and signs out without service credentials", async () => {
  const requests = [];
  const responses = [
    Response.json({ accessToken: "access-one", refreshToken: "refresh-one", expiresIn: 60, user }),
    Response.json({ accessToken: "access-two", refreshToken: "refresh-two", expiresIn: 90, user }),
    new Response(null, { status: 204 }),
  ];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return responses.shift();
  };
  const persistence = new MemoryAuthSessionPersistence();
  const auth = new MakoAuthClient(config(), { persistence, fetch, now: () => 1_000 });
  const signedIn = await auth.signInWithPassword("user@example.test", "password-123");
  assert.equal(signedIn.accessToken, "access-one");
  assert.equal(signedIn.refreshToken, undefined);
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(JSON.stringify(requests).includes("mako_sk"), false);

  const refreshed = await auth.refreshSession();
  assert.equal(refreshed.accessToken, "access-two");
  assert.equal(JSON.parse(requests[1].init.body).refreshToken, "refresh-one");
  const restored = new MakoAuthClient(config(), { persistence, fetch });
  assert.equal((await restored.restoreSession()).accessToken, "access-two");
  await restored.signOut();
  assert.equal(requests[2].init.headers.Authorization, "Bearer access-two");
  assert.equal(await persistence.load(), null);
});

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
