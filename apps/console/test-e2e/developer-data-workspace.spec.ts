import { expect, type Route, test } from "@playwright/test";

const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const OTHER_ENVIRONMENT_ID = "env_ijklmnop";
const CAPABILITY = "mx1.xcap-v1.sensitive_explorer_capability_never_persisted";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const session = {
      accessToken: "developer-session-token",
      expiresAt: "2099-01-01T00:00:00.000Z",
      audience: "mako-management",
      profile: { id: "dev_abcdefgh", email: "owner@example.test", displayName: "Owner" },
    };
    window.__MAKO_CONSOLE__ = {
      managementEndpoint: "http://127.0.0.1:4174",
      developerAuth: {
        loadSession: async () => session,
        beginSignIn: async () => {},
        signOut: async () => {},
        subscribe: () => () => {},
      },
      developerWorkspaceEnabled: true,
      developerExplorerAdminEnabled: true,
      developerDataJobsEnabled: true,
      developerSyncDetailsEnabled: true,
      developerRestoreEnabled: true,
    };
  });
});

test("direct browsing stays in memory, supports queries, and revokes on environment switch", async ({
  page,
}) => {
  const requests: { method: string; path: string; capability: string | undefined }[] = [];
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    requests.push({
      method: request.method(),
      path,
      capability: request.headers()["x-mako-explorer-capability"],
    });
    if (request.headers().authorization !== "Bearer developer-session-token")
      return json(route, {}, 401);
    if (path === `/v1/projects/${PROJECT_ID}`) return json(route, project());
    if (path === `/v1/projects/${PROJECT_ID}/environments`)
      return json(route, {
        items: [
          environment(ENVIRONMENT_ID, "development"),
          environment(OTHER_ENVIRONMENT_ID, "preview"),
        ],
      });
    if (path.endsWith("/workspace/navigation"))
      return json(
        route,
        navigation(path.includes(OTHER_ENVIRONMENT_ID) ? OTHER_ENVIRONMENT_ID : ENVIRONMENT_ID),
      );
    if (path.endsWith("/collections") && request.method() === "GET")
      return json(route, { items: [collection()] });
    if (path.endsWith("/users") && request.method() === "GET")
      return json(route, {
        users: [
          {
            id: "usr_abcdefgh",
            email: "app-user@example.test",
            status: "active",
            createdAt: "2026-08-13T00:00:00Z",
            updatedAt: "2026-08-13T00:00:00Z",
          },
        ],
        truncated: false,
      });
    if (path.endsWith("/explorer/grants") && request.method() === "POST")
      return json(
        route,
        {
          grantId: "xgr_0123456789abcdef0123456789abcdef",
          capability: CAPABILITY,
          mode: "administrative",
          operations: ["get", "browse", "query", "plan", "simulate"],
          applicationUserId: null,
          issuedAtUnixSeconds: 1_786_579_200,
          expiresAtUnixSeconds: 4_102_444_800,
          authorizationEpoch: 1,
        },
        201,
      );
    if (path.includes("/explorer/grants/") && request.method() === "DELETE")
      return json(route, {
        grantId: "xgr_0123456789abcdef0123456789abcdef",
        revokedAtUnixSeconds: 1_786_579_201,
      });
    if (path.endsWith("/browse"))
      return json(route, {
        items: [document()],
        nextCursor: null,
        snapshot: "snapshot-safe",
        exhausted: true,
      });
    if (path.endsWith("/query/plan"))
      return json(route, {
        supported: true,
        indexName: "by_owner",
        effectiveOrder: [],
        effectiveLimit: 25,
        queryFingerprint: "query-fingerprint",
        requiredIndex: null,
      });
    if (path.endsWith("/query"))
      return json(route, {
        items: [document()],
        nextCursor: null,
        snapshot: "snapshot-safe",
        exhausted: true,
      });
    if (path.endsWith("/data-jobs") && request.method() === "GET")
      return json(route, { items: [], nextCursor: null });
    if (path.endsWith("/simulate"))
      return json(route, {
        allowed: true,
        schemaValid: true,
        diagnostics: [],
        wouldConflict: false,
      });
    return json(route, { error: "unhandled" }, 500);
  });

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
  await expect(page.getByRole("link", { name: "Skip to main content" })).toHaveAttribute(
    "href",
    "#main-content",
  );
  await expect(page.getByRole("heading", { name: "Create scoped access" })).toHaveCount(0);
  await expect(page.getByLabel("Application user", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Access reason")).toHaveCount(0);
  await expect(page.getByText("doc_visible01")).toBeVisible();
  await expect(page.getByText("doc_hidden01")).toHaveCount(0);
  await page.getByRole("button", { name: "View JSON" }).click();
  await page.getByRole("button", { name: "Parse, validate, and simulate" }).click();
  await expect(page.getByText("Simulation allowed")).toBeVisible();
  await expect(page.getByRole("button", { name: "Commit conditionally" })).toBeVisible();
  await page.getByRole("tab", { name: "Query editor" }).click();
  await page.getByLabel("Field", { exact: true }).fill("ownerId");
  await page.getByLabel("JSON value").fill('"usr_abcdefgh"');
  await page.getByRole("button", { name: "Plan and run query" }).click();
  await expect(page.getByText("Using index")).toBeVisible();

  const browserState = await page.evaluate(() => ({
    url: window.location.href,
    local: Object.entries(localStorage),
    session: Object.entries(sessionStorage),
  }));
  expect(JSON.stringify(browserState)).not.toContain(CAPABILITY);
  expect(browserState.url).not.toContain("mx1.");
  expect(requests.some((request) => request.path.endsWith("/users"))).toBe(false);
  expect(requests.filter((request) => request.path.endsWith("/browse"))).toEqual([
    expect.objectContaining({ capability: CAPABILITY }),
  ]);

  await page.getByLabel("Switch environment").selectOption(OTHER_ENVIRONMENT_ID);
  await expect(page).toHaveURL(new RegExp(`${OTHER_ENVIRONMENT_ID}/overview$`, "u"));
  await expect
    .poll(() =>
      requests.some(
        (request) => request.method === "DELETE" && request.path.includes("/explorer/grants/"),
      ),
    )
    .toBe(true);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole("complementary", { name: "Environment navigation" })).toBeVisible();
  expect(
    await page.evaluate(
      () => globalThis.document.documentElement.scrollWidth <= window.innerWidth + 1,
    ),
  ).toBe(true);
});

