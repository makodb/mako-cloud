import assert from "node:assert/strict";
import test from "node:test";

import {
  DEVELOPER_AUTH_OPERATIONS,
  MANAGEMENT_OPERATIONS,
  OPERATOR_OPERATIONS,
  ManagementApiError,
  createManagementClient,
  createDeveloperAuthClient,
  createOperatorClient,
} from "../dist/index.js";

test("hosted developer auth keeps access tokens in responses and refresh credentials in cookies", async () => {
  const requests = [];
  const client = createDeveloperAuthClient({
    endpoint: "https://cloud.example.test",
    fetch: async (request) => {
      requests.push(request);
      return Response.json({
        accessToken: "signed-developer-access-token",
        tokenType: "Bearer",
        audience: "mako-developer-waitlist",
        status: "waitlisted",
        expiresAt: "2026-08-09T12:15:00Z",
      });
    },
  });
  const session = await client.signIn("person@example.test", "developer password");
  assert.equal(session.status, "waitlisted");
  assert.equal(requests[0].url, "https://cloud.example.test/v1/developer-auth/sessions");
  assert.deepEqual(await requests[0].json(), {
    email: "person@example.test",
    password: "developer password",
  });
  assert.equal(requests[0].headers.get("authorization"), null);
});

test("developer auth inventory covers every Auth operation in the OpenAPI contract", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(new URL("../../../api/openapi/mako-cloud-v1.yaml", import.meta.url), "utf8");
  const operations = Array.from(
    source.matchAll(/tags: \[Auth\]\s+operationId: ([A-Za-z0-9]+)/gu),
    (match) => match[1],
  ).filter((operation) => operation.includes("Developer")).sort();
  assert.deepEqual([...DEVELOPER_AUTH_OPERATIONS].sort(), operations);
});

test("developer and automation credentials use the same versioned management contract", async () => {
  for (const kind of ["developer_session", "automation_token"]) {
    const requests = [];
    const client = createManagementClient({
      endpoint: "https://api.example.test",
      credential: { kind, accessToken: `${kind}-credential-value` },
      fetch: async (request) => {
        requests.push(request);
        return Response.json(team(), { status: 201 });
      },
    });
    assert.deepEqual(await client.createTeam("Example"), team());
    assert.equal(requests[0].url, "https://api.example.test/v1/teams");
    assert.equal(requests[0].headers.get("authorization"), `Bearer ${kind}-credential-value`);
    assert.deepEqual(await requests[0].json(), { name: "Example" });
  }
});

test("surfaces stable server errors and never echoes arbitrary response bodies", async () => {
  const stable = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async () =>
      Response.json(
        {
          apiVersion: "v1",
          error: {
            code: "permission_denied",
            message: "viewer cannot mutate team",
            requestId: "req_denied",
            retry: { kind: "never" },
          },
        },
        { status: 403 },
      ),
  });
  await assert.rejects(
    () => stable.updateTeam("org_abcdefgh", "Denied"),
    (error) =>
      error instanceof ManagementApiError &&
      error.code === "permission_denied" &&
      error.requestId === "req_denied" &&
      error.retry.kind === "never",
  );

  const untrusted = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "automation_token", accessToken: "automation-token-value" },
    fetch: async () => new Response("secret database diagnostic", { status: 503 }),
  });
  await assert.rejects(
    () => untrusted.getTeam("org_abcdefgh"),
    (error) =>
      error instanceof ManagementApiError &&
      error.message === "management request failed" &&
      !error.message.includes("database"),
  );
});

test("operation inventory covers every management operation in the OpenAPI contract", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(new URL("../../../api/openapi/mako-cloud-v1.yaml", import.meta.url), "utf8");
  const operations = Array.from(
    source.matchAll(
      /tags: \[(?:Management|Explorer|Developer Workspace)\]\s+operationId: ([A-Za-z0-9]+)/gu,
    ),
    (match) => match[1],
  ).filter(
    (operation) =>
      operation !== "uploadDataJobArtifact" && operation !== "downloadDataJobArtifact",
  );
  operations.push("verifyCurrentDeveloperPassword");
  operations.sort();
  assert.deepEqual([...MANAGEMENT_OPERATIONS].sort(), operations);
});

