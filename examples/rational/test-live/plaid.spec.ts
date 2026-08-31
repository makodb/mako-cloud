import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * Plaid Sandbox end to end, opt-in: this spec runs only when the environment
 * carried real sandbox credentials into the bootstrap (`PLAID_CLIENT_ID` and
 * `PLAID_SECRET`). CI never sets them, so CI never touches the network.
 *
 * The Link widget is skipped the way Plaid's own docs suggest for tests:
 * `/sandbox/public_token/create` mints the public token the widget would
 * have, and the app's own exchange route takes it from there. Everything
 * after that is the real machinery — the access token crossing into
 * `plaid_items` under the service credential, the worker reaching
 * `sandbox.plaid.com` only because its deployment declared it, the
 * fifteen-minute schedule running the sync, and the cursor keeping a second
 * run from importing anything twice.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

const devices: { context: BrowserContext; page: Page; directory: string }[] = [];

async function openDevice(user: { email: string; password: string }): Promise<Page> {
  const directory = mkdtempSync(join(tmpdir(), "rational-plaid-device-"));
  const context = await chromium.launchPersistentContext(directory, { headless: true });
  const page = context.pages()[0] ?? (await context.newPage());
  await page.addInitScript(
    (overrides) => {
      window.__RATIONAL__ = overrides;
    },
    {
      mode: "live",
      endpoint: "same-origin",
      functionsEndpoint: "same-origin",
      projectId: tenant.projectId,
      environmentId: tenant.environmentId,
      publicProjectKey: tenant.publicProjectKey,
      retryTimeMs: 500,
    },
  );
  devices.push({ context, page, directory });
  await page.goto(tenant.appUrl);
  await page.waitForFunction(() => window.rational !== undefined, undefined, { timeout: 60_000 });
  await page.waitForFunction(() => window.rational.state.phase !== "starting", undefined, {
    timeout: 60_000,
  });
  await page.evaluate(
    ([email, password]) => window.rational.signIn(email as string, password as string),
    [user.email, user.password],
  );
  await page.waitForFunction(() => window.rational.state.phase === "ready", undefined, {
    timeout: 60_000,
  });
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 60_000 },
  );
  return page;
}

test.skip(
  !tenant.plaidConfigured,
  "Plaid Sandbox credentials were not provided (set PLAID_CLIENT_ID and PLAID_SECRET to run this opt-in spec)",
);

test.afterEach(async () => {
  for (const device of devices.splice(0)) {
    await device.context.close().catch(() => undefined);
    rmSync(device.directory, { recursive: true, force: true });
  }
});