for (const kind of ["personal", "team"] as const) {
  test(`${kind} projects load documents directly without application users`, async ({ page }) => {
    const bodies: Array<Record<string, unknown>> = [];
    const unexpected: string[] = [];
    await page.route("**/v1/**", async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (path === `/v1/projects/${PROJECT_ID}`)
        return json(route, {
          ...project(),
          teamId: kind === "personal" ? "org_personal" : "org_abcdefgh",
        });
      if (await serveWorkspaceShell(route, path)) return;
      if (path.endsWith("/collections")) return json(route, { items: [collection()] });
      if (path.endsWith("/explorer/grants")) {
        bodies.push(request.postDataJSON() as Record<string, unknown>);
        return json(route, automaticGrant(), 201);
      }
      if (path.endsWith("/browse")) return json(route, documentPage());
      if (path.includes("/explorer/grants/")) return json(route, {});
      unexpected.push(path);
      return json(route, {}, 500);
    });
    await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
    await expect(page.getByText("doc_visible01")).toBeVisible();
    expect(bodies).toEqual([
      {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        collectionId: "todos",
        mode: "administrative",
        applicationUserId: null,
        operations: ["get", "browse", "query", "plan", "history", "simulate", "mutate"],
        durationSeconds: 300,
        reason: "Browse and manage documents in the cloud console",
      },
    ]);
    expect(unexpected).toEqual([]);
    await expect(page.getByRole("button", { name: "Create access grant" })).toHaveCount(0);
  });
}

