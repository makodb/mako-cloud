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

test("storage object paths keep their slashes on the wire and bucket deletion carries its confirmation", async () => {
  const requests = [];
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      return Response.json(
        request.method === "DELETE" && request.url.includes("/objects/")
          ? storageObject()
          : { objectCount: 2, totalBytes: 1024 },
      );
    },
  });
  assert.deepEqual(
    await client.deleteStorageObject("prj_example0001", "env_example0001", "avatars", "users/42/me avatar.png"),
    storageObject(),
  );
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/storage-buckets/avatars/objects/users/42/me%20avatar.png",
  );
  for (const escaping of ["", "users//me.png", "users/../me.png", "/me.png", "me.png/"]) {
    await assert.rejects(
      client.deleteStorageObject("prj_example0001", "env_example0001", "avatars", escaping),
      TypeError,
    );
  }
  assert.deepEqual(
    await client.deleteStorageBucket(
      "prj_example0001",
      "env_example0001",
      "avatars",
      "delete avatars",
      true,
    ),
    { objectCount: 2, totalBytes: 1024 },
  );
  assert.equal(
    requests[1].url,
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/storage-buckets/avatars?deleteObjects=true",
  );
  assert.equal(requests[1].headers.get("confirmation"), "delete avatars");
  await client.deleteStorageBucket("prj_example0001", "env_example0001", "avatars", "delete avatars");
  assert.equal(
    requests[2].url,
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/storage-buckets/avatars",
  );
  assert.equal(requests.length, 3, "a refused path never reaches the network");
});

test("allowed origins are read and replaced whole on the environment, with an idempotency key", async () => {
  const requests = [];
  let allowed = { allowedOrigins: ["https://app.example.test"] };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      if (request.method === "PUT") allowed = await request.clone().json();
      return Response.json(allowed);
    },
  });
  const path =
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/allowed-origins";

  assert.deepEqual(await client.getAllowedOrigins("prj_example0001", "env_example0001"), {
    allowedOrigins: ["https://app.example.test"],
  });
  assert.equal(requests[0].method, "GET");
  assert.equal(requests[0].url, path, "the allowlist belongs to the environment, not to a domain");

  const replacement = { allowedOrigins: ["https://app.example.test", "http://127.0.0.1:5173"] };
  const replaced = await client.updateAllowedOrigins(
    "prj_example0001",
    "env_example0001",
    replacement,
    "idempotency-key-allowed-origins-0001",
  );
  assert.deepEqual(replaced, replacement);
  assert.equal(requests[1].method, "PUT");
  assert.equal(requests[1].url, path);
  assert.equal(
    requests[1].headers.get("idempotency-key"),
    "idempotency-key-allowed-origins-0001",
  );
  assert.equal(requests[1].headers.get("content-type"), "application/json");
  assert.deepEqual(
    await requests[1].json(),
    replacement,
    "the whole list is sent; the server replaces, never merges",
  );

  const cleared = await client.updateAllowedOrigins(
    "prj_example0001",
    "env_example0001",
    { allowedOrigins: [] },
    "idempotency-key-allowed-origins-0002",
  );
  assert.deepEqual(cleared, { allowedOrigins: [] }, "an empty list means no cross-origin access");
  assert.deepEqual(await requests[2].json(), { allowedOrigins: [] });
});

