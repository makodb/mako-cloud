import { expect, test, type Page, type Route } from "@playwright/test";

const NOW = "2026-08-06T12:00:00.000Z";
const TEAM_ID = "org_abcdefgh";
const PROJECT_ID = "prj_abcdefgh";
const ENVIRONMENT_ID = "env_abcdefgh";
const API_URL = "https://api.cloud-test.makodb.com";
const PUBLIC_KEY_ID = "pk_primary";
const PUBLIC_KEY_VALUE = "mako_pk.live0123456789abcdef";
const PUBLIC_KEY_PLACEHOLDER = "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY";
// A service credential the harness holds and the page must never show.
const SERVICE_SECRET = "mako_sk.NEVER_SHOWN_0123456789abcdef";
const ENVIRONMENT_PATH = `/v1/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}`;
const DOCS_URL = `/projects/${PROJECT_ID}/environments/${ENVIRONMENT_ID}/api-docs`;
const ISO_TIME = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/u;

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

// The API docs screen renders what the environment actually has, from the
// same endpoints the rest of the console reads. The wire is mocked, so every
// assertion is about what the page derives from a response.
test("the reference is generated from the environment's schema, policy, functions, buckets, and key", async ({
  page,
}) => {
  const api = new ApiDocsHarness();
  await api.install(page);

  await page.goto(DOCS_URL);
  await expect(page.getByRole("heading", { name: "This environment's API" })).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "Generated from" })).toHaveText(
    /Generated from the environment at \d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/u,
  );
  const generatedAt = await page.locator(".api-docs-generated time").getAttribute("datetime");
  expect(generatedAt).toMatch(ISO_TIME);

  // It is a real destination, current while open.
  const destinations = page.getByRole("navigation", { name: "Environment destinations" });
  const link = destinations.getByRole("link", { name: "API docs", exact: true });
  await expect(link).toHaveAttribute("href", DOCS_URL);
  await expect(link).toHaveAttribute("aria-current", "page");

  // Overview: the API URL and the public key id, never a service credential.
  const overview = page.locator("#api-docs-overview");
  const facts = overview.locator(".definition-grid");
  await expect(facts.getByText(API_URL, { exact: true })).toBeVisible();
  await expect(facts.getByText(PUBLIC_KEY_ID, { exact: true })).toBeVisible();
  await expect(overview).toContainText("X-Mako-Key");
  await expect(overview).toContainText("Authorization: Bearer");
  await expect(overview).toContainText(`use the placeholder ${PUBLIC_KEY_PLACEHOLDER}`);

  // Auth: the environment's provider and registered redirect are what the flow uses.
  const auth = page.locator("#api-docs-auth");
  await expect(auth).toContainText("Providers configured: github; magic links are enabled");
  await expect(auth.locator('[data-example-id="auth-signin"] .api-docs-url')).toHaveText(
    `${API_URL}${ENVIRONMENT_PATH}/auth/signin`,
  );
  await expect(auth.locator('[data-example-id="auth-signin"] pre').first()).toContainText(
    `X-Mako-Key: ${PUBLIC_KEY_PLACEHOLDER}`,
  );
  await expect(auth.locator('[data-example-id="auth-provider-start"] .api-docs-url')).toHaveText(
    `${API_URL}${ENVIRONMENT_PATH}/auth/providers/github/start`,
  );
  await expect(auth.locator('[data-example-id="auth-provider-start"] pre').first()).toContainText(
    "https://app.example.test/auth/callback",
  );
  for (const id of [
    "auth-signup",
    "auth-refresh",
    "auth-user",
    "auth-provider-callback",
    "auth-provider-exchange",
    "auth-magic-link",
    "auth-magic-link-redeem",
  ]) {
    await expect(auth.locator(`[data-example-id="${id}"]`)).toBeVisible();
  }

  // Collections: the document shape comes from the schema.
  const todos = page.locator('[data-collection-id="todos"]');
  await expect(todos).toContainText("Schema version 3 · primary key id");
  const title = todos.locator('tr[data-property="title"]');
  await expect(title.locator("td").nth(0)).toHaveText("string");
  await expect(title.locator("td").nth(1)).toHaveText("required");
  await expect(title.locator("td").nth(2)).toHaveText("What to do");
  await expect(todos.locator('tr[data-property="id"]')).toContainText("primary key");
  await expect(todos.locator('tr[data-property="done"]').locator("td").nth(0)).toHaveText(
    "boolean",
  );
  await expect(todos.locator('tr[data-property="done"]').locator("td").nth(1)).toHaveText(
    "optional",
  );
  await expect(todos.locator('tr[data-property="priority"]').locator("td").nth(0)).toHaveText(
    'enum: "low" | "high"',
  );
  await expect(todos.locator('tr[data-property="updatedAt"]').locator("td").nth(0)).toHaveText(
    "integer",
  );
  await expect(todos.locator(".api-docs-indexes")).toContainText("by-updated-at v1");
  await expect(todos.locator(".api-docs-indexes")).toContainText("updatedAt ↑");

  // Operations are derived from the active policy: true allows, any other
  // expression is conditional, nothing covering the operation denies, and
  // deny rules are noted where they apply.
  await expect(todos).toContainText("Derived from active policy version 2 (authorization epoch 4)");
  const operation = (name: string) => todos.locator(`[data-operation="${name}"]`);
  await expect(operation("read")).toHaveAttribute("data-access", "allowed");
  await expect(operation("read")).toContainText("Allowed");
  await expect(operation("create")).toHaveAttribute("data-access", "conditional");
  await expect(operation("create")).toContainText("new.ownerId == identity.user_id");
  await expect(operation("update")).toHaveAttribute("data-access", "conditional");
  await expect(operation("update")).toContainText("Denied when old.locked == true");
  await expect(operation("delete")).toHaveAttribute("data-access", "denied");
  await expect(operation("delete")).toContainText("No allow rule names this operation");
  await expect(operation("create")).not.toContainText("Denied when");

  // A collection with no active policy denies everything.
  const notes = page.locator('[data-collection-id="notes"]');
  await expect(notes).toContainText("No policy is active on this collection");
  for (const name of ["create", "read", "update", "delete"]) {
    await expect(notes.locator(`[data-operation="${name}"]`)).toHaveAttribute(
      "data-access",
      "denied",
    );
  }
  await expect(notes).toContainText("No secondary indexes");

  // Example requests carry the collection's own URL and a document built from its schema.
  await expect(todos.locator('[data-example-id="todos-query"] .api-docs-url')).toHaveText(
    `${API_URL}${ENVIRONMENT_PATH}/collections/todos/documents/query`,
  );
  const create = todos.locator('[data-example-id="todos-create"] pre').first();
  await expect(create).toContainText(
    `${API_URL}${ENVIRONMENT_PATH}/collections/todos/documents/todos-1`,
  );
  await expect(create).toContainText('"title": "title-example"');
  await expect(create).toContainText('"done": true');
  await expect(create).toContainText('"priority": "low"');
  await expect(create).toContainText('"schemaVersion": 3');
  await expect(create).toContainText("Authorization: Bearer $MAKO_ACCESS_TOKEN");
  // The query predicates on the indexed field.
  await expect(todos.locator('[data-example-id="todos-query"] pre').first()).toContainText(
    '"field": "updatedAt"',
  );
  for (const id of ["todos-read", "todos-update", "todos-delete", "todos-pull", "todos-push"]) {
    await expect(todos.locator(`[data-example-id="${id}"]`)).toBeVisible();
  }
  await expect(todos.locator('[data-example-id="todos-stream"] .api-docs-url')).toHaveText(
    `${API_URL}${ENVIRONMENT_PATH}/collections/todos/replication/stream?schemaVersion=3`,
  );

  // Functions: the route the gateway serves, the active version, and the caller.
  const hello = page.locator('[data-function-name="hello-world"]');
  await expect(hello).toContainText(
    `${API_URL}/${PROJECT_ID}--${ENVIRONMENT_ID}/functions/v1/hello-world`,
  );
  await expect(hello).toContainText("v3");
  await expect(hello).toContainText("Application session required");
  await expect(hello.locator("pre").first()).toContainText("Authorization: Bearer");
  const webhook = page.locator('[data-function-name="public-webhook"]');
  await expect(webhook).toContainText("None deployed");
  await expect(webhook).toContainText("Public: no session is verified");
  await expect(webhook.locator("pre").first()).not.toContainText("Authorization");

  // Storage: each bucket's own upload and download.
  const avatars = page.locator('[data-bucket-id="avatars"]');
  await expect(
    avatars.locator('[data-example-id="bucket-avatars-upload"] .api-docs-url'),
  ).toHaveText(`${API_URL}${ENVIRONMENT_PATH}/storage/avatars/objects/uploads/example.png`);
  await expect(
    avatars.locator('[data-example-id="bucket-avatars-upload"] pre').first(),
  ).toContainText("Content-Type: image/png");
  const exports = page.locator('[data-bucket-id="exports"]');
  await expect(exports).toContainText("Public: anyone may download");
  await expect(
    exports.locator('[data-example-id="bucket-exports-download"] pre').first(),
  ).not.toContainText("Authorization");

  // Quickstarts carry the API URL and the key id for every client.
  const quickstarts = page.locator("#api-docs-quickstarts");
  await expect(quickstarts).toContainText("Service credentials never appear here.");
  const panel = quickstarts.getByRole("tabpanel");
  await expect(panel).toContainText(`export MAKO_API_URL="${API_URL}"`);
  await expect(panel).toContainText(`# key id ${PUBLIC_KEY_ID}`);
  await expect(panel).toContainText("/collections/todos/documents/query");
  await expect(panel).toContainText("functions/v1/hello-world");
  await quickstarts.getByRole("tab", { name: "JavaScript (fetch)" }).click();
  await expect(panel).toContainText(`const API_URL = "${API_URL}";`);
  await expect(panel).toContainText(`// key id ${PUBLIC_KEY_ID}`);
  await expect(panel).toContainText("async function queryTodos(");
  await quickstarts.getByRole("tab", { name: "RxDB replication" }).click();
  await expect(panel).toContainText("normalizeMakoRxdbConfig({");
  await expect(panel).toContainText(`endpoint: "${API_URL}"`);
  await expect(panel).toContainText('collectionId: "todos"');
  await expect(panel).toContainText("schemaVersion: 3");
  await expect(panel).toContainText(`publicProjectKey: "${PUBLIC_KEY_PLACEHOLDER}"`);
  await expect(panel).toContainText("replicateRxCollection");

  // No secret material anywhere on the page, and the console never asked for any.
  const content = await page.content();
  expect(content).not.toContain(SERVICE_SECRET);
  expect(content).not.toContain("mako_sk.");
  expect(api.requests.filter((request) => request.path.includes("/credentials"))).toEqual([]);
  expect(api.requests.every((request) => request.method === "GET")).toBe(true);
  expect(api.unhandled).toEqual([]);
});

