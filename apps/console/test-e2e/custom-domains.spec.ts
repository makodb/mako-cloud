import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const LATER = "2026-08-07T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const DEVELOPMENT = "env_abcdefgh";
const PRODUCTION = "env_prodprod";
const DOMAINS_PATH = `/v1/projects/${PROJECT_ID}/domains`;
const DOMAINS_URL = `/projects/${PROJECT_ID}/domains`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const PENDING = "dom_api000000001";
const VERIFIED = "dom_app000000001";
const FAILED = "dom_old000000001";
const RECORD_VALUE = "mako-verify=0123456789abcdef0123456789abcdef";

declare global {
  interface Window {
    __MAKO_CONSOLE__?: {
      managementEndpoint: string;
      developerAuth: {
        loadSession(): Promise<unknown>;
        beginSignIn(): Promise<void>;
        signOut(): Promise<void>;
        subscribe(): () => void;
      };
      developerWorkspaceEnabled?: boolean;
      developerExplorerAdminEnabled?: boolean;
      developerDataJobsEnabled?: boolean;
      developerSyncDetailsEnabled?: boolean;
      developerRestoreEnabled?: boolean;
    };
  }
}

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
    };
  });
});

// Custom domains are project-level: the Domains section of the project page
// lists every hostname with the environment it serves and its verification
// state, hands out the DNS record that proves control, checks it on request,
// and removes a domain after confirmation. The API is mocked at the wire;
// every assertion is about what the page shows for a response or sends for
// an action.
test("the Domains section lists every state with its reason and is a project destination", async ({
  page,
}) => {
  const api = new DomainApiHarness();
  await api.install(page);

  await page.goto(DOMAINS_URL);
  await expect(page.getByRole("heading", { name: "Custom domains", exact: true })).toBeVisible();
  const destinations = page.getByRole("navigation", { name: "Project destinations" });
  const link = destinations.getByRole("link", { name: "Domains", exact: true });
  await expect(link).toHaveAttribute("href", DOMAINS_URL);
  await expect(link).toHaveAttribute("aria-current", "page");

  const pending = page.locator(`tr[data-domain-id="${PENDING}"]`);
  await expect(pending).toContainText("api.example.com");
  await expect(pending).toContainText("development");
  await expect(pending.locator(".status-badge")).toHaveText("Pending");
  await expect(pending.locator(".domain-error")).toHaveCount(0);
  await expect(pending).toContainText("never");
  await expect(pending).toContainText("not yet");

  const verified = page.locator(`tr[data-domain-id="${VERIFIED}"]`);
  await expect(verified).toContainText("app.example.com");
  await expect(verified).toContainText("production");
  await expect(verified.locator(".status-badge")).toHaveText("Verified");
  await expect(verified.locator(".status-badge")).toHaveClass(/success/u);
  await expect(verified.locator(`time[datetime="${NOW}"]`)).toBeVisible();
  await expect(verified.locator(`time[datetime="${LATER}"]`)).toBeVisible();

  // A domain whose record went missing says that serving stopped and why.
  const failed = page.locator(`tr[data-domain-id="${FAILED}"]`);
  await expect(failed.locator(".status-badge")).toHaveText("Failed");
  await expect(failed.locator(".status-badge")).toHaveClass(/error/u);
  await expect(failed.locator(".domain-error")).toHaveText(
    "Serving stopped: the TXT record was not found (record_missing).",
  );

  // The listing carries the records, but none is shown until asked for.
  await expect(page.getByRole("region", { name: /DNS record for/u })).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

test("adding a domain posts the hostname and environment with an idempotency key and shows the record to publish", async ({
  page,
}) => {
  const api = new DomainApiHarness();
  await api.install(page);

  await page.goto(DOMAINS_URL);
  await expect(page.locator(`tr[data-domain-id="${PENDING}"]`)).toBeVisible();
  const form = page.getByRole("region", { name: "Add a domain" });
  await expect(form.getByLabel("Environment")).toHaveValue(DEVELOPMENT);

  // A name that is not a hostname never leaves the browser.
  await form.getByLabel("Hostname").fill("not a hostname");
  await form.getByRole("button", { name: "Add domain" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Hostname must be a fully qualified DNS name",
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // The name is normalised, the environment chosen, and the record shown.
  await form.getByLabel("Hostname").fill("New.Example.com.");
  await form.getByLabel("Environment").selectOption(PRODUCTION);
  await form.getByRole("button", { name: "Add domain" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Domain new.example.com added; publish its DNS record, then choose Verify now.",
  );
  const creates = api.requests.filter(
    (request) => request.method === "POST" && request.path === DOMAINS_PATH,
  );
  expect(creates).toHaveLength(1);
  expect(creates[0]?.body).toEqual({ hostname: "new.example.com", environmentId: PRODUCTION });
  expect(creates[0]?.headers["idempotency-key"]).toMatch(UUID);

  const record = page.getByRole("region", { name: "DNS record for new.example.com" });
  await expect(record).toBeVisible();
  await expect(record.locator('[data-field="recordName"]')).toHaveText(
    "_mako-verify.new.example.com",
  );
  await expect(record.locator('[data-field="recordType"]')).toHaveText("TXT");
  await expect(record.locator('[data-field="recordValue"]')).toHaveText(RECORD_VALUE);
  await expect(record.getByRole("button", { name: "Copy name" })).toBeVisible();
  await expect(record.getByRole("button", { name: "Copy value" })).toBeVisible();
  await expect(record).toContainText("then choose Verify now");

  const row = page.locator('tr[data-domain-id="dom_new000000001"]');
  await expect(row).toContainText("new.example.com");
  await expect(row).toContainText("production");
  await expect(row.locator(".status-badge")).toHaveText("Pending");
  await expect(form.getByLabel("Hostname")).toHaveValue("");

  // A hostname another project claims is refused with the API's message.
  api.createRefusal = "hostname taken.example.com is already claimed";
  await form.getByLabel("Hostname").fill("taken.example.com");
  await form.getByRole("button", { name: "Add domain" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "hostname taken.example.com is already claimed",
  );
  expect(api.domains.map((domain) => domain.hostname)).toEqual([
    "api.example.com",
    "app.example.com",
    "old.example.com",
    "new.example.com",
  ]);
  expect(api.unhandled).toEqual([]);
});

test("Show DNS record reveals the TXT record for a listed domain and hides it again", async ({
  page,
}) => {
  const api = new DomainApiHarness();
  await api.install(page);

  await page.goto(DOMAINS_URL);
  const pending = page.locator(`tr[data-domain-id="${PENDING}"]`);
  const toggle = pending.getByRole("button", { name: "Show DNS record" });
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await toggle.click();
  const record = page.getByRole("region", { name: "DNS record for api.example.com" });
  await expect(record).toBeVisible();
  await expect(record.locator('[data-field="recordName"]')).toHaveText(
    "_mako-verify.api.example.com",
  );
  await expect(record.locator('[data-field="recordType"]')).toHaveText("TXT");
  await expect(record.locator('[data-field="recordValue"]')).toHaveText(RECORD_VALUE);
  await expect(pending.getByRole("button", { name: "Hide DNS record" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await pending.getByRole("button", { name: "Hide DNS record" }).click();
  await expect(record).toHaveCount(0);
  // Reading the record needs no request: it came with the listing.
  expect(api.requests.filter((request) => request.path.startsWith(DOMAINS_PATH))).toHaveLength(
    api.requests.filter((request) => request.method === "GET" && request.path === DOMAINS_PATH)
      .length,
  );
  expect(api.unhandled).toEqual([]);
});

test("Verify now posts the check with an idempotency key and re-renders the outcome", async ({
  page,
}) => {
  const api = new DomainApiHarness();
  await api.install(page);

  await page.goto(DOMAINS_URL);
  const pending = page.locator(`tr[data-domain-id="${PENDING}"]`);
  await expect(pending.locator(".status-badge")).toHaveText("Pending");

  // The record is in place: the check verifies the domain.
  await pending.getByRole("button", { name: "Verify now" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Domain api.example.com verified; it is served with a managed certificate.",
  );
  await expect(pending.locator(".status-badge")).toHaveText("Verified");
  await expect(pending.locator(`time[datetime="${LATER}"]`)).toHaveCount(2);
  const verifies = api.requests.filter((request) => request.path.endsWith("/actions/verify"));
  expect(verifies.map((request) => [request.method, request.path])).toEqual([
    ["POST", `${DOMAINS_PATH}/${PENDING}/actions/verify`],
  ]);
  expect(verifies[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(verifies[0]?.body).toBeNull();

  // A record that is still missing leaves the domain unserved and says why.
  api.verifyOutcome = { state: "failed", lastError: "record_mismatch" };
  const failed = page.locator(`tr[data-domain-id="${FAILED}"]`);
  await failed.getByRole("button", { name: "Verify now" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Domain old.example.com failed verification: the TXT record has a different value (record_mismatch); serving stopped until it verifies again.",
  );
  await expect(failed.locator(".status-badge")).toHaveText("Failed");
  await expect(failed.locator(".domain-error")).toHaveText(
    "Serving stopped: the TXT record has a different value (record_mismatch).",
  );
  expect(api.requests.filter((request) => request.path.endsWith("/actions/verify"))).toHaveLength(
    2,
  );
  expect(api.unhandled).toEqual([]);
});

test("removing a domain asks first, then deletes it with an idempotency key", async ({ page }) => {
  const api = new DomainApiHarness();
  await api.install(page);
  const dialogs: string[] = [];
  const decisions: boolean[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void (decisions.shift() === true ? dialog.accept() : dialog.dismiss());
  });

  await page.goto(DOMAINS_URL);
  const failed = page.locator(`tr[data-domain-id="${FAILED}"]`);
  await expect(failed).toBeVisible();

  // Dismissing the confirmation sends nothing.
  decisions.push(false);
  await failed.getByRole("button", { name: "Remove" }).click();
  expect(dialogs).toHaveLength(1);
  expect(dialogs[0]).toContain("Remove domain old.example.com?");
  expect(dialogs[0]).toContain("This action will be audited.");
  expect(api.requests.filter((request) => request.method === "DELETE")).toHaveLength(0);
  await expect(failed).toBeVisible();

  // Confirming removes the domain and the row.
  decisions.push(true);
  await failed.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Domain old.example.com removed; nothing is served on its name any more.",
  );
  await expect(failed).toHaveCount(0);
  await expect(page.locator(`tr[data-domain-id="${PENDING}"]`)).toBeVisible();
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes.map((request) => request.path)).toEqual([`${DOMAINS_PATH}/${FAILED}`]);
  expect(deletes[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(api.domains.map((domain) => domain.id)).toEqual([PENDING, VERIFIED]);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

type DomainState = "pending" | "verified" | "failed";

interface DomainFixture {
  id: string;
  projectId: string;
  environmentId: string;
  hostname: string;
  state: DomainState;
  verification: { recordName: string; recordType: "TXT"; recordValue: string };
  verifiedAt: string | null;
  lastCheckedAt: string | null;
  lastError: string | null;
  createdAt: string;
  updatedAt: string;
}

class DomainApiHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  domains: DomainFixture[] = [
    domainFixture({ id: PENDING, hostname: "api.example.com", environmentId: DEVELOPMENT }),
    domainFixture({
      id: VERIFIED,
      hostname: "app.example.com",
      environmentId: PRODUCTION,
      state: "verified",
      verifiedAt: NOW,
      lastCheckedAt: LATER,
    }),
    domainFixture({
      id: FAILED,
      hostname: "old.example.com",
      environmentId: DEVELOPMENT,
      state: "failed",
      verifiedAt: NOW,
      lastCheckedAt: LATER,
      lastError: "record_missing",
    }),
  ];
  createRefusal: string | null = null;
  /** What the next check finds; by default the record is in place. */
  verifyOutcome: { state: DomainState; lastError: string | null } = {
    state: "verified",
    lastError: null,
  };

  async install(page: Page) {
    await page.route("**/v1/**", (route) => void this.handle(route));
  }

  async handle(route: Route) {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (request.headers().authorization !== "Bearer developer-session-token") {
      await json(route, apiError("unauthenticated", "A valid developer session is required."), 401);
      return;
    }
    const body = request.postData();
    this.requests.push({
      method,
      path,
      headers: request.headers(),
      body: body === null ? null : (JSON.parse(body) as unknown),
    });
    const idempotent = request.headers()["idempotency-key"] !== undefined;

    if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, project());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: environments() });
    } else if (path === `/v1/teams/${TEAM_ID}` && method === "GET") {
      await json(route, team());
    } else if (path === DOMAINS_PATH && method === "GET") {
      await json(route, { items: this.domains });
    } else if (path === DOMAINS_PATH && method === "POST" && idempotent) {
      if (this.createRefusal !== null) {
        await json(route, apiError("conflict", this.createRefusal), 409);
        return;
      }
      const input = request.postDataJSON() as { hostname: string; environmentId: string };
      const domain = domainFixture({
        id: `dom_${input.hostname.split(".")[0]}000000001`,
        hostname: input.hostname,
        environmentId: input.environmentId,
        createdAt: LATER,
        updatedAt: LATER,
      });
      this.domains.push(domain);
      await json(route, domain, 201);
    } else if (path.startsWith(`${DOMAINS_PATH}/`)) {
      await this.handleDomain(route, path.slice(DOMAINS_PATH.length + 1), method, idempotent);
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    }
  }

  private async handleDomain(route: Route, rest: string, method: string, idempotent: boolean) {
    const [domainId, ...tail] = rest.split("/");
    const domain = this.domains.find((item) => item.id === domainId);
    if (domainId === undefined || domain === undefined) {
      await json(route, apiError("not_found", "custom domain not found"), 404);
      return;
    }
    if (tail.length === 0 && method === "GET") {
      await json(route, domain);
    } else if (tail.length === 0 && method === "DELETE" && idempotent) {
      this.domains = this.domains.filter((item) => item.id !== domain.id);
      await route.fulfill({ status: 204 });
    } else if (tail.join("/") === "actions/verify" && method === "POST" && idempotent) {
      const { state, lastError } = this.verifyOutcome;
      Object.assign(domain, {
        state,
        lastError,
        lastCheckedAt: LATER,
        verifiedAt: state === "verified" ? LATER : domain.verifiedAt,
        updatedAt: LATER,
      });
      await json(route, domain);
    } else {
      this.unhandled.push(`${method} ${DOMAINS_PATH}/${rest}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${rest}`), 404);
    }
  }
}

function domainFixture(
  overrides: Partial<DomainFixture> & { id: string; hostname: string; environmentId: string },
): DomainFixture {
  return {
    projectId: PROJECT_ID,
    state: "pending",
    verification: {
      recordName: `_mako-verify.${overrides.hostname}`,
      recordType: "TXT",
      recordValue: RECORD_VALUE,
    },
    verifiedAt: null,
    lastCheckedAt: null,
    lastError: null,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function project() {
  return {
    id: PROJECT_ID,
    teamId: TEAM_ID,
    name: "Mako Test Project",
    region: "us-east-1",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function team() {
  return {
    id: TEAM_ID,
    name: "Acme",
    kind: "team",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function environments() {
  return [
    { id: DEVELOPMENT, name: "development" },
    { id: PRODUCTION, name: "production" },
  ].map((environment) => ({
    ...environment,
    projectId: PROJECT_ID,
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  }));
}

function apiError(code: string, message: string) {
  return {
    apiVersion: "v1",
    error: { code, message, requestId: "req_e2e", retry: { kind: "never" } },
  };
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}