test("auth settings are read and replaced whole; the client secret travels once, in the request body", async () => {
  const requests = [];
  const installed = {
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "client-id.apps.googleusercontent.com",
        scopes: [],
        enabled: true,
        hasSecret: true,
      },
      { name: "github", kind: { type: "git_hub" }, clientId: "Iv1.github", scopes: [], enabled: false, hasSecret: true },
    ],
    redirectUrls: ["https://app.example.test/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
    version: 3,
  };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      return Response.json(installed);
    },
  });
  assert.deepEqual(await client.getAuthSettings("prj_example0001", "env_example0001"), installed);
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/auth-settings",
  );
  assert.equal(requests[0].method, "GET");

  const update = {
    providers: [
      {
        name: "google",
        kind: { type: "oidc", issuer: "https://accounts.google.com" },
        clientId: "client-id.apps.googleusercontent.com",
        clientSecret: "GOCSPX-plain-secret",
        enabled: true,
      },
      { name: "github", kind: { type: "git_hub" }, clientId: "Iv1.github", enabled: false },
    ],
    redirectUrls: ["https://app.example.test/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
  };
  const replaced = await client.updateAuthSettings(
    "prj_example0001",
    "env_example0001",
    update,
    "idempotency-key-auth-settings-0001",
  );
  assert.deepEqual(replaced, installed);
  assert.equal(requests[1].method, "PUT");
  assert.equal(
    requests[1].url,
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/auth-settings",
  );
  assert.equal(requests[1].headers.get("idempotency-key"), "idempotency-key-auth-settings-0001");
  assert.equal(requests[1].headers.get("content-type"), "application/json");
  assert.deepEqual(await requests[1].json(), update, "the body is sent as given, secret included, once");
  assert.doesNotMatch(JSON.stringify(replaced), /GOCSPX/u, "the answer carries no secret");
});

test("email templates are keyed by kind, saved with an idempotency key, and previewed with or without unsaved text", async () => {
  const requests = [];
  const template = {
    kind: "magic_link",
    subject: "Sign in to {{project_name}}",
    textBody: "{{link}}",
    isDefault: false,
    version: 2,
    updatedAt: "2026-08-29T10:00:00Z",
  };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      if (request.url.endsWith("/email-templates")) return Response.json({ items: [template] });
      if (request.url.endsWith("/actions/preview")) {
        return Response.json({ subject: "Sign in to Field Notes", textBody: "https://example.test/link" });
      }
      return Response.json(template);
    },
  });
  const base = "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/email-templates";
  assert.deepEqual(await client.listEmailTemplates("prj_example0001", "env_example0001"), [template]);
  assert.equal(requests[0].url, base);
  assert.deepEqual(await client.getEmailTemplate("prj_example0001", "env_example0001", "magic_link"), template);
  assert.equal(requests[1].url, `${base}/magic_link`);
  await client.updateEmailTemplate(
    "prj_example0001",
    "env_example0001",
    "magic_link",
    { subject: template.subject, textBody: template.textBody },
    "idempotency-key-0001",
  );
  assert.equal(requests[2].method, "PUT");
  assert.equal(requests[2].url, `${base}/magic_link`);
  assert.equal(requests[2].headers.get("idempotency-key"), "idempotency-key-0001");
  assert.deepEqual(await requests[2].json(), { subject: template.subject, textBody: template.textBody });
  await client.resetEmailTemplate("prj_example0001", "env_example0001", "magic_link");
  assert.equal(requests[3].method, "DELETE");
  assert.equal(requests[3].url, `${base}/magic_link`);
  assert.equal(requests[3].headers.get("confirmation"), null, "a reset is not destructive of data");
  const rendered = await client.previewEmailTemplate("prj_example0001", "env_example0001", "magic_link");
  assert.equal(rendered.subject, "Sign in to Field Notes");
  assert.equal(requests[4].url, `${base}/magic_link/actions/preview`);
  assert.equal(await requests[4].text(), "", "no body previews the stored template");
  await client.previewEmailTemplate("prj_example0001", "env_example0001", "magic_link", { subject: "Hi {{email}}" });
  assert.deepEqual(await requests[5].json(), { subject: "Hi {{email}}" }, "only the given part is sent");
});

function storageObject() {
  return {
    path: "users/42/me avatar.png",
    contentType: "image/png",
    sizeBytes: 512,
    ownerId: "usr_example0001",
    digest: "sha256:abc",
    createdAt: "2026-08-13T00:00:00Z",
    updatedAt: "2026-08-13T00:00:00Z",
  };
}