test("expired access renews automatically before the next document request", async ({ page }) => {
  let grants = 0;
  const capabilities: Array<string | undefined> = [];
  await page.clock.install();
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    if (path.endsWith("/explorer/grants")) {
      grants += 1;
      return json(
        route,
        {
          ...automaticGrant(),
          grantId: `grant-${grants}`,
          capability: `${CAPABILITY}-${grants}`,
          expiresAtUnixSeconds: grants === 1 ? Math.floor(Date.now() / 1000) + 300 : 4_102_444_800,
        },
        201,
      );
    }
    if (path.includes("/explorer/grants/")) return json(route, {});
    if (path.endsWith("/browse")) {
      capabilities.push(request.headers()["x-mako-explorer-capability"]);
      return json(route, documentPage());
    }
    return json(route, {}, 500);
  });
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
  await expect(page.getByText("doc_visible01")).toBeVisible();
  await page.clock.fastForward(301_000);
  await page.getByRole("button", { name: "Refresh documents" }).click();
  await expect.poll(() => capabilities.length).toBe(2);
  expect(capabilities).toEqual([`${CAPABILITY}-1`, `${CAPABILITY}-2`]);
  expect(grants).toBe(2);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

for (const status of [401, 503]) {
  test(`access failure ${status} does not request data and can be retried`, async ({ page }) => {
    let allowed = false;
    let grants = 0;
    let reads = 0;
    await page.route("**/v1/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (await serveWorkspaceShell(route, path)) return;
      if (path.endsWith("/collections")) return json(route, { items: [collection()] });
      if (path.endsWith("/explorer/grants")) {
        grants += 1;
        return allowed
          ? json(route, automaticGrant(), 201)
          : json(
              route,
              {
                apiVersion: "v1",
                error: {
                  code:
                    status === 403
                      ? "permission_denied"
                      : status === 401
                        ? "unauthenticated"
                        : "unavailable",
                  message: "Document access is unavailable.",
                  requestId: "req_access_denied",
                  retry: { kind: "never" },
                },
              },
              status,
            );
      }
      if (path.endsWith("/browse")) {
        reads += 1;
        return json(route, documentPage());
      }
      if (path.includes("/explorer/grants/")) return json(route, {});
      return json(route, {}, 500);
    });
    await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
    await expect(page.getByRole("alert")).toContainText("Document access is unavailable.");
    expect(reads).toBe(0);
    expect(grants).toBe(1);
    allowed = true;
    await page.getByRole("button", { name: "Retry loading documents" }).click();
    await expect(page.getByText("doc_visible01")).toBeVisible();
    expect(grants).toBe(2);
    expect(reads).toBe(1);
  });
}

test("a developer refused administrative access previews the collection as an application user", async ({
  page,
}) => {
  const grantRequests: Record<string, unknown>[] = [];
  let reads = 0;
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    if (path.endsWith("/users") && request.method() === "GET") {
      return json(route, {
        truncated: false,
        users: [
          {
            id: "usr_riya0001",
            email: "riya@example.test",
            status: "active",
            createdAt: "2026-08-06T12:00:00.000Z",
            updatedAt: "2026-08-06T12:00:00.000Z",
          },
          {
            id: "usr_gone0001",
            email: "gone@example.test",
            status: "disabled",
            createdAt: "2026-08-06T12:00:00.000Z",
            updatedAt: "2026-08-06T12:00:00.000Z",
          },
        ],
      });
    }
    if (path.endsWith("/explorer/grants")) {
      const body = request.postDataJSON() as Record<string, unknown>;
      grantRequests.push(body);
      if (body.mode === "administrative") {
        return json(
          route,
          {
            apiVersion: "v1",
            error: {
              code: "permission_denied",
              message: "explorer action is forbidden",
              requestId: "req_denied",
              retry: { kind: "never" },
            },
          },
          403,
        );
      }
      return json(
        route,
        {
          ...automaticGrant(),
          mode: "policy_preview",
          operations: ["get", "browse", "query", "plan", "simulate"],
          applicationUserId: "usr_riya0001",
        },
        201,
      );
    }
    if (path.endsWith("/browse")) {
      reads += 1;
      return json(route, documentPage());
    }
    if (path.includes("/explorer/grants/")) return json(route, {});
    return json(route, {}, 500);
  });
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
  await expect(page.getByText("Browsing every document needs an administrator")).toBeVisible();
  await expect(page.getByText("explorer action is forbidden")).toHaveCount(0);
  expect(reads).toBe(0);
  // Only active users are offered.
  await expect(page.getByLabel("Preview as").locator("option")).toHaveText([
    "Choose an application user",
    "riya@example.test",
  ]);
  await page.getByLabel("Preview as").selectOption("usr_riya0001");
  await page.getByRole("button", { name: "Preview as this user" }).click();
  await expect(page.getByText("doc_visible01")).toBeVisible();
  await expect(
    page.getByText("Showing what riya@example.test may read under the active policy."),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "New document" })).toHaveCount(0);
  expect(grantRequests.at(-1)).toMatchObject({
    mode: "policy_preview",
    applicationUserId: "usr_riya0001",
    reason: null,
    operations: ["get", "browse", "query", "plan", "simulate"],
  });
  expect(reads).toBe(1);
});