test("a service credential on file is refused before it can reach a snippet", async ({ page }) => {
  const api = new ApiDocsHarness();
  // A misbehaving backend hands the console a service credential where the
  // public key belongs. The guard refuses it: the section says so, the
  // examples fall back to the placeholder, and the value is nowhere.
  api.publicKey = SERVICE_SECRET;
  await api.install(page);

  await page.goto(DOCS_URL);
  await expect(page.locator("#api-docs-overview").getByRole("status")).toContainText(
    "Connection metadata could not be read: API documentation carries public project keys only",
  );
  await expect(page.locator("#api-docs-quickstarts").getByRole("tabpanel")).toContainText(
    `export MAKO_PUBLIC_KEY="${PUBLIC_KEY_PLACEHOLDER}"`,
  );
  // The other sections are unaffected.
  await expect(page.locator('[data-collection-id="todos"]')).toBeVisible();
  const content = await page.content();
  expect(content).not.toContain(SERVICE_SECRET);
  expect(content).not.toContain("mako_sk.");
  expect(api.unhandled).toEqual([]);
});

test("an unreadable source marks only its own section, and a public key on file is used", async ({
  page,
}) => {
  const api = new ApiDocsHarness();
  api.functionsFailure = true;
  api.publicKey = PUBLIC_KEY_VALUE;
  await api.install(page);

  await page.goto(DOCS_URL);
  const functions = page.locator("#api-docs-functions");
  await expect(functions.getByRole("status")).toContainText(
    "Functions could not be read: the function registry is unavailable",
  );
  await expect(functions).toContainText("The other sections are unaffected");
  // Everything else still renders from its own read.
  await expect(page.locator(".api-docs-generated")).toContainText(
    "Generated from the environment at",
  );
  await expect(
    page.locator('[data-collection-id="todos"] [data-operation="read"]'),
  ).toHaveAttribute("data-access", "allowed");
  await expect(page.locator('[data-bucket-id="avatars"]')).toBeVisible();
  // The console holds a public key value, so the quickstart carries it.
  const panel = page.locator("#api-docs-quickstarts").getByRole("tabpanel");
  await expect(panel).toContainText(`export MAKO_PUBLIC_KEY="${PUBLIC_KEY_VALUE}"`);
  await expect(panel).not.toContainText(PUBLIC_KEY_PLACEHOLDER);
  await expect(panel).not.toContainText("functions/v1");
  const content = await page.content();
  expect(content).not.toContain("mako_sk.");
  expect(api.unhandled).toEqual([]);
});

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
}

