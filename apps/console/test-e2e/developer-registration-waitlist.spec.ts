import { expect, test, type Route } from "@playwright/test";

const encode = (value: unknown) => Buffer.from(JSON.stringify(value), "utf8").toString("base64url");
// The console accepts a session only from its own issuer, which is its origin;
// playwright.config.ts lets a suite run on another port, so follow it.
const ISSUER = `http://127.0.0.1:${process.env.CONSOLE_TEST_PORT ?? "4174"}/control-identity`;

function developerToken(audience: "mako-management" | "mako-developer-waitlist") {
  const issuedAt = Math.floor(Date.now() / 1_000);
  const status = audience === "mako-management" ? "active" : "waitlisted";
  return `${encode({ alg: "EdDSA", typ: "JWT", kid: "devkid_0123456789abcdef" })}.${encode({
    iss: ISSUER,
    sub: "dev_applicant01",
    aud: [audience],
    email: "applicant@example.test",
    emailVerified: true,
    name: "Beta Applicant",
    sid: "session_abcdefgh",
    developerIdentityId: "dev_applicant01",
    status,
    authorizationEpoch: 2,
    iat: issuedAt,
    exp: issuedAt + 900,
  })}.signature`;
}

test("registration, fragment verification, pending sign-in, and product isolation", async ({
  page,
}) => {
  const requests: string[] = [];
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    requests.push(`${request.method()} ${path}`);
    if (path === "/v1/developer-auth/sessions/refresh") {
      await apiError(route, 401);
    } else if (path === "/v1/developer-auth/registrations") {
      await json(route, { status: "accepted", message: "If eligible, check your email." }, 202);
    } else if (path === "/v1/developer-auth/verifications") {
      await json(route, { status: "waitlisted" });
    } else if (path === "/v1/developer-auth/sessions") {
      const token = developerToken("mako-developer-waitlist");
      const claims = JSON.parse(Buffer.from(token.split(".")[1] ?? "", "base64url").toString());
      await json(route, {
        accessToken: token,
        tokenType: "Bearer",
        audience: "mako-developer-waitlist",
        status: "waitlisted",
        expiresAt: new Date(claims.exp * 1_000).toISOString().replace(".000Z", "Z"),
      });
    } else if (path === "/v1/developer-auth/wait-list-status") {
      expect(request.headers().authorization).toContain("Bearer ");
      await json(route, { developerIdentityId: "dev_applicant01", status: "waitlisted" });
    } else if (path === "/v1/developer-auth/sessions/current" && request.method() === "DELETE") {
      await route.fulfill({ status: 204, body: "" });
    } else if (path === "/v1/developer-auth/password-recovery-requests") {
      await json(route, { status: "accepted", message: "If eligible, check your email." }, 202);
    } else if (path === "/v1/developer-auth/password-recoveries") {
      await json(route, { status: "password_updated" });
    } else {
      await apiError(route, 403);
    }
  });

  await page.goto("/create-account");
  await page.getByLabel("Display name").fill("Beta Applicant");
  await page.getByLabel("Developer email").fill("applicant@example.test");
  await page.getByLabel("Password").fill("a safe beta password");
  await page.getByRole("button", { name: "Create developer account" }).click();
  await expect(page).toHaveURL(/\/check-email$/u);

  await page.goto(`/verify-email#token=${"v".repeat(64)}`);
  await expect(page.getByText(/now waiting for review/u)).toBeVisible();
  await expect(page).toHaveURL(/\/verify-email$/u);
  expect(new URL(page.url()).hash).toBe("");

  await page.goto("/sign-in");
  await page.getByLabel("Developer email").fill("applicant@example.test");
  await page.getByLabel("Password").fill("a safe beta password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/wait-list$/u);
  await expect(page.getByText("Status: pending review")).toBeVisible();
  await expect(page.getByRole("link", { name: "Recover account" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
  expect(requests.some((request) => request.startsWith("GET /v1/teams"))).toBe(false);
  expect(requests.some((request) => request.includes("/v1/projects"))).toBe(false);

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(
    page.getByRole("heading", { name: "Sign in to your developer account" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Forgot password?" }).click();
  await expect(page).toHaveURL(/\/forgot-password$/u);
  await page.getByLabel("Developer email").fill("applicant@example.test");
  await page.getByRole("button", { name: "Send recovery email" }).click();
  await expect(page).toHaveURL(/\/check-recovery-email$/u);
  await page.goto(`/reset-password#token=${"r".repeat(64)}`);
  await page.getByLabel("New password").fill("a newly recovered password");
  await page.getByRole("button", { name: "Update password" }).click();
  await expect(page.getByText(/Existing sessions were revoked/u)).toBeVisible();
  expect(new URL(page.url()).hash).toBe("");
});

test("password-backed operator session reviews and commits a wait-list approval", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const applicant = {
    developerIdentityId: "dev_applicant01",
    email: "applicant@example.test",
    displayName: "Beta Applicant",
    status: "waitlisted",
    operatorEntitlementStatus: "active",
    authorizationEpoch: 2,
    emailVerified: true,
    createdAt: "2026-08-09T12:00:00.000Z",
    updatedAt: "2026-08-09T12:00:00.000Z",
  };
  const rejectedApplicant = {
    ...applicant,
    developerIdentityId: "dev_rejected001",
    email: "rejected@example.test",
    displayName: "Rejected Applicant",
  };
  let operatorSignedIn = false;
  let requireStepUp = true;
  let revokeOperator = false;
  const approvalIdempotencyKeys: string[] = [];
  const approvalBodies: unknown[] = [];
  const operatorSession = {
    operatorId: "opr_betaadmin01",
    developerIdentityId: "dev_applicant01",
    email: "operator@example.test",
    displayName: "Beta Operator",
    developerStatus: "waitlisted",
    permissions: ["waitlist_review"],
    passwordVerifiedAt: new Date().toISOString(),
    expiresAt: new Date(Date.now() + 30 * 60 * 1_000).toISOString(),
  };
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/v1/developer-auth/sessions/refresh") {
      await apiError(route, 401);
      return;
    }
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "GET") {
      if (operatorSignedIn) await json(route, operatorSession);
      else await apiError(route, 401);
      return;
    }
    if (path === "/v1/operator-auth/sessions" && request.method() === "POST") {
      operatorSignedIn = true;
      await json(route, operatorSession);
      return;
    }
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "DELETE") {
      operatorSignedIn = false;
      await route.fulfill({ status: 204, body: "" });
      return;
    }
    if (
      path === "/v1/operator-auth/sessions/current/actions/verify-password" &&
      request.method() === "POST"
    ) {
      operatorSession.passwordVerifiedAt = new Date().toISOString();
      await json(route, operatorSession);
      return;
    }
    expect(request.headers().authorization).toBeUndefined();
    if (path === "/v1/operator/developer-waitlist" && request.method() === "GET") {
      if (revokeOperator) {
        operatorSignedIn = false;
        await apiError(route, 401);
        return;
      }
      await json(route, {
        applicants: [applicant, rejectedApplicant].filter((value) => value.status === "waitlisted"),
        nextCursor: null,
      });
    } else if (
      path === "/v1/operator/developer-waitlist/dev_applicant01" &&
      request.method() === "GET"
    ) {
      await json(route, applicant);
    } else if (
      path === "/v1/operator/developer-waitlist/dev_rejected001" &&
      request.method() === "GET"
    ) {
      await json(route, rejectedApplicant);
    } else if (path.endsWith("/actions/approve") && request.method() === "POST") {
      approvalIdempotencyKeys.push(request.headers()["idempotency-key"] ?? "");
      approvalBodies.push(request.postDataJSON());
      if (requireStepUp) {
        requireStepUp = false;
        await json(
          route,
          {
            apiVersion: "v1",
            error: {
              code: "operator_step_up_required",
              message: "Password verification is required.",
              requestId: "req_stepup01",
              retry: { kind: "never" },
            },
          },
          401,
        );
        return;
      }
      expect(request.headers()["idempotency-key"]).toMatch(/^waitlist-approve-/u);
      applicant.status = "active";
      applicant.authorizationEpoch = 3;
      applicant.updatedAt = "2026-08-09T12:05:00.000Z";
      await json(route, applicant);
    } else if (path.endsWith("/dev_rejected001/actions/reject") && request.method() === "POST") {
      expect(request.headers()["idempotency-key"]).toMatch(/^waitlist-reject-/u);
      rejectedApplicant.status = "rejected";
      rejectedApplicant.authorizationEpoch = 3;
      rejectedApplicant.updatedAt = "2026-08-09T12:06:00.000Z";
      await json(route, rejectedApplicant);
    } else {
      await apiError(route, 404);
    }
  });
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto("/operator");
  await expect(page.getByLabel(/token/iu)).toHaveCount(0);
  await page.getByLabel("Email").fill("operator@example.test");
  await page.getByLabel("Password").fill("operator password");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await expect(page.getByRole("columnheader", { name: "Developer status" })).toBeVisible();
  await expect(page.getByRole("columnheader", { name: "Operator access" })).toBeVisible();
  await expect(page.getByRole("cell", { name: "active" }).first()).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Developer registration wait list" }),
  ).toBeVisible();
  const workspaceSection = page.locator("#operator-main-content > section");
  const waitListPanel = page.locator(
    '#operator-main-content > section > section[aria-labelledby="developer-waitlist-title"]',
  );
  const [workspaceBox, waitListBox] = await Promise.all([
    workspaceSection.boundingBox(),
    waitListPanel.boundingBox(),
  ]);
  expect(workspaceBox).not.toBeNull();
  expect(waitListBox).not.toBeNull();
  expect(Math.abs((workspaceBox?.width ?? 0) - (waitListBox?.width ?? 0))).toBeLessThan(2);
  await page.setViewportSize({ width: 375, height: 812 });
  const [mobileWorkspaceBox, mobileWaitListBox] = await Promise.all([
    workspaceSection.boundingBox(),
    waitListPanel.boundingBox(),
  ]);
  expect(mobileWorkspaceBox).not.toBeNull();
  expect(mobileWaitListBox).not.toBeNull();
  expect(Math.abs((mobileWorkspaceBox?.width ?? 0) - (mobileWaitListBox?.width ?? 0))).toBeLessThan(
    2,
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    ),
  ).toBe(true);
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect(page.getByRole("heading", { name: "Tenant lookup" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Provisioning repair" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Quota override" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Abuse response" })).toHaveCount(0);
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Developer registration wait list" }),
  ).toBeVisible();
  await page
    .getByRole("row", { name: /Beta Applicant/u })
    .getByRole("button", { name: "Review" })
    .click();
  const approvalDetail = page.getByRole("complementary", { name: "Beta Applicant" });
  await expect(
    approvalDetail.getByLabel("Private review reason / case reference (optional)"),
  ).not.toHaveAttribute("required");
  await page.getByLabel("Approve developer").check();
  await page.getByRole("button", { name: "Review and confirm decision" }).click();
  const stepUp = page.getByRole("dialog", { name: "Verify your operator password" });
  await expect(stepUp).toBeVisible();
  await stepUp.getByLabel("Password").fill("operator password");
  await stepUp.getByRole("button", { name: "Verify and continue" }).click();
  await expect(
    page.getByLabel("Beta Applicant").getByText("active", { exact: true }).first(),
  ).toBeVisible();
  expect(approvalIdempotencyKeys).toHaveLength(2);
  expect(approvalIdempotencyKeys[0]).toBe(approvalIdempotencyKeys[1]);
  expect(approvalBodies).toEqual([{}, {}]);
  await page.getByRole("button", { name: "Close detail" }).click();
  await page
    .getByRole("row", { name: /Rejected Applicant/u })
    .getByRole("button", { name: "Review" })
    .click();
  await page
    .getByRole("complementary", { name: "Rejected Applicant" })
    .getByLabel("Private review reason / case reference (optional)")
    .fill("Rejected from beta review");
  await page.getByLabel("Reject developer").check();
  await page.getByRole("button", { name: "Review and confirm decision" }).click();
  await expect(page.getByText("rejected", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in as a platform operator" })).toBeVisible();
  await expect(page.getByLabel(/token/iu)).toHaveCount(0);
  await page.getByLabel("Email").fill("operator@example.test");
  await page.getByLabel("Password").fill("operator password");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await expect(
    page.getByRole("heading", { name: "Developer registration wait list" }),
  ).toBeVisible();
  revokeOperator = true;
  await page.getByRole("button", { name: "Refresh" }).click();
  await expect(page.getByRole("heading", { name: "Sign in as a platform operator" })).toBeVisible();
});

test("operator batch approval clears hidden selection and reports partial results", async ({
  page,
}) => {
  const applicants = [
    {
      developerIdentityId: "dev_operator01",
      email: "operator@example.test",
      displayName: "Beta Operator",
      status: "waitlisted",
      operatorEntitlementStatus: "active",
      authorizationEpoch: 2,
      emailVerified: true,
      createdAt: "2026-08-09T12:00:00.000Z",
      updatedAt: "2026-08-09T12:00:00.000Z",
    },
    {
      developerIdentityId: "dev_batch00002",
      email: "second@example.test",
      displayName: "Second Applicant",
      status: "waitlisted",
      operatorEntitlementStatus: "none",
      authorizationEpoch: 2,
      emailVerified: true,
      createdAt: "2026-08-09T12:01:00.000Z",
      updatedAt: "2026-08-09T12:01:00.000Z",
    },
    {
      developerIdentityId: "dev_batchstale",
      email: "stale@example.test",
      displayName: "Stale Applicant",
      status: "waitlisted",
      operatorEntitlementStatus: "none",
      authorizationEpoch: 2,
      emailVerified: true,
      createdAt: "2026-08-09T12:02:00.000Z",
      updatedAt: "2026-08-09T12:02:00.000Z",
    },
  ];
  const approvalRequests: Array<{ path: string; key: string; body: unknown }> = [];
  const operatorSession = {
    operatorId: "opr_betaadmin01",
    developerIdentityId: "dev_operator01",
    email: "operator@example.test",
    displayName: "Beta Operator",
    developerStatus: "waitlisted",
    permissions: ["waitlist_review"],
    passwordVerifiedAt: new Date().toISOString(),
    expiresAt: new Date(Date.now() + 30 * 60 * 1_000).toISOString(),
  };

  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/v1/developer-auth/sessions/refresh") {
      await apiError(route, 401);
      return;
    }
    if (path === "/v1/operator-auth/sessions/current" && request.method() === "GET") {
      await json(route, operatorSession);
      return;
    }
    if (path === "/v1/operator/developer-waitlist" && request.method() === "GET") {
      await json(route, {
        applicants: applicants.filter((applicant) => applicant.status === "waitlisted"),
        nextCursor: null,
      });
      return;
    }
    if (path.endsWith("/actions/approve") && request.method() === "POST") {
      approvalRequests.push({
        path,
        key: request.headers()["idempotency-key"] ?? "",
        body: request.postDataJSON(),
      });
      const applicant = applicants.find((value) => path.includes(value.developerIdentityId));
      expect(applicant).toBeDefined();
      if (applicant?.developerIdentityId === "dev_batchstale") {
        applicant.status = "active";
        applicant.authorizationEpoch = 3;
        await json(
          route,
          {
            apiVersion: "v1",
            error: {
              code: "conflict",
              message: "The wait-list state changed.",
              requestId: "req_stale_batch",
              retry: { kind: "never" },
            },
          },
          409,
        );
        return;
      }
      if (applicant !== undefined) {
        applicant.status = "active";
        applicant.authorizationEpoch = 3;
        await json(route, applicant);
        return;
      }
    }
    await apiError(route, 404);
  });
  page.on("dialog", (dialog) => void dialog.accept());

  await page.goto("/operator");
  await expect(
    page.getByRole("heading", { name: "Developer registration wait list" }),
  ).toBeVisible();
  await page.getByLabel("Select operator@example.test for batch approval").check();
  await expect(page.getByRole("button", { name: "Approve selected (1)" })).toBeEnabled();
  await page.getByLabel("Filter this page by applicant, email, or developer ID").fill("Second");
  await expect(page.getByRole("button", { name: "Approve selected (0)" })).toBeDisabled();
  await page.getByLabel("Filter this page by applicant, email, or developer ID").fill("");

  await page.getByLabel("Select all visible applicants for batch approval").check();
  await expect(page.getByRole("button", { name: "Approve selected (3)" })).toBeEnabled();
  await page.getByRole("button", { name: "Approve selected (3)" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Batch approval finished: 2 committed, 1 failed.",
  );
  expect(approvalRequests).toHaveLength(3);
  expect(new Set(approvalRequests.map((request) => request.key)).size).toBe(3);
  expect(
    approvalRequests.every((request) =>
      /^waitlist-batch-approve-[0-9a-f-]+-[1-3]$/u.test(request.key),
    ),
  ).toBe(true);
  expect(approvalRequests.map((request) => request.body)).toEqual([{}, {}, {}]);
  await expect(
    page.getByText("No matching wait-listed applicants are on this page."),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Developer registration wait list" }),
  ).toBeVisible();
});