function team() {
  return {
    id: "org_abcdefgh",
    name: "Example",
    state: "active",
    createdAt: "2026-08-06T00:00:00Z",
    updatedAt: "2026-08-06T00:00:00Z",
  };
}

test("webhook endpoints give the signing secret once on create and rotate, page deliveries by query, and redeliver by id", async () => {
  const requests = [];
  const endpoint = {
    id: "whk_abcdef123456",
    url: "https://hooks.example.test/mako",
    description: "orders",
    subscriptions: [{ collectionId: "orders", events: ["insert", "update"] }],
    state: "active",
    enabled: true,
    pausedReason: null,
    pausedAt: null,
    consecutiveFailures: 0,
    secretVersion: 1,
    createdAt: "2026-08-29T10:00:00Z",
    updatedAt: "2026-08-29T10:00:00Z",
  };
  const delivery = {
    id: "whd_abcdef123456",
    endpointId: endpoint.id,
    event: "insert",
    collectionId: "orders",
    documentId: "ord_1",
    revision: "1-a",
    commitPosition: 7,
    state: "failed",
    attempts: 5,
    nextAttemptAt: null,
    lastResponseStatus: 503,
    lastError: "status_503",
    redeliveryOf: null,
    createdAt: "2026-08-29T10:01:00Z",
    deliveredAt: null,
  };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      const path = new URL(request.url).pathname;
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      if (path.endsWith("/webhooks") && request.method === "GET") {
        return Response.json({ items: [endpoint] });
      }
      if (path.endsWith("/webhooks") && request.method === "POST") {
        return Response.json({ endpoint, signingSecret: "whs_created_once" }, { status: 201 });
      }
      if (path.endsWith("/actions/rotate-secret")) {
        return Response.json({ endpoint: { ...endpoint, secretVersion: 2 }, signingSecret: "whs_rotated_once" });
      }
      if (path.endsWith("/actions/resume")) return Response.json(endpoint);
      if (path.endsWith("/deliveries")) return Response.json({ items: [delivery], nextCursor: "c2" });
      if (path.endsWith("/actions/redeliver")) {
        return Response.json({ ...delivery, id: "whd_redeliver0001", state: "pending", redeliveryOf: delivery.id }, { status: 202 });
      }
      return Response.json(endpoint);
    },
  });
  const base = "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/webhooks";

  const created = await client.createWebhookEndpoint(
    "prj_example0001",
    "env_example0001",
    { url: endpoint.url, description: "orders", subscriptions: endpoint.subscriptions },
    "idempotency-key-webhook-create",
  );
  assert.equal(created.signingSecret, "whs_created_once", "the secret is in the create answer");
  assert.deepEqual(created.endpoint, endpoint);
  assert.equal(requests[0].method, "POST");
  assert.equal(requests[0].url, base);
  assert.equal(requests[0].headers.get("idempotency-key"), "idempotency-key-webhook-create");
  assert.deepEqual(await requests[0].json(), {
    url: endpoint.url,
    description: "orders",
    subscriptions: endpoint.subscriptions,
  });

  const listed = await client.listWebhookEndpoints("prj_example0001", "env_example0001");
  assert.deepEqual(listed, [endpoint]);
  assert.doesNotMatch(JSON.stringify(listed), /whs_/u, "listing never carries a secret");
  assert.equal(requests[1].url, base);

  assert.deepEqual(await client.getWebhookEndpoint("prj_example0001", "env_example0001", endpoint.id), endpoint);
  assert.equal(requests[2].url, `${base}/${endpoint.id}`);

  await client.updateWebhookEndpoint(
    "prj_example0001",
    "env_example0001",
    endpoint.id,
    { enabled: false },
    "idempotency-key-webhook-update",
  );
  assert.equal(requests[3].method, "PATCH");
  assert.equal(requests[3].url, `${base}/${endpoint.id}`);
  assert.equal(requests[3].headers.get("idempotency-key"), "idempotency-key-webhook-update");
  assert.deepEqual(await requests[3].json(), { enabled: false });

  const rotated = await client.rotateWebhookSecret(
    "prj_example0001",
    "env_example0001",
    endpoint.id,
    "idempotency-key-webhook-rotate",
  );
  assert.equal(rotated.signingSecret, "whs_rotated_once");
  assert.equal(rotated.endpoint.secretVersion, 2);
  assert.equal(requests[4].method, "POST");
  assert.equal(requests[4].url, `${base}/${endpoint.id}/actions/rotate-secret`);
  assert.equal(requests[4].headers.get("idempotency-key"), "idempotency-key-webhook-rotate");
  assert.equal(await requests[4].text(), "", "an action carries no body");

  await client.resumeWebhookEndpoint("prj_example0001", "env_example0001", endpoint.id, "idempotency-key-webhook-resume");
  assert.equal(requests[5].url, `${base}/${endpoint.id}/actions/resume`);
  assert.equal(requests[5].headers.get("idempotency-key"), "idempotency-key-webhook-resume");

  const page = await client.listWebhookDeliveries("prj_example0001", "env_example0001", endpoint.id, {
    state: "failed",
    cursor: "c1",
    limit: 25,
  });
  assert.deepEqual(page, { items: [delivery], nextCursor: "c2" });
  assert.equal(requests[6].method, "GET");
  assert.equal(requests[6].url, `${base}/${endpoint.id}/deliveries?state=failed&cursor=c1&limit=25`);
  await client.listWebhookDeliveries("prj_example0001", "env_example0001", endpoint.id);
  assert.equal(requests[7].url, `${base}/${endpoint.id}/deliveries`, "no query parameter is invented");

  const redelivered = await client.redeliverWebhookDelivery(
    "prj_example0001",
    "env_example0001",
    endpoint.id,
    delivery.id,
    "idempotency-key-webhook-redeliver",
  );
  assert.equal(redelivered.redeliveryOf, delivery.id);
  assert.equal(requests[8].method, "POST");
  assert.equal(requests[8].url, `${base}/${endpoint.id}/deliveries/${delivery.id}/actions/redeliver`);
  assert.equal(requests[8].headers.get("idempotency-key"), "idempotency-key-webhook-redeliver");

  assert.equal(
    await client.deleteWebhookEndpoint("prj_example0001", "env_example0001", endpoint.id, "idempotency-key-webhook-delete"),
    undefined,
  );
  assert.equal(requests[9].method, "DELETE");
  assert.equal(requests[9].url, `${base}/${endpoint.id}`);
  assert.equal(requests[9].headers.get("idempotency-key"), "idempotency-key-webhook-delete");
});