class ApiDocsHarness {
  readonly unhandled: string[] = [];
  readonly requests: RecordedRequest[] = [];
  publicKey = "";
  functionsFailure = false;

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
    this.requests.push({ method, path });
    if (method !== "GET") {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
      return;
    }
    if (path === `/v1/projects/${PROJECT_ID}`) {
      await json(route, project());
    } else if (path === `/v1/projects/${PROJECT_ID}/environments`) {
      await json(route, { items: [environment()] });
    } else if (path === `${ENVIRONMENT_PATH}/workspace/navigation`) {
      await json(route, navigation());
    } else if (path === `${ENVIRONMENT_PATH}/collections`) {
      await json(route, { items: [todosCollection(), notesCollection()] });
    } else if (path === `${ENVIRONMENT_PATH}/collections/todos/indexes`) {
      await json(route, { items: [updatedAtIndex()] });
    } else if (path === `${ENVIRONMENT_PATH}/collections/notes/indexes`) {
      await json(route, { items: [] });
    } else if (path === `${ENVIRONMENT_PATH}/collections/todos/policies`) {
      await json(route, todosPolicy());
    } else if (path === `${ENVIRONMENT_PATH}/collections/notes/policies`) {
      await json(route, { defaultDeny: true, authorizationEpoch: 1 });
    } else if (path === `${ENVIRONMENT_PATH}/functions`) {
      if (this.functionsFailure) {
        await json(route, apiError("unavailable", "the function registry is unavailable"), 503);
      } else {
        await json(route, { items: [helloWorldFunction(), publicWebhookFunction()] });
      }
    } else if (path === `${ENVIRONMENT_PATH}/storage-buckets`) {
      await json(route, { items: [avatarsBucket(), exportsBucket()] });
    } else if (path === `${ENVIRONMENT_PATH}/auth-settings`) {
      await json(route, authSettings());
    } else if (path === `${ENVIRONMENT_PATH}/connect`) {
      await json(route, connectMetadata(this.publicKey));
    } else if (path === `${ENVIRONMENT_PATH}/credentials/service-worker`) {
      // Present in the harness so its absence from the page is meaningful; the
      // screen has no reason to ask for it.
      await json(route, {
        id: "service-worker",
        kind: "service",
        state: "active",
        createdAt: NOW,
        value: SERVICE_SECRET,
      });
    } else {
      this.unhandled.push(`${method} ${path}`);
      await json(route, apiError("not_found", `No fixture for ${method} ${path}`), 404);
    }
  }
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

