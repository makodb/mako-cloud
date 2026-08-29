import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const LATER = "2026-08-07T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const WEBHOOKS_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/webhooks`;
const WEBHOOKS_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/webhooks`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const ACTIVE = "whk_orders000001";
const PAUSED = "whk_paused000001";
const DISABLED = "whk_disabled0001";
const CREATED_SECRET = "whs_created_secret_shown_exactly_once";
const ROTATED_SECRET = "whs_rotated_secret_shown_exactly_once";

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

// The Webhooks screen is the console's view of an environment's endpoints and
// their deliveries. The API is mocked at the wire: every assertion is about
// what the screen shows for a response, or what it sends for an action — above
// all that a signing secret is shown once and is nowhere on the page after.
test("the endpoint list shows every state with its reason and Webhooks is a real destination", async ({
  page,
}) => {
  const api = new WebhookApiHarness();
  await api.install(page);

  await page.goto(WEBHOOKS_URL);
  await expect(page.getByRole("heading", { name: "Webhooks", exact: true })).toBeVisible();

  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  const link = destinations.getByRole("link", { name: "Webhooks", exact: true });
  await expect(link).toBeVisible();
  await expect(link).toHaveAttribute("href", WEBHOOKS_URL);
  await expect(link).toHaveAttribute("aria-current", "page");
  await expect(
    destinations.locator(".destination-unavailable", { hasText: "Webhooks" }),
  ).toHaveCount(0);

  const active = page.locator(`tr[data-webhook-id="${ACTIVE}"]`);
  await expect(active).toContainText("https://hooks.example.test/orders");
  await expect(active).toContainText("orders to the warehouse");
  await expect(active).toContainText("orders insert, update");
  await expect(active).toContainText("shipments delete");
  await expect(active.locator(".webhook-state")).toHaveText("active");
  await expect(active.locator("td.numeric")).toHaveText("0");

  const paused = page.locator(`tr[data-webhook-id="${PAUSED}"]`);
  await expect(paused.locator(".webhook-state")).toHaveText("paused");
  await expect(paused.locator(".paused-reason")).toHaveText(
    "12 consecutive failures since 2026-08-07T11:00:00Z",
  );
  await expect(paused.locator("td.numeric")).toHaveText("12");

  const disabled = page.locator(`tr[data-webhook-id="${DISABLED}"]`);
  await expect(disabled.locator(".webhook-state")).toHaveText("disabled");
  await expect(disabled.locator(".paused-reason")).toHaveCount(0);

  // Listing never carries a secret, and the page never shows one.
  expect(await page.content()).not.toContain("whs_");
  expect(api.unhandled).toEqual([]);
});

test("registering an endpoint posts its subscriptions and shows the signing secret exactly once", async ({
  page,
}) => {
  const api = new WebhookApiHarness();
  await api.install(page);

  await page.goto(WEBHOOKS_URL);
  await expect(page.locator(`tr[data-webhook-id="${ACTIVE}"]`)).toBeVisible();

  const form = page.getByRole("region", { name: "Register endpoint" });
  await form.getByLabel("Endpoint URL").fill("https://hooks.example.test/invoices");
  await form.getByLabel("Description").fill("invoices to accounting");
  // The first row subscribes to every event by default; narrow it and add a second.
  await form.getByLabel("Subscription 1 collection").fill("invoices");
  await form.getByLabel("Subscription 1 delete").uncheck();
  await form.getByRole("button", { name: "Add collection" }).click();
  await form.getByLabel("Subscription 2 collection").fill("payments");
  await form.getByLabel("Subscription 2 insert").uncheck();
  await form.getByLabel("Subscription 2 update").uncheck();
  await expect(form.getByLabel("Subscription 2 delete")).toBeChecked();
  await form.getByRole("button", { name: "Register endpoint" }).click();

  await expect(page.getByRole("status")).toHaveText("Webhook whk_invoices00001 registered.");
  const creates = api.requests.filter(
    (request) => request.method === "POST" && request.path === WEBHOOKS_PATH,
  );
  expect(creates).toHaveLength(1);
  expect(creates[0]?.body).toEqual({
    url: "https://hooks.example.test/invoices",
    description: "invoices to accounting",
    subscriptions: [
      { collectionId: "invoices", events: ["insert", "update"] },
      { collectionId: "payments", events: ["delete"] },
    ],
    enabled: true,
  });
  expect(creates[0]?.headers["idempotency-key"]).toMatch(UUID);

  // The secret is shown once, hidden until revealed, and gone once dismissed.
  const panel = page.getByRole("complementary", {
    name: "Copy this signing secret for webhook whk_invoices00001 now. It will not be shown again.",
  });
  await expect(panel).toBeVisible();
  expect(await page.content()).not.toContain(CREATED_SECRET);
  await panel.getByRole("button", { name: "Reveal value" }).click();
  await expect(panel.getByText(CREATED_SECRET)).toBeVisible();
  await panel.getByRole("button", { name: "I have stored it securely" }).click();
  await expect(panel).toHaveCount(0);
  expect(await page.content()).not.toContain(CREATED_SECRET);

  // The new endpoint is listed from the API, which never returns the secret,
  // and the form is ready for the next one.
  const row = page.locator('tr[data-webhook-id="whk_invoices00001"]');
  await expect(row).toBeVisible();
  await expect(row).toContainText("invoices insert, update");
  await expect(row).toContainText("payments delete");
  await expect(form.getByLabel("Endpoint URL")).toHaveValue("");
  await expect(form.getByLabel("Subscription 1 collection")).toHaveValue("");
  await expect(form.getByLabel("Subscription 2 collection")).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

test("a malformed or refused registration is shown inline and creates nothing", async ({
  page,
}) => {
  const api = new WebhookApiHarness();
  await api.install(page);

  await page.goto(WEBHOOKS_URL);
  const form = page.getByRole("region", { name: "Register endpoint" });
  await expect(page.locator(`tr[data-webhook-id="${ACTIVE}"]`)).toBeVisible();

  // A subscription without a collection never leaves the browser.
  await form.getByLabel("Endpoint URL").fill("https://hooks.example.test/x");
  await form.getByRole("button", { name: "Register endpoint" }).click();
  await expect(page.getByRole("alert")).toContainText("Subscription 1 needs a collection id");
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // Nor does a collection with every event unticked.
  await form.getByLabel("Subscription 1 collection").fill("orders");
  for (const event of ["insert", "update", "delete"]) {
    await form.getByLabel(`Subscription 1 ${event}`).uncheck();
  }
  await form.getByRole("button", { name: "Register endpoint" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Subscription 1 (orders) needs at least one event.",
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // A URL the API refuses is shown with its message, verbatim, and no secret.
  api.createRefusal = "url must be an absolute https URL outside loopback";
  await form.getByLabel("Subscription 1 insert").check();
  await form.getByRole("button", { name: "Register endpoint" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "url must be an absolute https URL outside loopback",
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(1);
  await expect(page.getByRole("complementary", { name: /signing secret/u })).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveCount(0);
  expect(api.endpoints.map((endpoint) => endpoint.id)).toEqual([ACTIVE, PAUSED, DISABLED]);
  expect(api.unhandled).toEqual([]);
});

test("a paused endpoint shows why and Resume sends the action", async ({ page }) => {
  const api = new WebhookApiHarness();
  await api.install(page);

  await page.goto(WEBHOOKS_URL);
  await page.getByRole("button", { name: `Open webhook ${PAUSED}` }).click();
  await expect(page).toHaveURL(`${WEBHOOKS_URL}/${PAUSED}`);
  await expect(page.getByRole("heading", { name: PAUSED, exact: true })).toBeVisible();

  const details = page.getByRole("region", { name: "Details" });
  await expect(details.locator('dd[data-field="state"] .webhook-state')).toHaveText("paused");
  await expect(details.locator('dd[data-field="state"] .paused-reason')).toHaveText(
    "12 consecutive failures since 2026-08-07T11:00:00Z",
  );
  await expect(details.locator('dd[data-field="consecutiveFailures"]')).toHaveText("12");
  await expect(details.locator('dd[data-field="pausedAt"]')).toBeVisible();
  // A failed delivery cannot be redelivered while the endpoint is paused.
  const deliveries = page.getByRole("region", { name: "Deliveries" });
  await expect(
    deliveries.getByRole("button", { name: "Redeliver whd_paused0000001" }),
  ).toBeDisabled();

  await details.getByRole("button", { name: "Resume" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Webhook resumed; deliveries still pending are retried from where they stopped.",
  );
  await expect(details.locator('dd[data-field="state"] .webhook-state')).toHaveText("active");
  await expect(details.locator('dd[data-field="state"] .paused-reason')).toHaveCount(0);
  await expect(details.locator('dd[data-field="consecutiveFailures"]')).toHaveText("0");
  await expect(details.getByRole("button", { name: "Resume" })).toHaveCount(0);
  await expect(
    deliveries.getByRole("button", { name: "Redeliver whd_paused0000001" }),
  ).toBeEnabled();

  const resumes = api.requests.filter((request) => request.path.endsWith("/actions/resume"));
  expect(resumes.map((request) => [request.method, request.path])).toEqual([
    ["POST", `${WEBHOOKS_PATH}/${PAUSED}/actions/resume`],
  ]);
  expect(resumes[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(resumes[0]?.body).toBeNull();
  expect(api.unhandled).toEqual([]);
});

test("the delivery log pages newest first, filters by state, and redelivers a failed one", async ({
  page,
}) => {
  const api = new WebhookApiHarness();
  await api.install(page);

  await page.goto(`${WEBHOOKS_URL}/${ACTIVE}`);
  const deliveries = page.getByRole("region", { name: "Deliveries" });
  await expect(deliveries.locator("tbody tr")).toHaveCount(2);
  const newest = deliveries.locator("tbody tr").first();
  await expect(newest).toHaveAttribute("data-delivery-id", "whd_orders0000005");
  await expect(newest).toContainText("update");
  await expect(newest).toContainText("orders");
  await expect(newest).toContainText("ord_5");
  await expect(newest.locator(".delivery-state")).toHaveText("pending");
  const failed = deliveries.locator('tr[data-delivery-id="whd_orders0000004"]');
  await expect(failed.locator(".delivery-state")).toHaveText("failed");
  await expect(failed).toContainText("HTTP 503");
  await expect(failed).toContainText("status_503");
  await expect(failed.locator("td.numeric")).toHaveText("5");
  // Only a failed delivery offers redelivery.
  await expect(newest.getByRole("button", { name: /Redeliver/u })).toHaveCount(0);

  await deliveries.getByRole("button", { name: "Load more" }).click();
  await expect(deliveries.locator("tbody tr")).toHaveCount(4);
  await expect(deliveries.locator('tr[data-delivery-id="whd_orders0000003"]')).toContainText(
    "HTTP 200",
  );
  await deliveries.getByRole("button", { name: "Load more" }).click();
  await expect(deliveries.locator("tbody tr")).toHaveCount(5);
  await expect(deliveries.getByRole("button", { name: "Load more" })).toBeDisabled();
  await expect(deliveries.getByText("Every retained delivery is listed.")).toBeVisible();

  // The state filter narrows the listing at the API.
  await deliveries.getByLabel("State").selectOption("failed");
  await expect(deliveries.locator("tbody tr")).toHaveCount(2);
  await expect(deliveries.locator("tbody tr").first()).toHaveAttribute(
    "data-delivery-id",
    "whd_orders0000004",
  );

  // The dev server mounts under StrictMode, which runs the listing effect
  // twice on mount; the distinct requests are what the screen asked for.
  const listings = distinct(
    api.requests
      .filter((request) => request.method === "GET" && request.path.endsWith("/deliveries"))
      .map((request) => ({
        state: request.query.get("state"),
        cursor: request.query.get("cursor"),
        limit: request.query.get("limit"),
      })),
  );
  expect(listings).toEqual([
    { state: null, cursor: null, limit: "50" },
    { state: null, cursor: "after-2", limit: "50" },
    { state: null, cursor: "after-4", limit: "50" },
    { state: "failed", cursor: null, limit: "50" },
  ]);

  // Redelivering posts to the delivery's action and logs the new delivery on top.
  await deliveries.getByRole("button", { name: "Redeliver whd_orders0000004" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Delivery whd_redeliver00001 queued as a redelivery of whd_orders0000004.",
  );
  const queued = deliveries.locator("tbody tr").first();
  await expect(queued).toHaveAttribute("data-delivery-id", "whd_redeliver00001");
  await expect(queued).toContainText("redelivery of whd_orders0000004");
  await expect(queued.locator(".delivery-state")).toHaveText("pending");
  await expect(failed.locator(".delivery-state")).toHaveText("failed");
  const redeliveries = api.requests.filter((request) =>
    request.path.endsWith("/actions/redeliver"),
  );
  expect(redeliveries.map((request) => [request.method, request.path])).toEqual([
    ["POST", `${WEBHOOKS_PATH}/${ACTIVE}/deliveries/whd_orders0000004/actions/redeliver`],
  ]);
  expect(redeliveries[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(api.unhandled).toEqual([]);
});

test("settings patch the endpoint; rotating the secret is confirmed and shows the new one once; delete returns to the list", async ({
  page,
}) => {
  const api = new WebhookApiHarness();
  await api.install(page);
  const dialogs: string[] = [];
  const decisions: boolean[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void (decisions.shift() === true ? dialog.accept() : dialog.dismiss());
  });

  await page.goto(`${WEBHOOKS_URL}/${ACTIVE}`);
  const details = page.getByRole("region", { name: "Details" });
  const settings = page.getByRole("region", { name: "Settings" });
  await expect(settings.getByLabel("Endpoint URL")).toHaveValue(
    "https://hooks.example.test/orders",
  );
  await expect(settings.getByLabel("Subscription 1 collection")).toHaveValue("orders");
  await expect(settings.getByLabel("Subscription 1 delete")).not.toBeChecked();
  await expect(settings.getByLabel("Subscription 2 collection")).toHaveValue("shipments");

  // Saving sends the settings as edited, with an idempotency key.
  await settings.getByLabel("Endpoint URL").fill("https://hooks.example.test/orders-v2");
  await settings.getByLabel("Subscription 1 delete").check();
  await settings.getByRole("button", { name: "Remove subscription 2" }).click();
  await settings.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toHaveText("Webhook settings saved.");
  const patches = api.requests.filter((request) => request.method === "PATCH");
  expect(patches.map((request) => request.path)).toEqual([`${WEBHOOKS_PATH}/${ACTIVE}`]);
  expect(patches[0]?.body).toEqual({
    url: "https://hooks.example.test/orders-v2",
    description: "orders to the warehouse",
    subscriptions: [{ collectionId: "orders", events: ["insert", "update", "delete"] }],
  });
  expect(patches[0]?.headers["idempotency-key"]).toMatch(UUID);

  // Disabling is its own patch; the state follows.
  await details.getByRole("button", { name: "Disable" }).click();
  await expect(details.locator('dd[data-field="state"] .webhook-state')).toHaveText("disabled");
  await expect(details.locator('dd[data-field="enabled"]')).toHaveText("no");
  expect(api.requests.filter((request) => request.method === "PATCH")[1]?.body).toEqual({
    enabled: false,
  });
  await details.getByRole("button", { name: "Enable" }).click();
  await expect(details.locator('dd[data-field="state"] .webhook-state')).toHaveText("active");

  // Dismissing the rotation confirmation sends nothing.
  decisions.push(false);
  await details.getByRole("button", { name: "Rotate secret" }).click();
  expect(dialogs).toHaveLength(1);
  expect(dialogs[0]).toContain(`Rotate the signing secret of webhook ${ACTIVE}?`);
  expect(dialogs[0]).toContain("This action will be audited.");
  expect(api.requests.filter((request) => request.path.endsWith("/rotate-secret"))).toHaveLength(0);

  // Confirming rotates, bumps the version, and shows the new secret once.
  decisions.push(true);
  await details.getByRole("button", { name: "Rotate secret" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Signing secret rotated; version 2 signs from now.",
  );
  await expect(details.locator('dd[data-field="secretVersion"]')).toHaveText("2");
  const rotations = api.requests.filter((request) => request.path.endsWith("/rotate-secret"));
  expect(rotations.map((request) => [request.method, request.path])).toEqual([
    ["POST", `${WEBHOOKS_PATH}/${ACTIVE}/actions/rotate-secret`],
  ]);
  expect(rotations[0]?.headers["idempotency-key"]).toMatch(UUID);
  const panel = page.getByRole("complementary", {
    name: `Copy this signing secret for webhook ${ACTIVE} now. It will not be shown again.`,
  });
  await expect(panel).toBeVisible();
  await panel.getByRole("button", { name: "Reveal value" }).click();
  await expect(panel.getByText(ROTATED_SECRET)).toBeVisible();
  await panel.getByRole("button", { name: "I have stored it securely" }).click();
  await expect(panel).toHaveCount(0);
  expect(await page.content()).not.toContain(ROTATED_SECRET);

  // Deleting asks first, then removes the endpoint and returns to the list.
  decisions.push(true);
  await details.getByRole("button", { name: "Delete" }).click();
  expect(dialogs[2]).toContain(`Delete webhook ${ACTIVE}?`);
  await expect(page).toHaveURL(WEBHOOKS_URL);
  await expect(page.getByRole("heading", { name: "Webhooks", exact: true })).toBeVisible();
  await expect(page.locator(`tr[data-webhook-id="${PAUSED}"]`)).toBeVisible();
  await expect(page.locator(`tr[data-webhook-id="${ACTIVE}"]`)).toHaveCount(0);
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes.map((request) => request.path)).toEqual([`${WEBHOOKS_PATH}/${ACTIVE}`]);
  expect(deletes[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(api.endpoints.map((endpoint) => endpoint.id)).toEqual([PAUSED, DISABLED]);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly query: URLSearchParams;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

type WebhookEvent = "insert" | "update" | "delete";

interface SubscriptionFixture {
  collectionId: string;
  events: WebhookEvent[];
}

interface EndpointFixture {
  id: string;
  url: string;
  description: string;
  subscriptions: SubscriptionFixture[];
  state: "active" | "paused" | "disabled";
  enabled: boolean;
  pausedReason: string | null;
  pausedAt: string | null;
  consecutiveFailures: number;
  secretVersion: number;
  createdAt: string;
  updatedAt: string;
}

interface DeliveryFixture {
  id: string;
  endpointId: string;
  event: WebhookEvent;
  collectionId: string;
  documentId: string;
  revision: string;
  commitPosition: number;
  state: "pending" | "delivered" | "failed";
  attempts: number;
  nextAttemptAt: string | null;
  lastResponseStatus: number | null;
  lastError: string | null;
  redeliveryOf: string | null;
  createdAt: string;
  deliveredAt: string | null;
}

class WebhookApiHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  endpoints: EndpointFixture[] = [
    endpointFixture({
      id: ACTIVE,
      url: "https://hooks.example.test/orders",
      description: "orders to the warehouse",
      subscriptions: [
        { collectionId: "orders", events: ["insert", "update"] },
        { collectionId: "shipments", events: ["delete"] },
      ],
    }),
    endpointFixture({
      id: PAUSED,
      url: "https://down.example.test/hook",
      state: "paused",
      pausedReason: "12 consecutive failures since 2026-08-07T11:00:00Z",
      pausedAt: LATER,
      consecutiveFailures: 12,
      secretVersion: 3,
    }),
    endpointFixture({
      id: DISABLED,
      url: "https://hooks.example.test/later",
      state: "disabled",
      enabled: false,
    }),
  ];
  deliveries: Record<string, DeliveryFixture[]> = {
    // Newest first, as the API lists them.
    [ACTIVE]: [
      deliveryFixture(ACTIVE, 5, "update", "pending", {
        attempts: 1,
        lastResponseStatus: 502,
        lastError: "status_502",
        nextAttemptAt: LATER,
        deliveredAt: null,
      }),
      deliveryFixture(ACTIVE, 4, "insert", "failed", {
        attempts: 5,
        lastResponseStatus: 503,
        lastError: "status_503",
        deliveredAt: null,
      }),
      deliveryFixture(ACTIVE, 3, "insert", "delivered", { attempts: 1, lastResponseStatus: 200 }),
      deliveryFixture(ACTIVE, 2, "update", "failed", {
        attempts: 5,
        lastResponseStatus: null,
        lastError: "timeout",
        deliveredAt: null,
      }),
      deliveryFixture(ACTIVE, 1, "insert", "delivered", { attempts: 2, lastResponseStatus: 200 }),
    ],
    [PAUSED]: [
      {
        ...deliveryFixture(PAUSED, 1, "insert", "failed", {
          attempts: 5,
          lastResponseStatus: null,
          lastError: "connect_refused",
          deliveredAt: null,
        }),
        id: "whd_paused0000001",
      },
    ],
    [DISABLED]: [],
  };
  createRefusal: string | null = null;
  readonly pageSize = 2;

  async install(page: Page) {
    await page.route("**/v1/**", (route) => void this.handle(route));
  }

  async handle(route: Route) {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    if (request.headers().authorization !== "Bearer developer-session-token") {
      await json(route, apiError("unauthenticated", "A valid developer session is required."), 401);
      return;
    }
    const body = request.postData();
    this.requests.push({
      method,
      path,
      query: url.searchParams,
      headers: request.headers(),
      body: body === null ? null : (JSON.parse(body) as unknown),
    });

    if (path === `/v1/projects/${PROJECT_ID}` && method === "GET") {
      await json(route, project());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments` && method === "GET") {
      await json(route, { items: [environment()] });
    } else if (path.endsWith("/workspace/navigation") && method === "GET") {
      await json(route, navigation());
    } else if (path === WEBHOOKS_PATH && method === "GET") {
      await json(route, { items: this.endpoints });
    } else if (path === WEBHOOKS_PATH && method === "POST") {
      if (this.createRefusal !== null) {
        await json(route, apiError("invalid_request", this.createRefusal), 400);
        return;
      }
      const input = request.postDataJSON() as {
        url: string;
        description?: string;
        subscriptions: SubscriptionFixture[];
        enabled?: boolean;
      };
      const enabled = input.enabled ?? true;
      const endpoint = endpointFixture({
        id: `whk_${input.url.split("/").pop()}00001`,
        url: input.url,
        description: input.description ?? "",
        subscriptions: input.subscriptions,
        enabled,
        state: enabled ? "active" : "disabled",
        createdAt: LATER,
        updatedAt: LATER,
      });
      this.endpoints.push(endpoint);
      this.deliveries[endpoint.id] = [];
      await json(route, { endpoint, signingSecret: CREATED_SECRET }, 201);
    } else if (path.startsWith(`${WEBHOOKS_PATH}/`)) {
      await this.handleEndpoint(route, path.slice(WEBHOOKS_PATH.length + 1), method, url);
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    }
  }

  private async handleEndpoint(route: Route, rest: string, method: string, url: URL) {
    const [webhookId, ...tail] = rest.split("/");
    const endpoint = this.endpoints.find((item) => item.id === webhookId);
    if (webhookId === undefined || endpoint === undefined) {
      await json(route, apiError("not_found", "webhook endpoint not found"), 404);
      return;
    }
    const log = this.deliveries[endpoint.id] ?? [];
    const idempotent = route.request().headers()["idempotency-key"] !== undefined;
    if (tail.length === 0 && method === "GET") {
      await json(route, endpoint);
    } else if (tail.length === 0 && method === "PATCH" && idempotent) {
      const patch = route.request().postDataJSON() as Partial<EndpointFixture>;
      Object.assign(endpoint, patch, { updatedAt: LATER });
      if (patch.enabled !== undefined) {
        endpoint.state = patch.enabled ? "active" : "disabled";
      }
      await json(route, endpoint);
    } else if (tail.length === 0 && method === "DELETE" && idempotent) {
      this.endpoints = this.endpoints.filter((item) => item.id !== endpoint.id);
      delete this.deliveries[endpoint.id];
      await route.fulfill({ status: 204 });
    } else if (tail.join("/") === "actions/rotate-secret" && method === "POST" && idempotent) {
      endpoint.secretVersion += 1;
      endpoint.updatedAt = LATER;
      await json(route, { endpoint, signingSecret: ROTATED_SECRET });
    } else if (tail.join("/") === "actions/resume" && method === "POST" && idempotent) {
      if (endpoint.state !== "paused") {
        await json(route, apiError("conflict", "endpoint is not paused"), 409);
        return;
      }
      Object.assign(endpoint, {
        state: "active",
        pausedReason: null,
        pausedAt: null,
        consecutiveFailures: 0,
        updatedAt: LATER,
      });
      await json(route, endpoint);
    } else if (tail[0] === "deliveries" && tail.length === 1 && method === "GET") {
      const state = url.searchParams.get("state");
      const cursor = url.searchParams.get("cursor");
      const offset = cursor === null ? 0 : Number(cursor.replace("after-", ""));
      const matching = log.filter((delivery) => state === null || delivery.state === state);
      const items = matching.slice(offset, offset + this.pageSize);
      const end = offset + items.length;
      await json(route, { items, nextCursor: end < matching.length ? `after-${end}` : null });
    } else if (
      tail[0] === "deliveries" &&
      tail[2] === "actions" &&
      tail[3] === "redeliver" &&
      method === "POST" &&
      idempotent
    ) {
      const original = log.find((delivery) => delivery.id === tail[1]);
      if (original === undefined) {
        await json(route, apiError("not_found", "delivery not found"), 404);
        return;
      }
      if (endpoint.state !== "active") {
        await json(route, apiError("conflict", "endpoint is paused or disabled"), 409);
        return;
      }
      const queued: DeliveryFixture = {
        ...original,
        id: "whd_redeliver00001",
        state: "pending",
        attempts: 0,
        nextAttemptAt: LATER,
        lastResponseStatus: null,
        lastError: null,
        redeliveryOf: original.id,
        createdAt: LATER,
        deliveredAt: null,
      };
      log.unshift(queued);
      await json(route, queued, 202);
    } else {
      this.unhandled.push(`${method} ${url.pathname}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${url.pathname}`), 404);
    }
  }
}

