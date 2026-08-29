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

test("retries rather than signing out when the session refresh is unavailable", async () => {
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json({ ...session(), expiresIn: 1 });
    }
    throw new TypeError("fetch failed");
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(
    () => createMakoPullHandler(config(), auth, { fetch })(undefined, 25),
    (error) =>
      error instanceof MakoReplicationError &&
      error.code === "unavailable" &&
      error.retryable === true,
  );
  assert.equal(auth.authenticationRequired, false);
  assert.equal(auth.refreshUnavailable, true);
});

test("renews the session once when a pull is refused, then repeats the request", async () => {
  const counts = { pull: 0, token: 0 };
  const bearers = [];
  const fetch = async (input, init) => {
    const url = String(input);
    if (url.endsWith("/auth/signin")) {
      return Response.json(session());
    }
    if (url.endsWith("/auth/token")) {
      counts.token += 1;
      return Response.json({ ...session(), accessToken: "renewed-access-token" });
    }
    counts.pull += 1;
    bearers.push(init.headers.Authorization);
    return counts.pull === 1
      ? refusedSession()
      : Response.json({ documents: [], checkpoint: "mcp1.after-renewal" });
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  const result = await createMakoPullHandler(config(), auth, { fetch })(undefined, 25);
  assert.deepEqual(result.checkpoint, { token: "mcp1.after-renewal" });
  assert.deepEqual(counts, { pull: 2, token: 1 });
  assert.deepEqual(bearers, ["Bearer access-token", "Bearer renewed-access-token"]);
  assert.equal(auth.authenticationRequired, false);
});

test("a session the service keeps refusing is terminal and bounded, never a retry storm", async () => {
  const counts = { pull: 0, token: 0 };
  const fetch = async (input) => {
    const url = String(input);
    if (url.endsWith("/auth/signin")) {
      return Response.json(session());
    }
    if (url.endsWith("/auth/token")) {
      counts.token += 1;
      return Response.json({ ...session(), accessToken: "renewed-access-token" });
    }
    counts.pull += 1;
    // The second refusal even advises an immediate retry: a refused credential
    // is terminal regardless, or the client hammers the service with it.
    return counts.pull === 1 ? refusedSession() : refusedSession({ kind: "immediate" });
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  const handler = createMakoPullHandler(config(), auth, { fetch });
  const isTerminalRefusal = (error) =>
    error instanceof MakoReplicationError &&
    error.code === "unauthenticated" &&
    error.retryable === false &&
    error.status === 401;
  await assert.rejects(() => handler(undefined, 25), isTerminalRefusal);
  assert.deepEqual(counts, { pull: 2, token: 1 });
  // RxDB repeats a failed pull on its own schedule; every repeat must cost one
  // refused request and no further renewal.
  await assert.rejects(() => handler(undefined, 25), isTerminalRefusal);
  await assert.rejects(() => handler(undefined, 25), isTerminalRefusal);
  assert.deepEqual(counts, { pull: 4, token: 1 });
});

test("carries an after_delay advice through to the error so RxDB can back off", async () => {
  const counts = { pull: 0, token: 0 };
  const fetch = async (input) => {
    const url = String(input);
    if (url.endsWith("/auth/signin")) {
      return Response.json(session());
    }
    if (url.endsWith("/auth/token")) {
      counts.token += 1;
      return Response.json(session());
    }
    counts.pull += 1;
    return Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "unavailable",
          message: "the collection is temporarily unavailable",
          requestId: "req_503",
          retry: { kind: "after_delay", afterMs: 2_500 },
        },
      },
      { status: 503 },
    );
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(
    () => createMakoPullHandler(config(), auth, { fetch })(undefined, 25),
    (error) =>
      error instanceof MakoReplicationError &&
      error.code === "unavailable" &&
      error.retryable === true &&
      error.retryAfterMilliseconds === 2_500 &&
      error.extensions.retryAfterMilliseconds === 2_500,
  );
  assert.deepEqual(counts, { pull: 1, token: 0 });
});

test("honors retry.kind never on a 500 despite the retryable status", async () => {
  const fetch = async (input) => {
    if (String(input).endsWith("/auth/signin")) {
      return Response.json(session());
    }
    return Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "internal",
          message: "the request cannot be served",
          requestId: "req_500",
          retry: { kind: "never" },
        },
      },
      { status: 500 },
    );
  };
  const auth = signedInAuth(fetch);
  await auth.signInWithPassword("user@example.test", "password");
  await assert.rejects(
    () => createMakoPullHandler(config(), auth, { fetch })(undefined, 25),
    (error) =>
      error instanceof MakoReplicationError &&
      error.code === "internal" &&
      error.status === 500 &&
      error.retryable === false &&
      error.retryAfterMilliseconds === null,
  );
});

/** A `401` the way the data plane answers one, with the advice it carries. */
function refusedSession(retry = { kind: "never" }) {
  return Response.json(
    {
      apiVersion: "v1",
      error: {
        code: "unauthenticated",
        message: "the access token is not valid",
        requestId: "req_401",
        retry,
      },
    },
    { status: 401 },
  );
}

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
