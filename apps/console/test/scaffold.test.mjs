import assert from "node:assert/strict";
import test from "node:test";

import {
  MemoryDeveloperAuthAdapter,
  MemoryOperatorAuthAdapter,
  HostedOperatorAuthAdapter,
  ShortLivedDeveloperSessionAuthAdapter,
  filterObservabilityRecords,
  isSessionActive,
  isOperatorSessionActive,
  legacyConsoleRedirectPath,
  matchConsoleRoute,
  observabilityRecordsToCsv,
} from "../dist/index.js";

const encode = (value) =>
  Buffer.from(JSON.stringify(value), "utf8").toString("base64url");

const developerToken = (overrides = {}) =>
  `${encode({ alg: "EdDSA", typ: "JWT", kid: "devkid_0123456789abcdef" })}.${encode({
    iss: "https://cloud-test.makodb.com/control-identity",
    sub: "developer@example.test",
    aud: ["mako-management"],
    email: "developer@example.test",
    emailVerified: true,
    name: "Developer",
    sid: "session_abcdefgh",
    developerIdentityId: "dev_abcdefgh",
    status: "active",
    credentialEpoch: 1,
    authorizationEpoch: 1,
    iat: 1_786_298_400,
    exp: 1_786_302_000,
    ...overrides,
  })}.signature`;