test("operator sign-in failures and throttling stay generic", async ({ page }) => {
  let attempts = 0;
  await page.route("**/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/v1/developer-auth/sessions/refresh") {
      await apiError(route, 401);
      return;
    }
    if (path === "/v1/operator-auth/sessions/current") {
      await apiError(route, 401);
      return;
    }
    if (path === "/v1/operator-auth/sessions" && request.method() === "POST") {
      attempts += 1;
      await json(
        route,
        {
          apiVersion: "v1",
          error: {
            code: attempts === 1 ? "unauthenticated" : "rate_limited",
            message: attempts === 1 ? "The email or password was not accepted." : "Try later.",
            requestId: `req_failure${attempts}`,
            retry: { kind: "never" },
          },
        },
        attempts === 1 ? 401 : 429,
      );
      return;
    }
    await apiError(route, 404);
  });

  await page.goto("/operator");
  await page.getByLabel("Email").fill("operator@example.test");
  await page.getByLabel("Password").fill("wrong password");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await expect(page.getByRole("alert")).toHaveText("The email or password was not accepted.");
  await expect(page.getByLabel("Password")).toHaveValue("");

  await page.getByLabel("Password").fill("wrong password again");
  await page.getByRole("button", { name: "Open operator console" }).click();
  await expect(page.getByRole("alert")).toHaveText("Too many attempts. Try again later.");
  await expect(page.getByLabel("Password")).toHaveValue("");
});

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

async function apiError(route: Route, status: number) {
  await json(
    route,
    {
      error: {
        code: status === 401 ? "unauthenticated" : "forbidden",
        message: "Request denied.",
        requestId: "req_console01",
        retry: { retryable: false },
      },
    },
    status,
  );
}
