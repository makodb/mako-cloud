import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoCallerIdentityRequiredError,
  MakoEdgeSdkError,
  createFunctionClient,
  createFunctionClientFromRequest,
  createServiceClient,
} from "../dist/index.js";

const CALLER_TOKEN = "verified.caller.token.value";
const SERVICE_CREDENTIAL = `mako_sk.service_edge.${"a".repeat(64)}`;

test("auth and document clients automatically propagate the verified caller", async () => {
  const requests = [];
  const client = createFunctionClient({
    endpoint: "https://api.example.test/internal",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    callerAuthorization: CALLER_TOKEN,
    requestId: "req_example00",
    fetch: async (request) => {
      requests.push(request);
      if (request.url.endsWith("/auth/user")) {
        return Response.json(user());
      }
      if (request.url.endsWith("/auth/signout")) {
        return new Response(null, { status: 204 });
      }
      if (request.url.endsWith("/documents/query")) {
        return Response.json({ documents: [document()], nextCursor: null });
      }
      if (request.method === "POST") {
        return Response.json({
          mutationId: "mutation_example00",
          status: "applied",
          document: document(),
          currentRevision: null,
        });
      }
      return Response.json(document());
    },
  });

  assert.deepEqual(await client.auth.getUser(), user());
  const todos = client.documents("todos");
  assert.deepEqual(await todos.get("todo/one"), document());
  assert.deepEqual(
    await todos.query({
      predicates: [{ field: "ownerId", operator: "eq", value: "usr_abcdefgh" }],
      sort: [{ field: "ownerId", direction: "asc" }],
      cursor: null,
      limit: 25,
    }),
    { documents: [document()], nextCursor: null },
  );
  await todos.mutate("todo/one", {
    mutationId: "mutation_example00",
    operation: "update",
    expectedRevision: "rev_example00",
    schemaVersion: 1,
    body: { ownerId: "usr_abcdefgh", title: "Updated" },
  });
  await client.auth.signOut();

  assert.equal(requests.length, 5);
  for (const request of requests) {
    assert.equal(request.headers.get("authorization"), `Bearer ${CALLER_TOKEN}`);
    assert.equal(request.headers.get("x-mako-request-id"), "req_example00");
    assert.equal(request.headers.has("x-mako-key"), false);
  }
  assert.equal(
    requests[1].url,
    "https://api.example.test/internal/v1/projects/prj_abcdefgh/environments/env_abcdefgh/collections/todos/documents/todo%2Fone",
  );
  assert.equal(requests[3].headers.get("idempotency-key"), "mutation_example00");
  assert.equal(JSON.stringify(client).includes(CALLER_TOKEN), false);
});

test("public invocations fail closed before auth or document access", async () => {
  let called = false;
  const client = createFunctionClient({
    endpoint: "http://localhost:8787",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    callerAuthorization: null,
    fetch: async () => {
      called = true;
      throw new Error("must not run");
    },
  });

  await assert.rejects(() => client.auth.getUser(), MakoCallerIdentityRequiredError);
  await assert.rejects(
    () => client.documents("todos").get("todo-one"),
    MakoCallerIdentityRequiredError,
  );
  assert.equal(called, false);
});

test("the runtime request helper consumes only trusted caller and correlation headers", async () => {
  const requests = [];
  const runtimeRequest = new Request("https://function.example.test/todos", {
    headers: {
      "x-mako-caller-authorization": CALLER_TOKEN,
      "x-mako-request-id": "req_runtime00",
    },
  });
  const client = createFunctionClientFromRequest({
    endpoint: "http://host.docker.internal:8787",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    request: runtimeRequest,
    fetch: async (request) => {
      requests.push(request);
      return Response.json(user());
    },
  });

  assert.deepEqual(await client.auth.getUser(), user());
  assert.equal(requests[0].headers.get("authorization"), `Bearer ${CALLER_TOKEN}`);
  assert.equal(requests[0].headers.get("x-mako-request-id"), "req_runtime00");
});

