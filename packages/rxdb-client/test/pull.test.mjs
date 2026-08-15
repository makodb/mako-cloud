import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoReplicationError,
  createMakoPullHandler,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

test("maps RxDB checkpoints and bounded batches onto authenticated pulls", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    requests.push({ input: String(input), init });
    return Response.json({
      documents: [{ id: "todo-1", value: 1, _deleted: false, _rev: "r1" }],
      checkpoint: "mcp1.next-checkpoint",
    });
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  const result = await createMakoPullHandler(config(), auth, { fetch })(
    { token: "mcp1.previous" },
    500,
  );
  assert.equal(result.documents[0].id, "todo-1");
  assert.deepEqual(result.checkpoint, { token: "mcp1.next-checkpoint" });
  const body = JSON.parse(requests[0].init.body);
  assert.deepEqual(body, {
    checkpoint: "mcp1.previous",
    schemaVersion: 3,
    batchSize: 25,
  });
  assert.equal(requests[0].init.headers.Authorization, "Bearer access-token");
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
});

test("maps stable server failures without leaking arbitrary bodies", async () => {
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    return Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "schema_mismatch",
          message: "migration required",
          requestId: "req_schema",
          retry: { kind: "never" },
          details: { requiredSchemaVersion: "4" },
        },
      },
      { status: 409 },
    );
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(
    () => createMakoPullHandler(config(), auth, { fetch })(undefined, 25),
    (error) =>
      error instanceof MakoReplicationError &&
      error.code === "schema_mismatch" &&
      error.retryable === false &&
      error.requestId === "req_schema",
  );
});

function signedInAuth(fetch) {
  return new MakoAuthClient(config(), { fetch });
}

function session() {
  return {
    accessToken: "access-token",
    refreshToken: "refresh-token-value-that-is-long-enough",
    expiresIn: 300,
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
    schemaVersion: 3,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
    pullBatchSize: 25,
  });
}