function todosCollection() {
  return {
    id: "todos",
    metadataVersion: 5,
    schemaVersion: 3,
    jsonSchema: {
      type: "object",
      properties: {
        id: { type: "string", maxLength: 100 },
        ownerId: { type: "string", maxLength: 100, description: "The user who owns the todo" },
        title: { type: "string", maxLength: 500, description: "What to do" },
        done: { type: "boolean" },
        priority: { type: "string", enum: ["low", "high"] },
        updatedAt: { type: "integer", minimum: 0 },
      },
      required: ["id", "ownerId", "title", "updatedAt"],
    },
    primaryKey: { kind: "field", field: "id" },
    compatibility: "compatible",
    state: "active",
  };
}

function notesCollection() {
  return {
    id: "notes",
    metadataVersion: 1,
    schemaVersion: 1,
    jsonSchema: {
      type: "object",
      properties: { id: { type: "string" }, body: { type: "string" } },
      required: ["id"],
    },
    primaryKey: { kind: "field", field: "id" },
    compatibility: "compatible",
    state: "active",
  };
}

function updatedAtIndex() {
  return {
    collectionId: "todos",
    name: "by-updated-at",
    version: 1,
    kind: "non_unique",
    fields: [{ path: "updatedAt", direction: "ascending" }],
    state: "active",
    activationFenced: false,
  };
}

