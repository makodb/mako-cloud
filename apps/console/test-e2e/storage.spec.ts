import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const LATER = "2026-08-07T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const BUCKETS_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/storage-buckets`;
const STORAGE_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/storage`;
const MIB = 1024 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;

const OWNER_RULES = [
  {
    id: "owner-creates",
    effect: "allow",
    operations: ["create"],
    expression: "new.owner_id == identity.user_id",
  },
  {
    id: "owner-changes",
    effect: "allow",
    operations: ["update"],
    expression: "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
  },
  {
    id: "owner-reads-deletes",
    effect: "allow",
    operations: ["read", "delete"],
    expression: "old.owner_id == identity.user_id",
  },
];

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

// The Storage screen is the console's view of the environment's buckets and
// objects. The API is mocked at the wire: every assertion here is about what
// the screen shows for a response, or what it sends for an action.
test("the bucket list shows counts and sizes and Storage is a real destination", async ({
  page,
}) => {
  const api = new StorageApiHarness();
  await api.install(page);

  await page.goto(STORAGE_URL);
  await expect(page.getByRole("heading", { name: "Storage", exact: true })).toBeVisible();

  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  const storageLink = destinations.getByRole("link", { name: "Storage", exact: true });
  await expect(storageLink).toBeVisible();
  await expect(storageLink).toHaveAttribute("href", STORAGE_URL);
  await expect(storageLink).toHaveAttribute("aria-current", "page");
  await expect(
    destinations.locator(".destination-unavailable", { hasText: "Storage" }),
  ).toHaveCount(0);

  // Each bucket names its totals as bytes a developer reads, plus its limits.
  const avatars = page.locator('tr[data-bucket-id="avatars"]');
  await expect(avatars).toContainText("policy");
  await expect(avatars.locator("td").nth(1)).toHaveText("3");
  await expect(avatars.locator("td").nth(2)).toHaveText("1.5 MiB");
  await expect(avatars.locator("td").nth(3)).toHaveText("1.0 MiB");
  await expect(avatars.locator("td").nth(4)).toHaveText("image/*");
  const exports = page.locator('tr[data-bucket-id="exports"]');
  await expect(exports).toContainText("public");
  await expect(exports.locator("td").nth(1)).toHaveText("0");
  await expect(exports.locator("td").nth(2)).toHaveText("0 B");
  await expect(exports.locator("td").nth(3)).toHaveText("16.0 MiB");
  await expect(exports.locator("td").nth(4)).toHaveText("any");
  expect(api.unhandled).toEqual([]);
});

test("creating a bucket posts its limits and rules with an idempotency key", async ({ page }) => {
  const api = new StorageApiHarness();
  await api.install(page);

  await page.goto(STORAGE_URL);
  await expect(page.locator('tr[data-bucket-id="avatars"]')).toBeVisible();

  const form = page.getByRole("region", { name: "Create bucket" });
  await form.getByLabel("Bucket ID").fill("uploads");
  await form.getByLabel("Access").selectOption("public");
  await form.getByLabel("Max object bytes").fill(String(2 * MIB));
  await form.getByLabel("Allowed content types").fill("image/png, application/pdf image/png");
  // The rules textarea starts with the owner-only example and is sent as parsed.
  await expect(form.getByLabel("Rules (JSON)")).toHaveValue(/owner-reads-deletes/u);
  await form.getByRole("button", { name: "Create bucket" }).click();

  await expect(page.getByRole("status")).toHaveText("Bucket uploads created.");
  await expect(page.locator('tr[data-bucket-id="uploads"]')).toBeVisible();
  await expect(page.locator('tr[data-bucket-id="uploads"]').locator("td").nth(3)).toHaveText(
    "2.0 MiB",
  );

  const creates = api.requests.filter(
    (request) => request.method === "POST" && request.path === BUCKETS_PATH,
  );
  expect(creates).toHaveLength(1);
  expect(creates[0]?.body).toEqual({
    id: "uploads",
    access: "public",
    maxObjectBytes: 2 * MIB,
    allowedContentTypes: ["image/png", "application/pdf"],
    rules: OWNER_RULES,
  });
  expect(creates[0]?.headers["idempotency-key"]).toMatch(UUID);
  // The form is ready for the next bucket.
  await expect(form.getByLabel("Bucket ID")).toHaveValue("");
  expect(api.unhandled).toEqual([]);
});

test("a refused or malformed policy is shown inline and creates nothing", async ({ page }) => {
  const api = new StorageApiHarness();
  await api.install(page);

  await page.goto(STORAGE_URL);
  const form = page.getByRole("region", { name: "Create bucket" });
  await expect(page.locator('tr[data-bucket-id="avatars"]')).toBeVisible();

  // Malformed JSON never leaves the browser.
  await form.getByLabel("Bucket ID").fill("broken");
  await form.getByLabel("Rules (JSON)").fill("[ not json");
  await form.getByRole("button", { name: "Create bucket" }).click();
  await expect(page.getByRole("alert")).toContainText("Rules must be valid JSON.");
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(0);

  // A rule the API cannot compile is refused with its message, verbatim.
  api.createRefusal =
    "bucket policy is invalid: rule owner-creates: unknown field `new.ownerid` in expression";
  await form.getByLabel("Rules (JSON)").fill(
    JSON.stringify([
      {
        id: "owner-creates",
        effect: "allow",
        operations: ["create"],
        expression: "new.ownerid == identity.user_id",
      },
    ]),
  );
  await form.getByRole("button", { name: "Create bucket" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "bucket policy is invalid: rule owner-creates: unknown field `new.ownerid` in expression",
  );
  expect(api.requests.filter((request) => request.method === "POST")).toHaveLength(1);
  expect(api.buckets.map((bucket) => bucket.id)).toEqual(["avatars", "exports"]);
  await expect(page.locator('tr[data-bucket-id="broken"]')).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveCount(0);
  expect(api.unhandled).toEqual([]);
});

test("a bucket lists its objects by prefix, pages with the cursor, and deletes one", async ({
  page,
}) => {
  const api = new StorageApiHarness();
  await api.install(page);
  const dialogs: string[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void dialog.accept();
  });

  await page.goto(STORAGE_URL);
  await page.getByRole("button", { name: "Open bucket avatars" }).click();
  await expect(page).toHaveURL(`${STORAGE_URL}/avatars`);
  await expect(page.getByRole("heading", { name: "avatars", exact: true })).toBeVisible();

  // Totals and settings come from the bucket itself.
  const totals = page.getByRole("region", { name: "Totals" });
  await expect(totals.locator('dd[data-field="objectCount"]')).toHaveText("3");
  await expect(totals.locator('dd[data-field="totalBytes"]')).toHaveText("1.5 MiB");
  const settings = page.getByRole("region", { name: "Settings" });
  await expect(settings.getByLabel("Access")).toHaveValue("policy");
  await expect(settings.getByLabel("Max object bytes")).toHaveValue(String(MIB));
  await expect(settings.getByLabel("Allowed content types")).toHaveValue("image/*");

  // The first page holds two objects and points at the next.
  const objects = page.getByRole("region", { name: "Objects" });
  await expect(objects.locator("tbody tr")).toHaveCount(2);
  await expect(objects.locator('tr[data-object-path="avatars/u1.png"]')).toContainText("512.0 KiB");
  await expect(objects.locator('tr[data-object-path="avatars/u1.png"]')).toContainText(
    "usr_owner001",
  );
  await expect(objects.locator('tr[data-object-path="avatars/u2.png"]')).toContainText("none");
  await objects.getByRole("button", { name: "Load more" }).click();
  await expect(objects.locator("tbody tr")).toHaveCount(3);
  await expect(objects.locator('tr[data-object-path="banners/summer sale.jpg"]')).toBeVisible();
  await expect(objects.getByRole("button", { name: "Load more" })).toBeDisabled();
  await expect(objects.getByText("Every matching object is listed.")).toBeVisible();

  // A prefix narrows the listing at the API.
  await objects.getByLabel("Path prefix").fill("banners/");
  await objects.getByRole("button", { name: "Apply prefix" }).click();
  await expect(objects.locator("tbody tr")).toHaveCount(1);
  await expect(objects.locator('tr[data-object-path="banners/summer sale.jpg"]')).toBeVisible();

  // The dev server mounts under StrictMode, which runs the listing effect
  // twice on mount; the distinct requests are what the screen asked for.
  const listings = distinct(
    api.requests
      .filter((request) => request.method === "GET" && request.path.endsWith("/objects"))
      .map((request) => ({
        prefix: request.query.get("prefix"),
        cursor: request.query.get("cursor"),
        limit: request.query.get("limit"),
      })),
  );
  expect(listings).toEqual([
    { prefix: null, cursor: null, limit: "100" },
    { prefix: null, cursor: "after-2", limit: "100" },
    { prefix: "banners/", cursor: null, limit: "100" },
  ]);

  // Deleting an object asks first, then sends the path segment-encoded.
  await objects.getByRole("button", { name: "Delete object banners/summer sale.jpg" }).click();
  expect(dialogs).toHaveLength(1);
  expect(dialogs[0]).toContain("Delete object banners/summer sale.jpg?");
  expect(dialogs[0]).toContain("This action will be audited.");
  await expect(page.getByRole("status")).toHaveText("Object banners/summer sale.jpg deleted.");
  await expect(objects.locator("tbody tr")).toHaveCount(0);
  await expect(objects.getByText("No objects start with banners/.")).toBeVisible();
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes.map((request) => request.path)).toEqual([
    `${BUCKETS_PATH}/avatars/objects/banners/summer%20sale.jpg`,
  ]);
  expect(api.objects.avatars?.map((object) => object.path)).toEqual([
    "avatars/u1.png",
    "avatars/u2.png",
  ]);
  // The bucket's totals are re-read after the removal.
  await expect(totals.locator('dd[data-field="objectCount"]')).toHaveText("2");
  expect(api.unhandled).toEqual([]);
});

test("saving bucket settings patches the bucket with an idempotency key", async ({ page }) => {
  const api = new StorageApiHarness();
  await api.install(page);

  await page.goto(`${STORAGE_URL}/exports`);
  const settings = page.getByRole("region", { name: "Settings" });
  await settings.getByLabel("Access").selectOption("policy");
  await settings.getByLabel("Max object bytes").fill(String(4 * MIB));
  await settings.getByLabel("Allowed content types").fill("text/csv");
  await settings.getByLabel("Rules (JSON)").fill(JSON.stringify(OWNER_RULES));
  await settings.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toHaveText("Bucket settings saved.");
  await expect(
    page.getByRole("region", { name: "Totals" }).locator('dd[data-field="version"]'),
  ).toHaveText("2");

  const patches = api.requests.filter((request) => request.method === "PATCH");
  expect(patches.map((request) => request.path)).toEqual([`${BUCKETS_PATH}/exports`]);
  expect(patches[0]?.body).toEqual({
    access: "policy",
    maxObjectBytes: 4 * MIB,
    allowedContentTypes: ["text/csv"],
    rules: OWNER_RULES,
  });
  expect(patches[0]?.headers["idempotency-key"]).toMatch(UUID);
  expect(api.unhandled).toEqual([]);
});

test("deleting a non-empty bucket needs a second confirmation before objects go", async ({
  page,
}) => {
  const api = new StorageApiHarness();
  await api.install(page);
  const dialogs: string[] = [];
  const decisions: boolean[] = [];
  page.on("dialog", (dialog) => {
    dialogs.push(dialog.message());
    void (decisions.shift() === true ? dialog.accept() : dialog.dismiss());
  });

  await page.goto(STORAGE_URL);
  await expect(page.locator('tr[data-bucket-id="avatars"]')).toBeVisible();

  // First confirmation, a 409 because objects remain, second confirmation
  // naming what is lost, then the delete carries deleteObjects=true.
  decisions.push(true, true);
  await page.getByRole("button", { name: "Delete bucket avatars" }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Bucket avatars deleted with 3 objects (1.5 MiB).",
  );
  await expect(page.locator('tr[data-bucket-id="avatars"]')).toHaveCount(0);
  expect(dialogs).toHaveLength(2);
  expect(dialogs[0]).toContain("Delete bucket avatars?");
  expect(dialogs[1]).toContain("Delete bucket avatars and its 3 objects (1.5 MiB)?");
  expect(dialogs[1]).toContain("permanently removed");

  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(
    deletes.map((request) => ({
      path: request.path,
      deleteObjects: request.query.get("deleteObjects"),
      confirmation: request.headers.confirmation,
    })),
  ).toEqual([
    { path: `${BUCKETS_PATH}/avatars`, deleteObjects: null, confirmation: "delete:avatars" },
    { path: `${BUCKETS_PATH}/avatars`, deleteObjects: "true", confirmation: "delete:avatars" },
  ]);
  expect(api.buckets.map((bucket) => bucket.id)).toEqual(["exports"]);

  // Dismissing the first dialog sends nothing at all.
  decisions.push(false);
  await page.getByRole("button", { name: "Delete bucket exports" }).click();
  expect(dialogs).toHaveLength(3);
  await expect(page.locator('tr[data-bucket-id="exports"]')).toBeVisible();
  expect(api.requests.filter((request) => request.method === "DELETE")).toHaveLength(2);
  expect(api.buckets.map((bucket) => bucket.id)).toEqual(["exports"]);
  expect(api.unhandled).toEqual([]);
});

test("dismissing the second confirmation keeps the bucket and its objects", async ({ page }) => {
  const api = new StorageApiHarness();
  await api.install(page);
  const decisions = [true, false];
  page.on("dialog", (dialog) => {
    void (decisions.shift() === true ? dialog.accept() : dialog.dismiss());
  });

  await page.goto(STORAGE_URL);
  await page.getByRole("button", { name: "Delete bucket avatars" }).click();
  await expect(page.getByRole("button", { name: "Delete bucket avatars" })).toBeEnabled();
  await expect(page.locator('tr[data-bucket-id="avatars"]')).toBeVisible();
  const deletes = api.requests.filter((request) => request.method === "DELETE");
  expect(deletes.map((request) => request.query.get("deleteObjects"))).toEqual([null]);
  expect(api.buckets.map((bucket) => bucket.id)).toEqual(["avatars", "exports"]);
  expect(api.objects.avatars).toHaveLength(3);
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly query: URLSearchParams;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

interface BucketFixture {
  id: string;
  access: "policy" | "public";
  maxObjectBytes: number;
  allowedContentTypes: string[];
  rules: typeof OWNER_RULES;
  version: number;
  objectCount: number;
  totalBytes: number;
  createdAt: string;
  updatedAt: string;
}

interface ObjectFixture {
  path: string;
  contentType: string;
  sizeBytes: number;
  ownerId: string | null;
  digest: string;
  createdAt: string;
  updatedAt: string;
}

class StorageApiHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  buckets: BucketFixture[] = [
    {
      id: "avatars",
      access: "policy",
      maxObjectBytes: MIB,
      allowedContentTypes: ["image/*"],
      rules: OWNER_RULES,
      version: 1,
      objectCount: 3,
      totalBytes: 1.5 * MIB,
      createdAt: NOW,
      updatedAt: LATER,
    },
    {
      id: "exports",
      access: "public",
      maxObjectBytes: 16 * MIB,
      allowedContentTypes: [],
      rules: [],
      version: 1,
      objectCount: 0,
      totalBytes: 0,
      createdAt: NOW,
      updatedAt: NOW,
    },
  ];
  objects: Record<string, ObjectFixture[]> = {
    avatars: [
      objectFixture("avatars/u1.png", "image/png", 512 * 1024, "usr_owner001"),
      objectFixture("avatars/u2.png", "image/png", 256 * 1024, null),
      objectFixture("banners/summer sale.jpg", "image/jpeg", 768 * 1024, "usr_owner001"),
    ],
    exports: [],
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
    } else if (path === BUCKETS_PATH && method === "GET") {
      await json(route, { items: this.buckets });
    } else if (path === BUCKETS_PATH && method === "POST") {
      if (this.createRefusal !== null) {
        await json(route, apiError("invalid_request", this.createRefusal), 400);
        return;
      }
      const input = request.postDataJSON() as Omit<
        BucketFixture,
        "version" | "objectCount" | "totalBytes" | "createdAt" | "updatedAt"
      >;
      const bucket: BucketFixture = {
        ...input,
        version: 1,
        objectCount: 0,
        totalBytes: 0,
        createdAt: LATER,
        updatedAt: LATER,
      };
      this.buckets.push(bucket);
      this.objects[bucket.id] = [];
      await json(route, bucket, 201);
    } else if (path.startsWith(`${BUCKETS_PATH}/`)) {
      await this.handleBucket(route, path.slice(BUCKETS_PATH.length + 1), method, url);
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    }
  }

  private async handleBucket(route: Route, rest: string, method: string, url: URL) {
    const [bucketId, ...tail] = rest.split("/");
    const bucket = this.buckets.find((item) => item.id === bucketId);
    if (bucketId === undefined || bucket === undefined) {
      await json(route, apiError("not_found", "storage bucket not found"), 404);
      return;
    }
    const objects = this.objects[bucket.id] ?? [];
    if (tail.length === 0 && method === "GET") {
      await json(route, bucket);
    } else if (tail.length === 0 && method === "PATCH") {
      const patch = route.request().postDataJSON() as Partial<BucketFixture>;
      Object.assign(bucket, patch, { version: bucket.version + 1, updatedAt: LATER });
      await json(route, bucket);
    } else if (tail.length === 0 && method === "DELETE") {
      if (route.request().headers().confirmation !== `delete:${bucket.id}`) {
        await json(route, apiError("invalid_request", "confirmation header is required"), 400);
        return;
      }
      if (objects.length > 0 && url.searchParams.get("deleteObjects") !== "true") {
        await json(route, apiError("conflict", "bucket still holds objects"), 409);
        return;
      }
      this.buckets = this.buckets.filter((item) => item.id !== bucket.id);
      delete this.objects[bucket.id];
      await json(route, {
        objectCount: objects.length,
        totalBytes: objects.reduce((sum, object) => sum + object.sizeBytes, 0),
      });
    } else if (tail[0] === "objects" && tail.length === 1 && method === "GET") {
      const prefix = url.searchParams.get("prefix") ?? "";
      const cursor = url.searchParams.get("cursor");
      const offset = cursor === null ? 0 : Number(cursor.replace("after-", ""));
      const matching = objects.filter((object) => object.path.startsWith(prefix));
      const items = matching.slice(offset, offset + this.pageSize);
      const end = offset + items.length;
      await json(route, { items, nextCursor: end < matching.length ? `after-${end}` : null });
    } else if (tail[0] === "objects" && tail.length > 1 && method === "DELETE") {
      const objectPath = tail.slice(1).map(decodeURIComponent).join("/");
      const object = objects.find((item) => item.path === objectPath);
      if (object === undefined) {
        await json(route, apiError("not_found", "storage object not found"), 404);
        return;
      }
      this.objects[bucket.id] = objects.filter((item) => item.path !== objectPath);
      bucket.objectCount -= 1;
      bucket.totalBytes -= object.sizeBytes;
      await json(route, object);
    } else {
      this.unhandled.push(`${method} ${url.pathname}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${url.pathname}`), 404);
    }
  }
}

function objectFixture(
  path: string,
  contentType: string,
  sizeBytes: number,
  ownerId: string | null,
): ObjectFixture {
  return {
    path,
    contentType,
    sizeBytes,
    ownerId,
    digest: `sha256:${"ab".repeat(32)}`,
    createdAt: NOW,
    updatedAt: LATER,
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
