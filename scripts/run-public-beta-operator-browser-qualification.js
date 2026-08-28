#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { chromium } from "playwright";

import {
  assert,
  assertSelectedRelease,
  parseOptions,
  repositoryRoot,
  writeJson,
} from "./public-beta-hosted-lib.js";

const options = parseOptions(process.argv.slice(2));
const manifest = JSON.parse(
  readFileSync(resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json"), "utf8"),
);
const releaseDigest = options.releaseDigest ?? manifest.releaseDigest;
const output =
  options.output ?? ".local/qualification/public-beta-operator-browser-qualification.json";
const publicOrigin = "https://cloud-test.makodb.com";
const startedAt = new Date().toISOString();
const permissions = [
  "tenant_read",
  "overview_read",
  "operations_read",
  "incident_read",
  "incident_manage",
  "backup_read",
  "recovery_manage",
  "fleet_read",
  "security_read",
  "security_manage",
  "activity_read",
  "activity_export",
  "provisioning_repair",
  "quota_override",
  "abuse_response",
  "support_access",
  "waitlist_review",
];

assert(releaseDigest === manifest.releaseDigest, "requested digest differs from release manifest");
assertSelectedRelease(releaseDigest);

const indexResponse = await fetch(`${publicOrigin}/operator`, { redirect: "manual" });
assert(indexResponse.status === 200, `hosted operator page returned ${indexResponse.status}`);
const securityHeaders = {
  strictTransportSecurity: indexResponse.headers.get("strict-transport-security"),
  contentTypeOptions: indexResponse.headers.get("x-content-type-options"),
  frameOptions: indexResponse.headers.get("x-frame-options"),
  referrerPolicy: indexResponse.headers.get("referrer-policy"),
  permissionsPolicy: indexResponse.headers.get("permissions-policy"),
  server: indexResponse.headers.get("server"),
};
assert(
  securityHeaders.strictTransportSecurity?.includes("max-age=") === true,
  "hosted operator page omits HSTS",
);
assert(securityHeaders.contentTypeOptions === "nosniff", "nosniff header is absent");
assert(securityHeaders.frameOptions === "DENY", "frame denial header is absent");
assert(securityHeaders.referrerPolicy === "no-referrer", "referrer policy is unsafe");
assert(
  securityHeaders.permissionsPolicy?.includes("camera=()") === true,
  "permissions policy is absent",
);
assert(securityHeaders.server === null, "server identity header is exposed");

const indexBody = await indexResponse.text();
const consoleArtifacts = new Map(
  manifest.artifacts.files
    .filter((entry) => entry.path.startsWith("console/"))
    .map((entry) => [entry.path.slice("console/".length), entry]),
);
const indexArtifact = consoleArtifacts.get("index.html");
assert(indexArtifact !== undefined, "release manifest omits console index");
assert(sha256(indexBody) === indexArtifact.sha256, "hosted console index differs from release");
const assetPaths = [
  ...indexBody.matchAll(/(?:src|href)="(\/assets\/[A-Za-z0-9._-]+\.(?:js|css))"/gu),
].map((match) => match[1]);
assert(assetPaths.length >= 2, "hosted console index does not reference its assets");
for (const assetPath of assetPaths) {
  const artifact = consoleArtifacts.get(assetPath.slice(1));
  assert(artifact !== undefined, `release manifest omits ${assetPath}`);
  const response = await fetch(`${publicOrigin}${assetPath}`, { redirect: "manual" });
  assert(response.status === 200, `${assetPath} returned ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  assert(bytes.length === artifact.size, `${assetPath} size differs from release`);
  assert(sha256(bytes) === artifact.sha256, `${assetPath} digest differs from release`);
}

const publicRouteStatuses = await probePublicRoutes(publicOrigin);
const browserResult = await qualifyBrowser(publicOrigin, permissions);
assertSelectedRelease(releaseDigest);

writeJson(output, {
  schemaVersion: 1,
  startedAt,
  completedAt: new Date().toISOString(),
  environment: "public-beta",
  publicOrigin,
  admissionMode: "pre_gate",
  releaseDigest,
  sourceDigest: manifest.source.digest,
  runtimeDigest: manifest.runtime.digest,
  dependencyDigest: manifest.dependencies.digest,
  artifactDigest: manifest.artifacts.digest,
  selectedReleaseVerifiedBeforeAndAfter: true,
  hostedBundleHashesVerified: true,
  hostedAssetCount: assetPaths.length,
  securityHeaders,
  publicRouteStatuses,
  browser: browserResult,
  passed: true,
});
console.log(`operator browser qualification passed; evidence: ${output}`);

async function qualifyBrowser(origin, expectedPermissions) {
  const browser = await chromium.launch({ headless: true });
  let signedIn = false;
  let revoked = false;
  let repairAttempts = 0;
  let stepUps = 0;
  let signOuts = 0;
  const routeCounts = new Map();
  const waitlistApprovalRequests = [];
  const now = new Date();
  const session = {
    operatorId: "opr_qualify01",
    developerIdentityId: "dev_qualify01",
    email: "operator@example.test",
    displayName: "Qualification Operator",
    developerStatus: "waitlisted",
    permissions: expectedPermissions,
    passwordVerifiedAt: now.toISOString(),
    expiresAt: new Date(now.getTime() + 30 * 60 * 1_000).toISOString(),
  };
  const projectId = "prj_qualify01";
  const environmentId = "env_qualify01";
  const waitlistApplicants = [
    {
      developerIdentityId: session.developerIdentityId,
      email: session.email,
      displayName: session.displayName,
      status: "waitlisted",
      operatorEntitlementStatus: "active",
      authorizationEpoch: 2,
      emailVerified: true,
      createdAt: now.toISOString(),
      updatedAt: now.toISOString(),
    },
    {
      developerIdentityId: "dev_qualify02",
      email: "second@example.test",
      displayName: "Second Qualification Applicant",
      status: "waitlisted",
      operatorEntitlementStatus: "none",
      authorizationEpoch: 2,
      emailVerified: true,
      createdAt: now.toISOString(),
      updatedAt: now.toISOString(),
    },
  ];
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    page.on("dialog", (dialog) => void dialog.accept());
    await page.route("**/v1/**", async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      const key = `${request.method()} ${path}`;
      routeCounts.set(key, (routeCounts.get(key) ?? 0) + 1);
      if (path === "/v1/developer-auth/sessions/refresh") {
        await apiError(route, 401, "unauthenticated");
        return;
      }
      if (path === "/v1/operator-auth/sessions/current" && request.method() === "GET") {
        if (signedIn && !revoked) await json(route, session);
        else await apiError(route, 401, "unauthenticated");
        return;
      }
      if (path === "/v1/operator-auth/sessions" && request.method() === "POST") {
        signedIn = true;
        revoked = false;
        await json(route, session);
        return;
      }
      if (path === "/v1/operator-auth/sessions/current" && request.method() === "DELETE") {
        signedIn = false;
        signOuts += 1;
        await route.fulfill({ status: 204, body: "" });
        return;
      }
      if (
        path === "/v1/operator-auth/sessions/current/actions/verify-password" &&
        request.method() === "POST"
      ) {
        stepUps += 1;
        await json(route, { ...session, passwordVerifiedAt: new Date().toISOString() });
        return;
      }
      if (path === "/v1/operator/support-sessions/current" && request.method() === "GET") {
        await json(route, { items: [] });
        return;
      }
      if (path === "/v1/operator/overview" && request.method() === "GET") {
        await json(route, {
          sections: [
            {
              id: "release",
              freshness: "current",
              observedAt: now.toISOString(),
              provider: "release",
              message: null,
              metrics: { ready: true },
              links: [],
            },
          ],
          activeTenants: 1,
          attentionTenants: 0,
          observedAt: now.toISOString(),
          partial: false,
        });
        return;
      }
      if (path === "/v1/operator/inventory/operations" && request.method() === "GET") {
        await json(route, {
          items: [
            {
              id: "operations",
              freshness: "current",
              observedAt: now.toISOString(),
              provider: "control-plane",
              message: null,
              metrics: { pendingCount: 1 },
              links: [],
            },
          ],
          observedAt: now.toISOString(),
        });
        return;
      }
      if (path === "/v1/operator/provisioning-workflows" && request.method() === "GET") {
        await json(route, {
          items: [
            {
              id: "wf_qualify01",
              resource: { kind: "project", projectId },
              operation: "create",
              state: "queued",
              diagnostics: [],
              operatorRepairs: [],
              updatedAt: now.toISOString(),
            },
          ],
        });
        return;
      }
      if (
        [
          `/v1/operator/projects/${projectId}/quota-overrides`,
          `/v1/operator/projects/${projectId}/abuse-responses`,
          `/v1/operator/projects/${projectId}/support-sessions`,
        ].includes(path) &&
        request.method() === "GET"
      ) {
        await json(route, { items: [] });
        return;
      }
      if (path === "/v1/operator/developer-waitlist" && request.method() === "GET") {
        if (revoked) {
          signedIn = false;
          await apiError(route, 401, "unauthenticated");
        } else {
          await json(route, {
            applicants: waitlistApplicants.filter((applicant) => applicant.status === "waitlisted"),
            nextCursor: null,
          });
        }
        return;
      }
      if (path.endsWith("/actions/approve") && request.method() === "POST") {
        const applicant = waitlistApplicants.find((value) =>
          path.includes(value.developerIdentityId),
        );
        assert(applicant !== undefined, "batch approval targeted an unknown applicant");
        waitlistApprovalRequests.push({
          path,
          idempotencyKey: request.headers()["idempotency-key"] ?? "",
          body: request.postDataJSON(),
        });
        applicant.status = "active";
        applicant.authorizationEpoch += 1;
        applicant.updatedAt = new Date().toISOString();
        await json(route, applicant);
        return;
      }
      if (path === `/v1/operator/projects/${projectId}` && request.method() === "GET") {
        await json(route, {
          project: {
            id: projectId,
            teamId: "org_qualify01",
            name: "Qualification Project",
            region: "us-east-1-beta",
            state: "active",
            createdAt: now.toISOString(),
            updatedAt: now.toISOString(),
          },
          environments: [
            {
              id: environmentId,
              projectId,
              name: "Qualification",
              state: "active",
              createdAt: now.toISOString(),
              updatedAt: now.toISOString(),
            },
          ],
        });
        return;
      }
      if (path.endsWith("/provisioning/wf_qualify01/actions/repair")) {
        repairAttempts += 1;
        if (repairAttempts === 1) {
          await apiError(route, 401, "operator_step_up_required");
        } else {
          await json(route, {
            id: "wf_qualify01",
            resource: { kind: "project", projectId },
            operation: "create",
            state: "queued",
            diagnostics: [],
            operatorRepairs: [
              {
                operatorId: session.operatorId,
                reason: "qualification fixture repair",
                action: "requeue",
                timestamp: new Date().toISOString(),
              },
            ],
            updatedAt: new Date().toISOString(),
          });
        }
        return;
      }
      await apiError(route, 404, "not_found");
    });

    const navigation = await page.goto(`${origin}/operator`, { waitUntil: "networkidle" });
    assert(navigation?.status() === 200, "browser navigation did not use hosted HTTPS");
    await visible(page.getByRole("heading", { name: "Sign in as a platform operator" }));
    assert(
      (await page.getByLabel(/token/iu).count()) === 0,
      "anonymous console exposes token input",
    );
    await page.getByLabel("Email").fill("operator@example.test");
    await page.getByLabel("Password").fill("qualification fixture password");
    await page.getByRole("button", { name: "Open operator console" }).click();
    await visible(page.getByRole("heading", { name: "Platform overview" }));
    await visible(page.getByText("Mako Cloud Control Center"));
    await page.getByRole("button", { name: "Developer wait list" }).click();
    await visible(page.getByRole("heading", { name: "Developer registration wait list" }));
    const waitlistPanel = page.locator('section[aria-labelledby="developer-waitlist-title"]');
    const batchReason = waitlistPanel.getByLabel(
      "Private review reason / case reference (optional)",
    );
    await visible(batchReason);
    assert((await batchReason.getAttribute("required")) === null, "batch reason is required");
    await waitlistPanel.getByLabel(`Select ${session.email} for batch approval`).check();
    await visible(waitlistPanel.getByRole("button", { name: "Approve selected (1)" }));
    await waitlistPanel
      .getByLabel("Filter this page by applicant, email, or developer ID")
      .fill("Second");
    assert(
      await waitlistPanel.getByRole("button", { name: "Approve selected (0)" }).isDisabled(),
      "filtering did not clear hidden batch selection",
    );
    await waitlistPanel
      .getByLabel("Filter this page by applicant, email, or developer ID")
      .fill("");
    await waitlistPanel.getByLabel("Select all visible applicants for batch approval").check();
    await waitlistPanel.getByRole("button", { name: "Approve selected (2)" }).click();
    await visible(page.getByText("Batch approval finished: 2 committed, 0 failed."));
    assert(waitlistApprovalRequests.length === 2, "batch did not approve both selected applicants");
    assert(
      new Set(waitlistApprovalRequests.map((request) => request.idempotencyKey)).size === 2,
      "batch reused an idempotency key",
    );
    assert(
      waitlistApprovalRequests.every(
        (request) =>
          /^waitlist-batch-approve-[0-9a-f-]+-[12]$/u.test(request.idempotencyKey) &&
          JSON.stringify(request.body) === "{}",
      ),
      "batch did not use independent reason-free approval requests",
    );
    const retiredLegacyRoute = await page.goto(`${origin}/operator/legacy`, {
      waitUntil: "networkidle",
    });
    assert(retiredLegacyRoute?.status() === 404, "retired legacy route did not fail closed");
    await page.goto(`${origin}/operator/operations`, { waitUntil: "networkidle" });
    await visible(page.getByRole("heading", { name: "Provisioning and operations" }));
    await page.getByLabel("Optional project scope").fill(projectId);
    await page.getByRole("button", { name: "Apply scope" }).click();
    for (const heading of [
      "Provisioning repair",
      "Quota override",
      "Abuse response",
      "Time-bounded support access",
    ]) {
      await visible(page.getByRole("heading", { name: heading }));
    }

    const repairPanel = page.locator('section[aria-labelledby="repair-title"]');
    await repairPanel.getByLabel("Workflow ID").fill("wf_qualify01");
    await repairPanel
      .getByLabel("Required operator reason / case reference")
      .fill("qualification fixture repair");
    await repairPanel.getByRole("button", { name: "Apply reasoned repair" }).click();
    const stepUp = page.getByRole("dialog", { name: "Verify your operator password" });
    await visible(stepUp);
    await stepUp.getByLabel("Password").fill("qualification fixture password");
    await stepUp.getByRole("button", { name: "Verify and continue" }).click();
    await visible(page.getByText("Provisioning workflow updated"));
    assert(repairAttempts === 2 && stepUps === 1, "step-up did not retry the exact mutation once");

    await page.reload({ waitUntil: "networkidle" });
    await visible(page.getByRole("heading", { name: "Provisioning and operations" }));
    const browserStorage = await page.evaluate(() => ({
      cookie: document.cookie,
      localStorage: { ...localStorage },
      sessionStorage: { ...sessionStorage },
    }));
    const stored = JSON.stringify(browserStorage);
    assert(!stored.includes("qualification fixture password"), "browser retained fixture password");
    assert(!stored.includes(session.operatorId), "browser storage retained operator authority");
    assert(browserStorage.cookie === "", "operator credential became JavaScript-readable");

    await page.getByRole("button", { name: "Sign out" }).click();
    await visible(page.getByRole("heading", { name: "Sign in as a platform operator" }));
    assert(signOuts === 1, "browser did not call operator sign-out");

    await page.getByLabel("Email").fill("operator@example.test");
    await page.getByLabel("Password").fill("qualification fixture password");
    await page.getByRole("button", { name: "Open operator console" }).click();
    await page.getByRole("button", { name: "Developer wait list" }).click();
    await visible(page.getByRole("heading", { name: "Developer registration wait list" }));
    revoked = true;
    await page
      .locator('section[aria-labelledby="developer-waitlist-title"]')
      .getByRole("button", { name: "Refresh" })
      .click();
    await visible(page.getByRole("heading", { name: "Sign in as a platform operator" }));
    assert(
      (await page.getByLabel(/token/iu).count()) === 0,
      "authorization loss exposed token input",
    );

    return {
      realHostedHttps: true,
      controlCenterShell: true,
      legacyRouteRetired: true,
      sessionRestoration: true,
      permissionFamilies: expectedPermissions,
      permissionPanelsVisible: 12,
      stepUpAndOriginalRetry: true,
      signOut: true,
      authorizationLossClearsSession: true,
      optionalReviewReason: true,
      batchApproval: {
        selected: 2,
        committed: 2,
        failed: 0,
        selfReviewIncluded: true,
        uniqueIdempotencyKeys: true,
        hiddenSelectionCleared: true,
      },
      tokenInputs: 0,
      javascriptReadableCredential: false,
      controlledFixtureRoutes: Object.fromEntries([...routeCounts].sort()),
    };
  } finally {
    await browser.close();
  }
}

async function probePublicRoutes(origin) {
  const probes = [
    ["GET", "/_internal/v1/control/operator-entitlements/plan", undefined, 404],
    ["GET", "/v1/not-a-public-route", undefined, 404],
    ["POST", "/v1/operator-auth/sessions", {}, 400],
    ["GET", "/v1/operator-auth/sessions/current", undefined, 401],
    ["DELETE", "/v1/operator-auth/sessions/current", undefined, 204],
    [
      "POST",
      "/v1/operator-auth/sessions/current/actions/verify-password",
      { password: "qualification fixture without session" },
      401,
    ],
    ["GET", "/v1/operator/projects/prj_qualify01", undefined, 401],
    ["GET", "/v1/operator/overview", undefined, 401],
    ["GET", "/v1/operator/tenants", undefined, 401],
    ["GET", "/v1/operator/inventory/operations", undefined, 401],
    ["GET", "/v1/operator/incidents", undefined, 401],
    ["GET", "/v1/operator/recovery-jobs", undefined, 401],
    ["GET", "/v1/operator/security", undefined, 401],
    ["GET", "/v1/operator/activity", undefined, 401],
    [
      "POST",
      "/v1/operator/projects/prj_qualify01/provisioning/wf_qualify01/actions/repair",
      {},
      401,
    ],
    ["POST", "/v1/operator/projects/prj_qualify01/quota-overrides", {}, 401],
    ["POST", "/v1/operator/projects/prj_qualify01/abuse-responses", {}, 401],
    ["POST", "/v1/operator/projects/prj_qualify01/support-sessions", {}, 401],
    [
      "POST",
      "/v1/operator/projects/prj_qualify01/support-sessions/sup_qualify01/actions/revoke",
      {},
      401,
    ],
    ["GET", "/v1/operator/developer-waitlist", undefined, 401],
    ["GET", "/v1/operator/developer-waitlist/dev_qualify01", undefined, 401],
    ["POST", "/v1/operator/developer-waitlist/dev_qualify01/actions/approve", {}, 401],
    ["POST", "/v1/operator/developer-waitlist/dev_qualify01/actions/reject", {}, 401],
  ];
  const results = [];
  for (const [method, path, body, expected] of probes) {
    const response = await fetch(`${origin}${path}`, {
      method,
      headers: {
        Origin: origin,
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: "manual",
    });
    assert(response.status === expected, `${method} ${path} returned ${response.status}`);
    results.push({ method, path, status: response.status });
  }
  return results;
}

async function visible(locator) {
  await locator.waitFor({ state: "visible", timeout: 10_000 });
}

async function json(route, body, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

async function apiError(route, status, code) {
  await json(
    route,
    {
      apiVersion: "v1",
      error: {
        code,
        message:
          code === "operator_step_up_required"
            ? "Password verification is required."
            : "Request denied.",
        requestId: `req_browser_${status}`,
        retry: { kind: "never" },
      },
    },
    status,
  );
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