function todosPolicy() {
  return {
    defaultDeny: true,
    authorizationEpoch: 4,
    policy: {
      version: 2,
      state: "active",
      diagnostics: [],
      rules: [
        { id: "anyone-reads", effect: "allow", operations: ["read"], expression: "true" },
        {
          id: "owner-writes",
          effect: "allow",
          operations: ["create", "update"],
          expression: "new.ownerId == identity.user_id",
        },
        {
          id: "locked-stays",
          effect: "deny",
          operations: ["update"],
          expression: "old.locked == true",
        },
      ],
    },
  };
}

function functionConfiguration(verifyJwt: boolean) {
  return {
    verifyJwt,
    regions: ["local"],
    secretNames: [],
    limits: {
      cpuMilliseconds: 100,
      wallMilliseconds: 1_000,
      memoryBytes: 134_217_728,
      requestBytes: 1_048_576,
      responseBytes: 1_048_576,
      concurrency: 10,
    },
  };
}

function helloWorldFunction() {
  return {
    name: "hello-world",
    state: "active",
    activeVersion: 3,
    configuration: functionConfiguration(true),
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function publicWebhookFunction() {
  return {
    name: "public-webhook",
    state: "active",
    activeVersion: null,
    configuration: functionConfiguration(false),
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function avatarsBucket() {
  return {
    id: "avatars",
    access: "policy",
    maxObjectBytes: 1_048_576,
    allowedContentTypes: ["image/*"],
    rules: [
      {
        id: "owner-creates",
        effect: "allow",
        operations: ["create"],
        expression: "new.owner_id == identity.user_id",
      },
    ],
    version: 1,
    objectCount: 3,
    totalBytes: 1_572_864,
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function exportsBucket() {
  return {
    id: "exports",
    access: "public",
    maxObjectBytes: 16_777_216,
    allowedContentTypes: [],
    rules: [],
    version: 1,
    objectCount: 0,
    totalBytes: 0,
    createdAt: NOW,
    updatedAt: NOW,
  };
}

function authSettings() {
  return {
    providers: [
      {
        name: "github",
        kind: { type: "git_hub" },
        clientId: "Iv1.example",
        scopes: ["read:user", "user:email"],
        enabled: true,
        hasSecret: true,
      },
    ],
    redirectUrls: ["https://app.example.test/auth/callback"],
    magicLinks: { enabled: true, linkTtlSeconds: 900 },
    emailVerification: { required: false },
    version: 2,
  };
}

function connectMetadata(publicKey: string) {
  return {
    tenant: { projectId: PROJECT_ID, environmentId: ENVIRONMENT_ID },
    publicEndpoint: API_URL,
    publicKeyId: PUBLIC_KEY_ID,
    publicKey,
    collections: [
      { collectionId: "todos", activeSchemaVersion: 3 },
      { collectionId: "notes", activeSchemaVersion: 1 },
    ],
    rxdbClientRange: ">=17.0.0 <18.0.0",
    templateVersion: 1,
  };
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