for (const state of ["empty", "disabled"] as const) {
  test(`${state} explorer does not issue access or read documents`, async ({ page }) => {
    if (state === "disabled")
      await page.addInitScript(() => {
        if (window.__MAKO_CONSOLE__) window.__MAKO_CONSOLE__.developerExplorerAdminEnabled = false;
      });
    const unexpected: string[] = [];
    await page.route("**/v1/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (await serveWorkspaceShell(route, path)) return;
      if (path.endsWith("/collections"))
        return json(route, { items: state === "empty" ? [] : [collection()] });
      unexpected.push(path);
      return json(route, {}, 500);
    });
    await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
    await expect(
      page.getByText(
        state === "empty"
          ? "Create and activate a collection schema before opening the explorer."
          : "Document browsing is unavailable in this deployment.",
      ),
    ).toBeVisible();
    expect(unexpected).toEqual([]);
  });
}

test("revoked access is discarded and reauthorized on the next explicit action", async ({
  page,
}) => {
  let grants = 0;
  const capabilities: Array<string | undefined> = [];
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    if (path.endsWith("/explorer/grants")) {
      grants += 1;
      return json(
        route,
        { ...automaticGrant(), grantId: `grant-${grants}`, capability: `${CAPABILITY}-${grants}` },
        201,
      );
    }
    if (path.includes("/explorer/grants/")) return json(route, {});
    if (path.endsWith("/browse")) {
      capabilities.push(request.headers()["x-mako-explorer-capability"]);
      if (capabilities.length === 2)
        return json(
          route,
          {
            apiVersion: "v1",
            error: {
              code: "unauthenticated",
              message: "explorer capability is invalid",
              requestId: "req_revoked",
              retry: { kind: "never" },
            },
          },
          401,
        );
      return json(route, documentPage());
    }
    return json(route, {}, 500);
  });
  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
  await expect(page.getByText("doc_visible01")).toBeVisible();
  await page.getByRole("button", { name: "Refresh documents" }).click();
  await expect(page.getByRole("alert")).toContainText("explorer capability is invalid");
  expect(capabilities).toHaveLength(2);
  expect(grants).toBe(1);
  await page.getByRole("button", { name: "Refresh documents" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(capabilities).toEqual([`${CAPABILITY}-1`, `${CAPABILITY}-1`, `${CAPABILITY}-2`]);
  expect(grants).toBe(2);
});

