import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoReplicationError,
  createMakoPushHandler,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

test("uses stable mutation IDs and maps readable conflicts", async () => {
  const pushes = [];
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    const body = JSON.parse(init.body);
    pushes.push({ body, headers: init.headers });
    return Response.json({
      outcomes: body.rows.map((row, index) =>
        index === 0
          ? { mutationId: row.mutationId, status: "accepted" }
          : {
              mutationId: row.mutationId,
              status: "conflict",
              masterState: { id: "todo-2", value: 9, _deleted: false },
            },
      ),
    });
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  const handler = createMakoPushHandler(config(), auth, { fetch });
  const rows = [
    { newDocumentState: { id: "todo-1", value: 1, _deleted: false } },
    {
      assumedMasterState: { id: "todo-2", value: 1, _deleted: false },
      newDocumentState: { id: "todo-2", value: 2, _deleted: false },
    },
  ];
  const conflicts = await handler(rows);
  await handler(rows);
  assert.equal(conflicts[0].value, 9);
  assert.deepEqual(
    pushes[0].body.rows.map((row) => row.mutationId),
    pushes[1].body.rows.map((row) => row.mutationId),
  );
  assert.equal(pushes[0].headers["Idempotency-Key"], pushes[1].headers["Idempotency-Key"]);
});

test("surfaces denied rows as non-retryable policy errors", async () => {
  const fetch = async (input, init) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    const row = JSON.parse(init.body).rows[0];
    return Response.json({
      outcomes: [
        {
          mutationId: row.mutationId,
          status: "denied",
          error: {
            apiVersion: "v1",
            error: {
              code: "permission_denied",
              message: "not permitted",
              requestId: "req_policy",
              retry: { kind: "never" },
            },
          },
        },
      ],
    });
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(
    () =>
      createMakoPushHandler(config(), auth, { fetch })([
        { newDocumentState: { id: "todo-1", value: 1, _deleted: false } },
      ]),
    (error) =>
      error instanceof MakoReplicationError &&
      error.code === "permission_denied" &&
      error.retryable === false,
  );
});

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
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
    pushBatchSize: 10,
  });
}