test("operator client uses only same-origin HttpOnly cookie credentials", async () => {
  const requests = [];
  const client = createOperatorClient({
    endpoint: "https://api.example.test",
    fetch: async (request) => {
      requests.push(request);
      return Response.json({
        project: {
          id: "prj_abcdefgh",
          teamId: "org_abcdefgh",
          name: "Example",
          region: "us-east",
          state: "active",
          failureDiagnostic: null,
          deletionDeadline: null,
          createdAt: "2026-08-06T00:00:00Z",
          updatedAt: "2026-08-06T00:00:00Z",
        },
        environments: [],
      });
    },
  });
  const view = await client.getOperatorProject("prj_abcdefgh");
  assert.equal(view.project.id, "prj_abcdefgh");
  assert.equal(requests[0].url, "https://api.example.test/v1/operator/projects/prj_abcdefgh");
  assert.equal(requests[0].headers.get("authorization"), null);
  assert.equal(requests[0].credentials, "include");
});

test("operator session lifecycle never exposes a JavaScript credential", async () => {
  const requests = [];
  const session = {
    operatorId: "opr_abcdefgh",
    developerIdentityId: "dev_abcdefgh",
    email: "operator@example.test",
    displayName: "Operator",
    permissions: ["waitlist_review"],
    passwordVerifiedAt: "2026-08-11T12:00:00Z",
    expiresAt: "2026-08-11T13:00:00Z",
  };
  const client = createOperatorClient({
    endpoint: "https://cloud.example.test",
    fetch: async (request) => {
      requests.push(request);
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      return Response.json(session);
    },
  });

  assert.deepEqual(await client.signIn("operator@example.test", "operator password"), session);
  assert.deepEqual(await client.currentSession(), session);
  assert.deepEqual(await client.verifyPassword("operator password"), session);
  await client.signOut();
  assert.deepEqual(
    requests.map((request) => [request.method, new URL(request.url).pathname]),
    [
      ["POST", "/v1/operator-auth/sessions"],
      ["GET", "/v1/operator-auth/sessions/current"],
      ["POST", "/v1/operator-auth/sessions/current/actions/verify-password"],
      ["DELETE", "/v1/operator-auth/sessions/current"],
    ],
  );
  for (const request of requests) {
    assert.equal(request.credentials, "include");
    assert.equal(request.headers.get("authorization"), null);
  }
});

test("operator wait-list decisions omit absent reasons and normalize supplied reasons", async () => {
  const requests = [];
  const applicant = {
    developerIdentityId: "dev_applicant01",
    email: "applicant@example.test",
    displayName: "Applicant",
    status: "active",
    operatorEntitlementStatus: "none",
    authorizationEpoch: 3,
    emailVerified: true,
    createdAt: "2026-08-11T12:00:00Z",
    updatedAt: "2026-08-11T12:05:00Z",
  };
  const client = createOperatorClient({
    endpoint: "https://cloud.example.test",
    fetch: async (request) => {
      requests.push(request);
      return Response.json(applicant);
    },
  });

  await client.approveDeveloperWaitListApplicant(
    "dev_applicant01",
    undefined,
    "idempotency_approve_no_reason",
  );
  await client.rejectDeveloperWaitListApplicant(
    "dev_applicant02",
    "  \t ",
    "idempotency_reject_no_reason",
  );
  await client.approveDeveloperWaitListApplicant(
    "dev_applicant03",
    "  approved for beta review  ",
    "idempotency_approve_with_reason",
  );

  assert.deepEqual(await requests[0].json(), {});
  assert.deepEqual(await requests[1].json(), {});
  assert.deepEqual(await requests[2].json(), { reason: "approved for beta review" });
  assert.deepEqual(
    requests.map((request) => request.headers.get("idempotency-key")),
    [
      "idempotency_approve_no_reason",
      "idempotency_reject_no_reason",
      "idempotency_approve_with_reason",
    ],
  );
  assert(requests.every((request) => request.credentials === "include"));
});

test("operator inventory covers every operator operation in the OpenAPI contract", async () => {
  const { readFile } = await import("node:fs/promises");
  const source = await readFile(new URL("../../../api/openapi/mako-cloud-v1.yaml", import.meta.url), "utf8");
  const operations = Array.from(
    source.matchAll(/tags: \[Operator\]\s+operationId: ([A-Za-z0-9]+)/gu),
    (match) => match[1],
  ).sort();
  assert.deepEqual([...OPERATOR_OPERATIONS].sort(), operations);
});

function team() {
  return {
    id: "org_abcdefgh",
    name: "Example",
    state: "active",
    createdAt: "2026-08-06T00:00:00Z",
    updatedAt: "2026-08-06T00:00:00Z",
  };
}