test("direct access preserves audited conflicts, history, tombstones, and data jobs", async ({
  page,
}) => {
  let confirmedImport = false;
  let exportCreated = false;
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/collections") && request.method() === "GET")
      return json(route, { items: [collection()] });
    if (path.endsWith("/users") && request.method() === "GET")
      return json(route, {
        users: [
          {
            id: "usr_abcdefgh",
            email: "app-user@example.test",
            status: "active",
            createdAt: "2026-08-13T00:00:00Z",
            updatedAt: "2026-08-13T00:00:00Z",
          },
        ],
        truncated: false,
      });
    if (path.endsWith("/explorer/grants") && request.method() === "POST")
      return json(
        route,
        {
          grantId: "xgr_admin0123456789abcdef0123456789ab",
          capability: CAPABILITY,
          mode: "administrative",
          operations: ["get", "browse", "query", "plan", "history", "simulate", "mutate"],
          applicationUserId: null,
          issuedAtUnixSeconds: 1_786_579_200,
          expiresAtUnixSeconds: 4_102_444_800,
          authorizationEpoch: 1,
        },
        201,
      );
    if (path.includes("/explorer/grants/") && request.method() === "DELETE")
      return json(route, {
        grantId: "xgr_admin0123456789abcdef0123456789ab",
        revokedAtUnixSeconds: 1_786_579_201,
      });
    if (path.endsWith("/browse"))
      return json(route, {
        items: [document(), tombstone()],
        nextCursor: null,
        snapshot: "snapshot-admin-safe",
        exhausted: true,
      });
    if (path.endsWith("/history"))
      return json(route, [
        {
          revision: "rev_previous01",
          schemaVersion: 1,
          commitPosition: 20,
          committedAtUnixSeconds: 1_786_492_800,
          deleted: false,
          retainedUntilUnixSeconds: null,
        },
        {
          revision: "rev_deleted01",
          schemaVersion: 1,
          commitPosition: 21,
          committedAtUnixSeconds: 1_786_579_100,
          deleted: true,
          retainedUntilUnixSeconds: 4_102_444_800,
        },
      ]);
    if (path.endsWith("/mutate"))
      return json(route, {
        committed: false,
        document: null,
        conflict: {
          ...document(),
          revision: "rev_concurrent01",
          content: { id: "doc_visible01", ownerId: "usr_abcdefgh", title: "Concurrent" },
        },
        auditReference: "audit_admin_mutation01",
      });
    if (path.endsWith("/data-jobs") && request.method() === "GET")
      return json(route, {
        items: [
          importJob(confirmedImport ? "queued" : "awaiting_confirmation"),
          exportJob(),
          ...(exportCreated ? [{ ...exportJob(), jobId: "djob_export02", state: "queued" }] : []),
        ],
        nextCursor: null,
      });
    if (path.endsWith("/data-jobs") && request.method() === "POST") {
      exportCreated = true;
      return json(route, { ...exportJob(), jobId: "djob_export02", state: "queued" }, 201);
    }
    if (path.endsWith("/confirm") && request.method() === "POST") {
      confirmedImport = true;
      return json(route, importJob("queued"));
    }
    return json(route, { error: "unhandled" }, 500);
  });

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/data`);
  await expect(page.getByText("doc_visible01")).toBeVisible();
  await page.getByRole("button", { name: "Include retained tombstones" }).click();
  await expect(page.getByRole("cell", { name: "retained tombstone" })).toBeVisible();
  await page.getByRole("button", { name: "History" }).first().click();
  await expect(page.getByText(/deleted; retained until/u)).toBeVisible();
  await page.getByRole("button", { name: "View JSON" }).first().click();
  await page.getByRole("button", { name: "Commit conditionally" }).click();
  await expect(
    page.getByRole("heading", { name: "Revision conflict—no automatic merge was attempted" }),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "Original" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Proposed" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Current", exact: true })).toBeVisible();
  await expect(page.getByText("audit_admin_mutation01")).toBeVisible();

  await page.getByRole("tab", { name: "Import / export" }).click();
  await page.getByRole("button", { name: "Review execution" }).click();
  await expect(page.getByText("Confirm import execution")).toBeVisible();
  await page
    .getByLabel(
      "I understand cancellation stops future rows and does not roll back committed rows.",
    )
    .check();
  await page.getByRole("button", { name: "Confirm execution" }).click();
  await expect.poll(() => confirmedImport).toBe(true);
  await page.getByLabel("Export the full authorized collection snapshot").check();
  await page.getByRole("button", { name: "Create export" }).click();
  await expect.poll(() => exportCreated).toBe(true);
  await expect(page.getByText("djob_export02", { exact: true })).toBeVisible();
});

test("overview preserves healthy sections when a provider is unavailable", async ({ page }) => {
  await page.route("**/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/workspace/summary")) {
      return json(route, {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        sections: {
          lifecycle: {
            status: "current",
            observedAtUnixSeconds: 1_786_579_200,
            freshUntilUnixSeconds: 1_786_579_260,
            payload: { ready: true, environment: "active" },
          },
          sync: {
            status: "unavailable",
            observedAtUnixSeconds: 1_786_579_200,
            freshUntilUnixSeconds: 1_786_579_200,
            retainedSinceUnixSeconds: null,
            remediationCode: "sync_provider_unavailable",
          },
        },
      });
    }
    if (path.endsWith("/collections")) return json(route, { items: [collection()] });
    return json(route, { error: "unhandled" }, 500);
  });

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/overview`);
  const environment = page.getByRole("article", { name: "Environment", exact: true });
  await expect(environment.getByText("Active", { exact: true })).toBeVisible();
  await expect(environment.getByText("Stale", { exact: true })).toBeVisible();
  await expect(page.getByText("Sync error records")).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "Collection inventory" })
      .getByRole("link", { name: "todos", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Unavailable", { exact: true }).first()).toBeVisible();
});