function endpointFixture(overrides: Partial<EndpointFixture> & { id: string; url: string }) {
  return {
    description: "",
    subscriptions: [{ collectionId: "orders", events: ["insert", "update", "delete"] }],
    state: "active",
    enabled: true,
    pausedReason: null,
    pausedAt: null,
    consecutiveFailures: 0,
    secretVersion: 1,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  } as EndpointFixture;
}

function deliveryFixture(
  endpointId: string,
  sequence: number,
  event: WebhookEvent,
  state: DeliveryFixture["state"],
  overrides: Partial<DeliveryFixture>,
): DeliveryFixture {
  return {
    id: `whd_orders000000${sequence}`,
    endpointId,
    event,
    collectionId: "orders",
    documentId: `ord_${sequence}`,
    revision: `${sequence}-abc`,
    commitPosition: 100 + sequence,
    state,
    attempts: 1,
    nextAttemptAt: null,
    lastResponseStatus: 200,
    lastError: null,
    redeliveryOf: null,
    createdAt: `2026-08-06T12:0${sequence}:00.000Z`,
    deliveredAt: `2026-08-06T12:0${sequence}:01.000Z`,
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

function environment() {
  return {
    id: ENVIRONMENT_ID,
    projectId: PROJECT_ID,
    name: "development",
    state: "active",
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function navigation() {
  return ["overview", "collections", "functions"].map((id) => ({
    id,
    label: id,
    path: `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/${id}`,
    permitted: true,
  }));
}

function distinct<T>(queries: readonly T[]): T[] {
  const seen = new Set<string>();
  return queries.filter((query) => {
    const key = JSON.stringify(query);
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
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