test("function schedules are created, read, patched, run now, paged by run outcome, and deleted under the function", async () => {
  const requests = [];
  const schedule = {
    id: "sch_nightly000001",
    functionName: "nightly-report",
    name: "nightly",
    cron: "0 2 * * *",
    timezone: "UTC",
    request: { method: "POST", path: "/", headers: {}, contentType: "application/json", body: "{}" },
    enabled: true,
    state: "active",
    nextRunAt: "2026-08-30T02:00:00Z",
    lastRun: null,
    createdAt: "2026-08-29T10:00:00Z",
    updatedAt: "2026-08-29T10:00:00Z",
  };
  const run = {
    id: "run_nightly000001",
    scheduleId: schedule.id,
    functionName: schedule.functionName,
    functionVersion: 3,
    dueAt: "2026-08-29T02:00:00Z",
    startedAt: "2026-08-29T02:00:00Z",
    completedAt: "2026-08-29T02:00:01Z",
    durationMilliseconds: 812,
    outcome: "succeeded",
    responseStatus: 200,
    error: null,
    manual: false,
    createdAt: "2026-08-29T02:00:00Z",
  };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      const path = new URL(request.url).pathname;
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      if (path.endsWith("/schedules") && request.method === "GET") {
        return Response.json({ items: [schedule] });
      }
      if (path.endsWith("/schedules") && request.method === "POST") {
        return Response.json(schedule, { status: 201 });
      }
      if (path.endsWith("/actions/run-now")) {
        return Response.json({ ...run, id: "run_manual0000001", outcome: null, completedAt: null, manual: true }, { status: 202 });
      }
      if (path.endsWith("/runs")) return Response.json({ items: [run], nextCursor: "c2" });
      if (request.method === "PATCH") {
        return Response.json({ ...schedule, enabled: false, state: "paused", nextRunAt: null });
      }
      return Response.json(schedule);
    },
  });
  const base =
    "https://api.example.test/v1/projects/prj_example0001/environments/env_example0001/functions/nightly-report/schedules";

  const created = await client.createFunctionSchedule(
    "prj_example0001",
    "env_example0001",
    "nightly-report",
    {
      name: "nightly",
      cron: "0 2 * * *",
      request: { method: "POST", path: "/", contentType: "application/json", body: "{}" },
    },
    "idempotency-key-schedule-create",
  );
  assert.deepEqual(created, schedule);
  assert.equal(requests[0].method, "POST");
  assert.equal(requests[0].url, base);
  assert.equal(requests[0].headers.get("idempotency-key"), "idempotency-key-schedule-create");
  assert.deepEqual(await requests[0].json(), {
    name: "nightly",
    cron: "0 2 * * *",
    request: { method: "POST", path: "/", contentType: "application/json", body: "{}" },
  });

  assert.deepEqual(
    await client.listFunctionSchedules("prj_example0001", "env_example0001", "nightly-report"),
    [schedule],
  );
  assert.equal(requests[1].method, "GET");
  assert.equal(requests[1].url, base);

  assert.deepEqual(
    await client.getFunctionSchedule("prj_example0001", "env_example0001", "nightly-report", schedule.id),
    schedule,
  );
  assert.equal(requests[2].url, `${base}/${schedule.id}`);

  const paused = await client.updateFunctionSchedule(
    "prj_example0001",
    "env_example0001",
    "nightly-report",
    schedule.id,
    { enabled: false },
    "idempotency-key-schedule-update",
  );
  assert.equal(paused.state, "paused");
  assert.equal(paused.nextRunAt, null, "a paused schedule has no next run");
  assert.equal(requests[3].method, "PATCH");
  assert.equal(requests[3].url, `${base}/${schedule.id}`);
  assert.equal(requests[3].headers.get("idempotency-key"), "idempotency-key-schedule-update");
  assert.deepEqual(await requests[3].json(), { enabled: false });

  const queued = await client.runFunctionScheduleNow(
    "prj_example0001",
    "env_example0001",
    "nightly-report",
    schedule.id,
    "idempotency-key-schedule-run",
  );
  assert.equal(queued.manual, true);
  assert.equal(queued.outcome, null, "a queued run has no outcome yet");
  assert.equal(requests[4].method, "POST");
  assert.equal(requests[4].url, `${base}/${schedule.id}/actions/run-now`);
  assert.equal(requests[4].headers.get("idempotency-key"), "idempotency-key-schedule-run");
  assert.equal(await requests[4].text(), "", "an action carries no body");

  const page = await client.listFunctionScheduleRuns(
    "prj_example0001",
    "env_example0001",
    "nightly-report",
    schedule.id,
    { outcome: "failed", cursor: "c1", limit: 25 },
  );
  assert.deepEqual(page, { items: [run], nextCursor: "c2" });
  assert.equal(requests[5].method, "GET");
  assert.equal(requests[5].url, `${base}/${schedule.id}/runs?outcome=failed&cursor=c1&limit=25`);
  await client.listFunctionScheduleRuns("prj_example0001", "env_example0001", "nightly-report", schedule.id);
  assert.equal(requests[6].url, `${base}/${schedule.id}/runs`, "no query parameter is invented");

  assert.equal(
    await client.deleteFunctionSchedule(
      "prj_example0001",
      "env_example0001",
      "nightly-report",
      schedule.id,
      "idempotency-key-schedule-delete",
    ),
    undefined,
  );
  assert.equal(requests[7].method, "DELETE");
  assert.equal(requests[7].url, `${base}/${schedule.id}`);
  assert.equal(requests[7].headers.get("idempotency-key"), "idempotency-key-schedule-delete");
});