test("Connect uses live metadata and renders safe compatibility remediation", async ({ page }) => {
  let connectionInput: unknown;
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/connect") && request.method() === "GET") {
      return json(route, {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        publicEndpoint: "https://cloud-test.makodb.com",
        publicKeyId: "key_public01",
        publicKey: "",
        collections: [{ collectionId: "todos", activeSchemaVersion: 7 }],
        rxdbClientRange: ">=17 <18",
        templateVersion: 1,
      });
    }
    if (path.endsWith("/connect/check") && request.method() === "POST") {
      connectionInput = request.postDataJSON();
      return json(route, {
        checkedAtUnixSeconds: 1_786_579_200,
        steps: [
          { id: "dns_tls", state: "passed", remediationCode: null, retryable: false },
          {
            id: "schema_compatibility",
            state: "failed",
            remediationCode: "schema_mismatch",
            retryable: false,
          },
          {
            id: "client_compatibility",
            state: "failed",
            remediationCode: "unsupported_client",
            retryable: false,
          },
        ],
      });
    }
    return json(route, { error: "unhandled" }, 500);
  });

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/connect`);
  await expect(page.getByText("No recoverable public key is available")).toBeVisible();
  await expect(page.locator("pre")).toContainText("schemaVersion: 7");
  await expect(page.locator("pre")).toContainText(
    'publicProjectKey: "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY"',
  );
  await page.getByRole("button", { name: "Run check" }).click();
  await expect(page.getByText("Schema mismatch")).toBeVisible();
  await expect(page.getByText("Unsupported client")).toBeVisible();
  expect(connectionInput).toEqual({
    publicKeyId: "key_public01",
    collectionId: "todos",
    schemaVersion: 7,
    rxdbVersion: "17.0.0",
  });
});

test("sync filters and isolated restore safeguards remain tenant scoped", async ({ page }) => {
  const syncQueries: URLSearchParams[] = [];
  let restoreInput: Record<string, unknown> | undefined;
  let restoreCreated = false;
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (await serveWorkspaceShell(route, path)) return;
    if (path.endsWith("/collections") && request.method() === "GET")
      return json(route, { items: [collection()] });
    if (path.endsWith("/sync/summary")) {
      syncQueries.push(new URLSearchParams(url.search));
      return json(route, {
        tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
        collectionId: url.searchParams.get("collectionId"),
        windowStartUnixSeconds: Number(url.searchParams.get("from")),
        windowEndUnixSeconds: Number(url.searchParams.get("until")),
        observedAtUnixSeconds: 1_786_579_200,
        retainedSinceUnixSeconds: 1_786_492_800,
        pullCount: 12,
        pushCount: 3,
        liveStreams: 1,
        lagP95Milliseconds: 14,
        conflicts: 0,
        policyDenials: 2,
        throttled: 0,
        checkpointExpired: 0,
        streamGaps: 0,
        resyncs: 0,
        schemaMismatches: 0,
        clientVersionClasses: { supported: 5 },
      });
    }
    if (path.endsWith("/backups")) return json(route, [backup()]);
    if (path === `/v1/projects/${PROJECT_ID}/restore-requests` && request.method() === "GET")
      return json(route, restoreCreated ? [restore()] : []);
    if (path === "/v1/developer-auth/sessions/current/actions/verify-password")
      return json(route, {
        token: "dst1_short_lived_step_up",
        expiresAtUnixSeconds: 4_102_444_800,
      });
    if (path === `/v1/projects/${PROJECT_ID}/restore-requests` && request.method() === "POST") {
      restoreInput = request.postDataJSON() as Record<string, unknown>;
      restoreCreated = true;
      return json(route, restore(), 201);
    }
    return json(route, { error: "unhandled" }, 500);
  });

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/sync`);
  await page.getByLabel("Collection").selectOption("todos");
  await page.getByLabel("Time window").selectOption("6");
  await page.getByRole("button", { name: "Apply" }).click();
  await expect(page.getByText("Policy denials are non-retryable")).toBeVisible();
  await expect
    .poll(() =>
      syncQueries.some(
        (query) =>
          query.get("collectionId") === "todos" &&
          Number(query.get("until")) - Number(query.get("from")) === 21_600,
      ),
    )
    .toBe(true);

  await page.goto(`/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/backups`);
  await expect(page.getByText("backup_verified01", { exact: true })).toBeVisible();
  await expect(page.getByText("Overwrite and promotion are prohibited.")).toBeVisible();
  await page.getByLabel("New environment name").fill("isolated-review");
  await page.getByLabel("Reason").fill("Validate the recovery point without touching production");
  await page.getByLabel("Confirm your developer password").fill("correct horse battery staple");
  await page.getByRole("button", { name: "Request recovery" }).click();
  await expect(page.getByText("env_recovery_safe01")).toBeVisible();
  expect(restoreInput).toMatchObject({
    environmentId: ENVIRONMENT_ID,
    backupId: "backup_verified01",
    targetEnvironmentName: "isolated-review",
    stepUpToken: "dst1_short_lived_step_up",
  });
  expect(restoreInput).not.toHaveProperty("overwritePermitted");
  expect(restoreInput).not.toHaveProperty("promotionPermitted");
  expect(restoreInput).not.toHaveProperty("targetEnvironmentId");
});