test("stable service errors remain sanitized and never expose caller credentials", async () => {
  const stable = createFunctionClient({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    callerAuthorization: CALLER_TOKEN,
    fetch: async () =>
      Response.json(
        {
          apiVersion: "v1",
          error: {
            code: "permission_denied",
            message: "document policy denied the query",
            requestId: "req_denied00",
            retry: { kind: "never" },
          },
        },
        { status: 403 },
      ),
  });
  await assert.rejects(
    () =>
      stable.documents("todos").query({
        predicates: [{ field: "ownerId", operator: "eq", value: "other" }],
        sort: [],
        cursor: null,
        limit: 10,
      }),
    (error) =>
      error instanceof MakoEdgeSdkError &&
      error.code === "permission_denied" &&
      error.requestId === "req_denied00" &&
      !error.message.includes(CALLER_TOKEN),
  );

  const untrusted = createFunctionClient({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    callerAuthorization: CALLER_TOKEN,
    fetch: async () => new Response(`database failed near ${CALLER_TOKEN}`, { status: 503 }),
  });
  await assert.rejects(
    () => untrusted.auth.getUser(),
    (error) =>
      error instanceof MakoEdgeSdkError &&
      error.message === "Mako service request failed" &&
      !error.message.includes(CALLER_TOKEN),
  );
});

test("service access is explicit, uses a separate route, and carries mandatory audit context", async () => {
  const requests = [];
  const service = createServiceClient({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    serviceCredential: SERVICE_CREDENTIAL,
    reason: "rebuild derived todo summaries",
    requestId: "req_service00",
    fetch: async (request) => {
      requests.push(request);
      return Response.json(document());
    },
  });

  await service.documents("todos").get("todo/one");
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/service/collections/todos/documents/todo%2Fone",
  );
  assert.equal(requests[0].headers.get("x-mako-service-key"), SERVICE_CREDENTIAL);
  assert.equal(
    requests[0].headers.get("x-mako-bypass-reason"),
    "rebuild derived todo summaries",
  );
  assert.equal(requests[0].headers.get("x-mako-request-id"), "req_service00");
  assert.equal(requests[0].headers.has("authorization"), false);
  assert.equal(JSON.stringify(service).includes(SERVICE_CREDENTIAL), false);

  assert.throws(
    () =>
      createServiceClient({
        endpoint: "https://api.example.test",
        projectId: "prj_abcdefgh",
        environmentId: "env_abcdefgh",
        serviceCredential: `mako_pk.public_edge.${"b".repeat(64)}`,
        reason: "must not work",
        requestId: "req_service01",
      }),
    MakoEdgeSdkError,
  );
});

test("service clients set app metadata on the users route under the configured or a per-call reason", async () => {
  const requests = [];
  const service = createServiceClient({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    serviceCredential: SERVICE_CREDENTIAL,
    reason: "household membership changed",
    requestId: "req_service02",
    fetch: async (request) => {
      requests.push(request);
      return Response.json({
        userId: "usr_abcdefgh",
        appMetadata: { households: { hh_one: "owner" } },
        authorizationEpoch: 3,
      });
    },
  });

  const result = await service.users.setAppMetadata("usr_abcdefgh", {
    households: { hh_one: "owner" },
    legacy: null,
  });
  assert.deepEqual(result, {
    userId: "usr_abcdefgh",
    appMetadata: { households: { hh_one: "owner" } },
    authorizationEpoch: 3,
  });
  assert.equal(requests.length, 1);
  assert.equal(requests[0].method, "POST");
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/service/users/usr_abcdefgh/app-metadata",
  );
  assert.equal(requests[0].headers.get("x-mako-service-key"), SERVICE_CREDENTIAL);
  assert.equal(requests[0].headers.get("x-mako-bypass-reason"), "household membership changed");
  assert.equal(requests[0].headers.get("x-mako-request-id"), "req_service02");
  assert.equal(requests[0].headers.get("content-type"), "application/json");
  assert.equal(requests[0].headers.has("authorization"), false);
  assert.equal(requests[0].headers.has("x-mako-key"), false);
  assert.deepEqual(JSON.parse(await requests[0].text()), {
    reason: "household membership changed",
    appMetadata: { households: { hh_one: "owner" }, legacy: null },
  });

  await service.users.setAppMetadata("usr_abcdefgh", { households: null }, "member removed");
  assert.equal(requests[1].headers.get("x-mako-bypass-reason"), "member removed");
  assert.deepEqual(JSON.parse(await requests[1].text()), {
    reason: "member removed",
    appMetadata: { households: null },
  });

  // Nothing malformed reaches the wire: the identifier, the patch shape, and
  // the per-call reason are checked first.
  await assert.rejects(() => service.users.setAppMetadata("not-a-user", {}), MakoEdgeSdkError);
  await assert.rejects(
    () => service.users.setAppMetadata("usr_abcdefgh", ["households"]),
    MakoEdgeSdkError,
  );
  await assert.rejects(
    () => service.users.setAppMetadata("usr_abcdefgh", {}, " padded "),
    MakoEdgeSdkError,
  );
  assert.equal(requests.length, 2);
});