test("route matching fails closed for unknown and malformed locations", () => {
  assert.deepEqual(matchConsoleRoute("/"), { name: "home" });
  assert.deepEqual(matchConsoleRoute("/login/"), { name: "login" });
  assert.deepEqual(matchConsoleRoute("/sign-in"), { name: "login" });
  assert.deepEqual(matchConsoleRoute("/create-account"), { name: "create_account" });
  assert.deepEqual(matchConsoleRoute("/verify-email"), { name: "verify_email" });
  assert.deepEqual(matchConsoleRoute("/wait-list"), { name: "wait_list" });
  assert.deepEqual(matchConsoleRoute("/operator/"), {
    name: "operator",
    section: "overview",
  });
  assert.deepEqual(matchConsoleRoute("/operator/tenants"), {
    name: "operator",
    section: "tenants",
  });
  assert.deepEqual(matchConsoleRoute("/operator/tenants/prj_abcdefgh"), {
    name: "operator",
    section: "tenant",
    projectId: "prj_abcdefgh",
  });
  assert.deepEqual(matchConsoleRoute("/operator/activity?actor=opr_abcdefgh"), {
    name: "operator",
    section: "activity",
  });
  assert.equal(matchConsoleRoute("/operator/tenants/not-valid").name, "not_found");
  assert.deepEqual(matchConsoleRoute("/teams/org_abcdefgh"), {
    name: "team",
    teamId: "org_abcdefgh",
  });
  assert.deepEqual(matchConsoleRoute("/invitations/inv_abcdefgh"), {
    name: "invitation",
    invitationId: "inv_abcdefgh",
  });
  assert.deepEqual(matchConsoleRoute("/projects/prj_abcdefgh"), {
    name: "project",
    projectId: "prj_abcdefgh",
    section: "overview",
  });
  assert.deepEqual(matchConsoleRoute("/projects/prj_abcdefgh/usage"), {
    name: "project",
    projectId: "prj_abcdefgh",
    section: "usage",
  });
  assert.equal(matchConsoleRoute("/projects/prj_abcdefgh/billing").name, "not_found");
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/logs"),
    { name: "logs", projectId: "prj_abcdefgh", environmentId: "env_abcdefgh" },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/activity"),
    { name: "activity", projectId: "prj_abcdefgh", environmentId: "env_abcdefgh" },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/storage"),
    { name: "storage", projectId: "prj_abcdefgh", environmentId: "env_abcdefgh" },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/storage/avatars"),
    {
      name: "storage",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      bucketId: "avatars",
    },
  );
  for (const invalidBucket of ["Avatars", "a", "1avatars", "avatars/objects", "avatars_x"]) {
    assert.equal(
      matchConsoleRoute(
        `/projects/prj_abcdefgh/environments/env_abcdefgh/storage/${invalidBucket}`,
      ).name,
      "not_found",
      invalidBucket,
    );
  }
  assert.equal(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/webhooks").name,
    "not_found",
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/collections"),
    {
      name: "collections",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute(
      "/projects/prj_abcdefgh/environments/env_abcdefgh/collections/todos/policies",
    ),
    {
      name: "policy",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      collectionId: "todos",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/observability"),
    {
      name: "observability",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/collections/todos"),
    {
      name: "collection",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      collectionId: "todos",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/users"),
    {
      name: "users",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/users/usr_abcdefgh"),
    {
      name: "user",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      userId: "usr_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/credentials"),
    {
      name: "credentials",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute("/projects/prj_abcdefgh/environments/env_abcdefgh/functions"),
    {
      name: "functions",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
    },
  );
  assert.deepEqual(
    matchConsoleRoute(
      "/projects/prj_abcdefgh/environments/env_abcdefgh/functions/process-order",
    ),
    {
      name: "function",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      functionName: "process-order",
    },
  );
  assert.equal(
    matchConsoleRoute(
      "/projects/prj_abcdefgh/environments/env_abcdefgh/collections/INVALID",
    ).name,
    "not_found",
  );
  assert.deepEqual(matchConsoleRoute("https://evil.example"), {
    name: "not_found",
    path: "/not-found",
  });
  assert.deepEqual(matchConsoleRoute("/unknown?secret=not-in-route"), {
    name: "not_found",
    path: "/unknown",
  });
});

test("legacy workspace redirects copy only validated identifiers", () => {
  assert.equal(
    legacyConsoleRedirectPath(
      "/projects/prj_abcdefgh/environments/env_abcdefgh/explorer?capability=must-drop",
    ),
    "/projects/prj_abcdefgh/environments/env_abcdefgh/data",
  );
  assert.equal(
    legacyConsoleRedirectPath(
      "/projects/prj_abcdefgh/environments/env_abcdefgh/api#secret-must-drop",
    ),
    "/projects/prj_abcdefgh/environments/env_abcdefgh/connect",
  );
  assert.equal(
    legacyConsoleRedirectPath("/projects/not-valid/environments/env_abcdefgh/recovery"),
    null,
  );
});

test("developer sessions expire and the local adapter never persists credentials", async () => {
  const session = {
    accessToken: "developer-session-token",
    expiresAt: "2026-08-06T01:00:00.000Z",
    audience: "mako-management",
    profile: {
      id: "dev_abcdefgh",
      email: "developer@example.test",
      displayName: "Developer",
    },
  };
  assert.equal(isSessionActive(session, Date.parse("2026-08-06T00:00:00.000Z")), true);
  assert.equal(isSessionActive(session, Date.parse(session.expiresAt)), false);

  const adapter = new MemoryDeveloperAuthAdapter(session);
  const observed = [];
  const unsubscribe = adapter.subscribe((value) => observed.push(value));
  await adapter.signOut();
  unsubscribe();
  assert.equal(await adapter.loadSession(), null);
  assert.deepEqual(observed, [null]);
});

test("hosted console accepts only environment-bound short-lived developer sessions", async () => {
  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  };
  const adapter = new ShortLivedDeveloperSessionAuthAdapter({
    managementEndpoint: "https://cloud-test.makodb.com",
    storage,
    now: () => 1_786_298_500_000,
  });
  const observed = [];
  adapter.subscribe((session) => observed.push(session?.profile.id ?? null));
  await adapter.acceptSessionToken(developerToken());
  assert.equal((await adapter.loadSession())?.profile.email, "developer@example.test");
  assert.deepEqual(observed, ["dev_abcdefgh"]);

  await adapter.signOut();
  assert.equal(await adapter.loadSession(), null);
  assert.deepEqual(observed, ["dev_abcdefgh", null]);

  await assert.rejects(
    adapter.acceptSessionToken(developerToken({ iss: "https://other.example/control-identity" })),
    /invalid or expired/u,
  );
  await assert.rejects(
    adapter.acceptSessionToken(developerToken({ exp: 1_786_298_500 })),
    /invalid or expired/u,
  );
});

test("observability search and CSV export retain safe structured audit data", () => {
  const records = [
    {
      timestamp: "2026-08-06T12:00:00.000Z",
      payload: {
        kind: "audit",
        teamId: "org_abcdefgh",
        actorId: '=HYPERLINK("https://example.test")',
        action: "policy.activate",
        target: "todos/policy/2",
        outcome: "allowed",
        requestId: "req_abcdefgh",
        details: "authorization epoch 4",
      },
    },
  ];

  assert.equal(filterObservabilityRecords(records, "REQ_ABCDEFGH").length, 1);
  assert.equal(filterObservabilityRecords(records, "not-present").length, 0);
  const csv = observabilityRecordsToCsv(records);
  assert.match(csv, /"'=HYPERLINK\(""https:\/\/example\.test""\)"/u);
  assert.match(csv, /"policy\.activate"/u);
});

test("operator sessions are separately validated and held only in memory", async () => {
  const session = {
    expiresAt: "2026-08-06T01:00:00.000Z",
    passwordVerifiedAt: "2026-08-06T00:00:00.000Z",
    permissions: ["waitlist_review"],
    profile: {
      id: "opr_abcdefgh",
      developerIdentityId: "dev_abcdefgh",
      email: "operator@example.test",
      displayName: "Operator",
      developerStatus: "waitlisted",
    },
  };
  assert.equal(isOperatorSessionActive(session, Date.parse("2026-08-06T00:00:00.000Z")), true);
  assert.equal(isOperatorSessionActive(session, Date.parse(session.expiresAt)), false);

  const adapter = new MemoryOperatorAuthAdapter(session);
  const observed = [];
  const unsubscribe = adapter.subscribe((value) => observed.push(value));
  await adapter.signOut();
  unsubscribe();
  assert.equal(await adapter.loadSession(), null);
  assert.deepEqual(observed, [null]);
});

test("hosted operator sessions use credentialed cookie operations with no readable token", async () => {
  const requests = [];
  const apiSession = {
    operatorId: "opr_abcdefgh",
    developerIdentityId: "dev_abcdefgh",
    email: "operator@example.test",
    displayName: "Operator",
    developerStatus: "waitlisted",
    permissions: ["tenant_read"],
    passwordVerifiedAt: "2026-08-06T00:00:00.000Z",
    expiresAt: "2026-08-06T01:00:00.000Z",
  };
  const adapter = new HostedOperatorAuthAdapter({
    managementEndpoint: "https://cloud-test.makodb.com",
    fetch: async (request) => {
      requests.push(request);
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      return Response.json(apiSession);
    },
    now: () => Date.parse("2026-08-06T00:00:00.000Z"),
  });
  assert.equal(
    (await adapter.signIn("operator@example.test", "operator password")).profile.id,
    "opr_abcdefgh",
  );
  assert.equal((await adapter.loadSession())?.profile.developerIdentityId, "dev_abcdefgh");
  assert.equal((await adapter.verifyPassword("operator password")).permissions[0], "tenant_read");
  await adapter.signOut();
  assert.deepEqual(
    requests.map((request) => [request.method, new URL(request.url).pathname]),
    [
      ["POST", "/v1/operator-auth/sessions"],
      ["GET", "/v1/operator-auth/sessions/current"],
      ["POST", "/v1/operator-auth/sessions/current/actions/verify-password"],
      ["DELETE", "/v1/operator-auth/sessions/current"],
    ],
  );
  assert.ok(requests.every((request) => request.credentials === "include"));
  assert.ok(requests.every((request) => request.headers.get("authorization") === null));
});