test("custom domains are project-level: created with an idempotency key and the TXT record in the answer, listed, read, verified on request, and deleted", async () => {
  const requests = [];
  const domain = {
    id: "dom_api000000001",
    projectId: "prj_example0001",
    environmentId: "env_example0001",
    hostname: "api.example.com",
    state: "pending",
    verification: {
      recordName: "_mako-verify.api.example.com",
      recordType: "TXT",
      recordValue: "mako-verify=0123456789abcdef",
    },
    verifiedAt: null,
    lastCheckedAt: null,
    lastError: null,
    createdAt: "2026-08-29T10:00:00Z",
    updatedAt: "2026-08-29T10:00:00Z",
  };
  const client = createManagementClient({
    endpoint: "https://api.example.test",
    credential: { kind: "developer_session", accessToken: "developer-session-token" },
    fetch: async (request) => {
      requests.push(request);
      const path = new URL(request.url).pathname;
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      if (path.endsWith("/domains") && request.method === "GET") {
        return Response.json({ items: [domain] });
      }
      if (path.endsWith("/domains") && request.method === "POST") {
        return Response.json(domain, { status: 201 });
      }
      if (path.endsWith("/actions/verify")) {
        return Response.json({
          ...domain,
          state: "verified",
          verifiedAt: "2026-08-29T10:05:00Z",
          lastCheckedAt: "2026-08-29T10:05:00Z",
        });
      }
      return Response.json(domain);
    },
  });
  const base = "https://api.example.test/v1/projects/prj_example0001/domains";

  const created = await client.createCustomDomain(
    "prj_example0001",
    { hostname: "api.example.com", environmentId: "env_example0001" },
    "idempotency-key-domain-create",
  );
  assert.deepEqual(created, domain);
  assert.equal(created.verification.recordType, "TXT", "the record to publish is in the answer");
  assert.equal(requests[0].method, "POST");
  assert.equal(requests[0].url, base, "domains hang off the project, not an environment");
  assert.equal(requests[0].headers.get("idempotency-key"), "idempotency-key-domain-create");
  assert.deepEqual(await requests[0].json(), {
    hostname: "api.example.com",
    environmentId: "env_example0001",
  });

  assert.deepEqual(await client.listCustomDomains("prj_example0001"), [domain]);
  assert.equal(requests[1].method, "GET");
  assert.equal(requests[1].url, base);

  assert.deepEqual(await client.getCustomDomain("prj_example0001", domain.id), domain);
  assert.equal(requests[2].method, "GET");
  assert.equal(requests[2].url, `${base}/${domain.id}`);

  const verified = await client.verifyCustomDomain(
    "prj_example0001",
    domain.id,
    "idempotency-key-domain-verify",
  );
  assert.equal(verified.state, "verified");
  assert.equal(verified.verifiedAt, "2026-08-29T10:05:00Z");
  assert.equal(requests[3].method, "POST");
  assert.equal(requests[3].url, `${base}/${domain.id}/actions/verify`);
  assert.equal(requests[3].headers.get("idempotency-key"), "idempotency-key-domain-verify");
  assert.equal(await requests[3].text(), "", "an action carries no body");

  assert.equal(
    await client.deleteCustomDomain("prj_example0001", domain.id, "idempotency-key-domain-delete"),
    undefined,
  );
  assert.equal(requests[4].method, "DELETE");
  assert.equal(requests[4].url, `${base}/${domain.id}`);
  assert.equal(requests[4].headers.get("idempotency-key"), "idempotency-key-domain-delete");
});