test("app metadata refusals surface the stable error envelope", async () => {
  const service = createServiceClient({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    serviceCredential: SERVICE_CREDENTIAL,
    reason: "household membership changed",
    requestId: "req_service03",
    fetch: async () =>
      Response.json(
        {
          apiVersion: "v1",
          error: {
            code: "permission_denied",
            message: "service credential does not permit app metadata writes",
            requestId: "req_service03",
            retry: { kind: "never" },
          },
        },
        { status: 403 },
      ),
  });
  await assert.rejects(
    () => service.users.setAppMetadata("usr_abcdefgh", { households: {} }),
    (error) =>
      error instanceof MakoEdgeSdkError &&
      error.code === "permission_denied" &&
      error.status === 403 &&
      error.requestId === "req_service03" &&
      !error.message.includes(SERVICE_CREDENTIAL),
  );
});

function user() {
  return {
    id: "usr_abcdefgh",
    email: "caller@example.test",
    status: "active",
    authorizationEpoch: 1,
  };
}

function document() {
  return {
    primaryKey: "todo/one",
    schemaVersion: 1,
    revision: "rev_example00",
    commitPosition: 7,
    _deleted: false,
    body: { ownerId: "usr_abcdefgh", title: "Example" },
  };
}

/**
 * A refusal that does not say what was wrong is a refusal a caller has to
 * guess at. Every bound the SDK checks locally names itself -- and the one a
 * caller is least likely to guess, that a query must name at least one
 * predicate because every query is served by an index, says why.
 */
test("a query or mutation the SDK refuses says which rule it broke", async () => {
  const client = createServiceClient({
    endpoint: "https://api.example.test/internal",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    serviceCredential: SERVICE_CREDENTIAL,
    reason: "a test",
    requestId: "req_example00",
    fetch: async () => Response.json({ documents: [], nextCursor: null }),
  });
  const todos = client.documents("todos");
  const query = (overrides) => ({
    predicates: [{ field: "ownerId", operator: "eq", value: "usr_abcdefgh" }],
    sort: [],
    cursor: null,
    limit: 25,
    ...overrides,
  });
  const predicate = { field: "ownerId", operator: "eq", value: "usr_abcdefgh" };

  await assert.rejects(async () => await todos.query(query({ predicates: [] })), (error) => {
    assert.ok(error instanceof MakoEdgeSdkError);
    assert.match(error.message, /at least one predicate/u);
    assert.match(error.message, /served by an index/u);
    return true;
  });
  await assert.rejects(
    async () => await todos.query(query({ predicates: Array.from({ length: 17 }, () => predicate) })),
    /at most 16 predicates/u,
  );
  await assert.rejects(
    async () =>
      await todos.query(
        query({ sort: Array.from({ length: 17 }, () => ({ field: "ownerId", direction: "asc" })) }),
      ),
    /at most 16 sort keys/u,
  );
  await assert.rejects(async () => await todos.query(query({ limit: 0 })), /from 1 to 1000/u);
  await assert.rejects(async () => await todos.query(query({ limit: 1_001 })), /from 1 to 1000/u);

  const mutation = (overrides) => ({
    mutationId: "mutation_example00",
    operation: "update",
    expectedRevision: "rev-1",
    schemaVersion: 1,
    body: { id: "todo/one" },
    ...overrides,
  });
  await assert.rejects(
    async () => await todos.mutate("todo/one", mutation({ mutationId: "short" })),
    /16 to 200 characters/u,
  );
  await assert.rejects(
    async () => await todos.mutate("todo/one", mutation({ schemaVersion: 0 })),
    /schema version/u,
  );
  await assert.rejects(
    async () => await todos.mutate("todo/one", mutation({ expectedRevision: null })),
    /names the revision it read/u,
  );
  await assert.rejects(
    async () =>
      await todos.mutate(
        "todo/one",
        mutation({ operation: "create", expectedRevision: "rev-1" }),
      ),
    /a create names no expected revision/u,
  );
  await assert.rejects(async () => await todos.mutate("todo/one", mutation({ body: [] })), /a JSON object/u);
});