function project() {
  return {
    id: PROJECT_ID,
    teamId: "org_abcdefgh",
    name: "Workspace project",
    region: "local",
    state: "active",
    createdAt: "2026-08-13T00:00:00Z",
    updatedAt: "2026-08-13T00:00:00Z",
  };
}
function environment(id: string, name: string) {
  return {
    id,
    projectId: PROJECT_ID,
    name,
    state: "active",
    createdAt: "2026-08-13T00:00:00Z",
    updatedAt: "2026-08-13T00:00:00Z",
  };
}
function collection() {
  return {
    id: "todos",
    metadataVersion: 1,
    schemaVersion: 1,
    jsonSchema: {
      type: "object",
      properties: { id: { type: "string" }, ownerId: { type: "string" } },
      required: ["id", "ownerId"],
    },
    primaryKey: { kind: "field", field: "id" },
    compatibility: "compatible",
    state: "active",
  };
}
function document() {
  return {
    documentId: "doc_visible01",
    revision: "rev_visible01",
    schemaVersion: 1,
    deleted: false,
    content: { id: "doc_visible01", ownerId: "usr_abcdefgh", title: "Visible" },
  };
}
function tombstone() {
  return {
    documentId: "doc_deleted01",
    revision: "rev_deleted01",
    schemaVersion: 1,
    deleted: true,
    content: null,
  };
}
function importJob(state: "awaiting_confirmation" | "queued") {
  return dataJob({
    jobId: "djob_import01",
    kind: "import",
    state,
    conflictStrategy: "create_only",
    progress: { processed: 2, committed: 0, failed: 0, skipped: 0, exported: 0, bytes: 64 },
    manifest: {
      formatVersion: 1,
      tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
      collectionId: "todos",
      schemaVersion: 1,
      snapshot: null,
      rowCount: 2,
      byteCount: 64,
      digest: "sha256:import_manifest",
      finalizedAtUnixSeconds: 1_786_579_200,
    },
  });
}
function exportJob() {
  return dataJob({
    jobId: "djob_export01",
    kind: "export",
    state: "succeeded",
    conflictStrategy: null,
    progress: { processed: 2, committed: 0, failed: 0, skipped: 0, exported: 2, bytes: 64 },
    manifest: {
      formatVersion: 1,
      tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
      collectionId: "todos",
      schemaVersion: 1,
      snapshot: "snapshot-export",
      rowCount: 2,
      byteCount: 64,
      digest: "sha256:export_manifest",
      finalizedAtUnixSeconds: 1_786_579_200,
    },
  });
}
function dataJob(input: Record<string, unknown>) {
  return {
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    collectionId: "todos",
    creatorId: "dev_abcdefgh",
    errors: [],
    createdAtUnixSeconds: 1_786_579_200,
    updatedAtUnixSeconds: 1_786_579_200,
    expiresAtUnixSeconds: 4_102_444_800,
    ...input,
  };
}
function navigation(environmentId: string) {
  return [
    "overview",
    "data",
    "collections",
    "sync",
    "users",
    "policies",
    "functions",
    "observability",
    "backups",
    "connect",
    "settings",
  ].map((id) => ({
    id,
    label: id,
    path: `/projects/${PROJECT_ID}/environments/${environmentId}/${id}`,
    permitted: true,
  }));
}
function backup() {
  return {
    backupId: "backup_verified01",
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    recoveryPointUnixSeconds: 1_786_492_800,
    verifiedAtUnixSeconds: 1_786_579_100,
    retainedUntilUnixSeconds: 4_102_444_800,
    lastRestoreDrillUnixSeconds: null,
    recoveryObjectiveStatus: "within_objective",
  };
}
function restore() {
  return {
    requestId: "drr_safe01",
    backupId: "backup_verified01",
    target: { projectId: PROJECT_ID, environmentId: "env_recovery_safe01" },
    state: "requested",
    accessible: false,
    overwritePermitted: false,
    promotionPermitted: false,
    requestedAtUnixSeconds: 1_786_579_200,
    updatedAtUnixSeconds: 1_786_579_200,
  };
}
async function serveWorkspaceShell(route: Route, path: string): Promise<boolean> {
  if (route.request().headers().authorization !== "Bearer developer-session-token") {
    await json(route, {}, 401);
    return true;
  }
  if (path === `/v1/projects/${PROJECT_ID}`) {
    await json(route, project());
    return true;
  }
  if (path === `/v1/projects/${PROJECT_ID}/environments`) {
    await json(route, {
      items: [
        environment(ENVIRONMENT_ID, "development"),
        environment(OTHER_ENVIRONMENT_ID, "preview"),
      ],
    });
    return true;
  }
  if (path.endsWith("/workspace/navigation")) {
    await json(
      route,
      navigation(path.includes(OTHER_ENVIRONMENT_ID) ? OTHER_ENVIRONMENT_ID : ENVIRONMENT_ID),
    );
    return true;
  }
  return false;
}
async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

function automaticGrant() {
  return {
    grantId: "xgr_0123456789abcdef0123456789abcdef",
    capability: CAPABILITY,
    mode: "administrative",
    operations: ["get", "browse", "query", "plan", "history", "simulate", "mutate"],
    applicationUserId: null,
    issuedAtUnixSeconds: Math.floor(Date.now() / 1000),
    expiresAtUnixSeconds: Math.floor(Date.now() / 1000) + 300,
    authorizationEpoch: 1,
  };
}
function documentPage() {
  return { items: [document()], nextCursor: null, snapshot: "snapshot-safe", exhausted: true };
}