test("a sandbox institution links, syncs through the schedule, and does not double", async () => {
  test.setTimeout(300_000);
  const page = await openDevice(tenant.owner);
  const householdId = await page.evaluate(() => window.rational.createHousehold("Linked", "USD"));
  await page.waitForFunction((id) => window.rational.state.currentHouseholdId === id, householdId, {
    timeout: 60_000,
  });
  await page.waitForFunction(() => window.rational.writes !== null, undefined, { timeout: 60_000 });
  const accountId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 500_000,
      opening_date: "2026-01-01",
    });
    return String(account.id);
  });
  await page.evaluate(() => window.rational.waitForSync());

  // The widget's part, played by Plaid's sandbox itself.
  const minted = await plaidSandbox("/sandbox/public_token/create", {
    institution_id: "ins_109508",
    initial_products: ["transactions"],
  });
  const publicToken = String((minted as { public_token?: unknown }).public_token ?? "");
  expect(publicToken, "the sandbox minted a public token").not.toBe("");

  // The deployment reports itself configured, and the exchange goes through
  // the app's own client -- the browser holds the public token briefly and an
  // access token never.
  const connectionId = await page.evaluate(
    async ([token, household, account]) => {
      const plaid = window.rational.plaid;
      if (plaid === null) throw new Error("the plaid client is not wired");
      if (!(await plaid.configured())) throw new Error("the deployment says it is unconfigured");
      return await plaid.exchange({
        publicToken: token as string,
        householdId: household as string,
        accountId: account as string,
        institution: "First Platypus Bank",
      });
    },
    [publicToken, householdId, accountId],
  );
  expect(connectionId).toMatch(/^con_plaid-/u);

  // The sync runs the way it always runs: through the schedule.
  const schedules = mako(["schedules", "list", "--function", "institution-sync"]) as {
    id: string;
    name: string;
  }[];
  const schedule = schedules.find((entry) => entry.name === "every-fifteen-minutes");
  expect(schedule, "the bootstrap created the sync schedule").toBeDefined();
  mako(["schedules", "run-now", schedule?.id ?? "", "--function", "institution-sync"]);
  await expect
    .poll(() => finished(schedule?.id ?? "").length, { timeout: 120_000 })
    .toBeGreaterThanOrEqual(1);
  const [first] = finished(schedule?.id ?? "");
  expect(first?.outcome, `the sync run did not succeed: ${JSON.stringify(first)}`).toBe(
    "succeeded",
  );

  // Sandbox institutions come with transaction history; some of it lands.
  await expect
    .poll(() => countTransactions(page, accountId), {
      timeout: 120_000,
      message: "no transactions replicated from the linked institution",
    })
    .toBeGreaterThan(0);
  const afterFirst = await countTransactions(page, accountId);

  // Run the night... the fifteen minutes again: the cursor absorbs it.
  mako(["schedules", "run-now", schedule?.id ?? "", "--function", "institution-sync"]);
  await expect.poll(() => finished(schedule?.id ?? "").length, { timeout: 120_000 }).toBe(2);
  const [, second] = finished(schedule?.id ?? "");
  expect(second?.outcome).toBe("succeeded");
  await page.evaluate(() => window.rational.waitForSync());
  expect(await countTransactions(page, accountId)).toBe(afterFirst);
});

async function countTransactions(page: Page, accountId: string): Promise<number> {
  return await page.evaluate(async (account) => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) return 0;
    const documents = await collection.find().exec();
    return documents.filter((document) => document.toJSON().account_id === account).length;
  }, accountId);
}

/** One call to Plaid's sandbox from the test itself, playing the widget. */
async function plaidSandbox(path: string, payload: Record<string, unknown>): Promise<unknown> {
  const response = await fetch(`https://sandbox.plaid.com${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_id: process.env.PLAID_CLIENT_ID,
      secret: process.env.PLAID_SECRET,
      ...payload,
    }),
  });
  if (!response.ok) {
    throw new Error(`plaid sandbox ${path} refused: ${response.status}`);
  }
  return await response.json();
}

/** The management API, driven as the developer's own tooling drives it. */
function mako(args: readonly string[]): unknown {
  const cli = join(process.cwd(), "..", "..", "node_modules", ".bin", "mako");
  const run = spawnSync(cli, [...args, "--json"], {
    encoding: "utf8",
    env: {
      ...process.env,
      MAKO_ENDPOINT: tenant.managementEndpoint,
      MAKO_PROJECT_ID: tenant.projectId,
      MAKO_ENVIRONMENT_ID: tenant.environmentId,
      MAKO_TOKEN: tenant.developerToken,
      MAKO_TOKEN_KIND: "developer_session",
      MAKO_CONFIG_DIR: tenant.cliConfigDir,
    },
  });
  if (run.status !== 0) {
    throw new Error(`mako ${args.join(" ")} failed (${run.status}): ${run.stderr}`);
  }
  return JSON.parse(run.stdout) as unknown;
}

interface ScheduleRun {
  readonly outcome: string | null;
  readonly responseStatus: number | null;
}

function finished(scheduleId: string): readonly ScheduleRun[] {
  if (scheduleId === "") return [];
  const listed = mako(["schedules", "runs", scheduleId, "--function", "institution-sync"]) as {
    items?: ScheduleRun[];
  };
  return (listed.items ?? []).filter((run) => run.outcome !== null);
}
