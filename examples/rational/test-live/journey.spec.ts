import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * The whole journey as one person would live it, against the real stack and
 * the real aggregator: sign up on the sign-in screen, make a household, link
 * a bank through the Connections screen, watch the scheduled sync bring the
 * bank's transactions onto the Transactions screen, and wake up to find the
 * nightly job filed them under the rule the person wrote.
 *
 * Every other suite proves a segment; this one proves the thread. It is
 * opt-in like the Plaid spec -- the aggregator is real, so CI never runs it
 * -- and the only stub is Plaid's own Link widget, replaced by a handler
 * that delivers a real sandbox public token minted the way Plaid's docs
 * suggest for tests. Everything downstream of that token is live.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

const devices: { context: BrowserContext; page: Page; directory: string }[] = [];

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

test("a person signs up, links a bank, and wakes to filed transactions", async () => {
  test.setTimeout(420_000);

  // The widget's part, played by Plaid's sandbox before the page even opens.
  const minted = await plaidSandbox("/sandbox/public_token/create", {
    institution_id: "ins_109508",
    initial_products: ["transactions"],
  });
  const publicToken = String((minted as { public_token?: unknown }).public_token ?? "");
  expect(publicToken, "the sandbox minted a public token").not.toBe("");

  const directory = mkdtempSync(join(tmpdir(), "rational-journey-"));
  const context = await chromium.launchPersistentContext(directory, { headless: true });
  const page = context.pages()[0] ?? (await context.newPage());
  devices.push({ context, page, directory });
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
  await page.addInitScript((token) => {
    (window as unknown as Record<string, unknown>).Plaid = {
      create(options: { onSuccess: (publicToken: string, metadata: unknown) => void }) {
        return {
          open() {
            options.onSuccess(token as string, {
              institution: { name: "First Platypus Bank" },
            });
          },
        };
      },
    };
  }, publicToken);

  // --- Sign up, on the screen a new person sees. -------------------------
  await page.goto(tenant.appUrl);
  await page.waitForFunction(() => window.rational !== undefined, undefined, { timeout: 60_000 });
  await page.waitForFunction(() => window.rational.state.phase !== "starting", undefined, {
    timeout: 60_000,
  });
  const email = `journey-${Date.now()}@rational.test`;
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill("RationalDemo1!");
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready", undefined, {
    timeout: 60_000,
  });
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 60_000 },
  );

  // --- A household of their own. -----------------------------------------
  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  await page.evaluate(() => window.rational.createHousehold("My money", "USD"));
  await page.waitForFunction(
    (previous) =>
      window.rational.state.currentHouseholdId !== previous &&
      window.rational.state.currentHouseholdId !== null,
    before,
    { timeout: 60_000 },
  );
  await page.waitForFunction(() => window.rational.writes !== null, undefined, { timeout: 60_000 });

  // The account the bank will fill, and the rule the nightly will apply --
  // written before any transaction exists, the way a person sets up a rule
  // for charges they know are coming.
  await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    await writes.createAccount({
      name: "Checking",
      type: "checking",
      currency: "USD",
      opening_balance: 0,
      opening_date: "2026-01-01",
    });
    const category = await writes.createCategory("Ride sharing", "expense");
    await writes.createRule({
      name: "Uber rides",
      match: { description_contains: "uber" },
      set_category_id: String(category.id),
      priority: 1,
    });
  });
  await page.evaluate(() => window.rational.waitForSync());

  // --- Link the bank, from the Connections screen. -----------------------
  await page
    .getByRole("navigation", { name: "Sections" })
    .getByRole("link", { name: "Settings" })
    .click();
  await page
    .getByRole("navigation", { name: "Settings pages" })
    .getByRole("link", { name: "Connections" })
    .click();
  await expect(page.getByTestId("connections-screen")).toBeVisible();
  const plaidForm = page.getByTestId("plaid-connect");
  await expect(plaidForm).toBeVisible();
  await plaidForm.getByLabel("Account").selectOption({ label: "Checking" });
  await plaidForm.getByRole("button", { name: "Connect through Plaid" }).click();
  await expect(
    page.getByRole("table", { name: "Connected accounts" }).getByText("First Platypus Bank"),
  ).toBeVisible({ timeout: 60_000 });

  // --- The sync runs the way it always runs: on the schedule. ------------
  // Plaid's sandbox backfills history asynchronously, so the schedule may
  // need a few passes before the ride shares arrive.
  const schedules = makoCloud(["schedules", "list", "--function", "institution-sync"]) as {
    id: string;
    name: string;
  }[];
  const syncSchedule = schedules.find((entry) => entry.name === "every-fifteen-minutes");
  expect(syncSchedule, "the bootstrap created the sync schedule").toBeDefined();
  await expect
    .poll(
      async () => {
        makoCloud([
          "schedules",
          "run-now",
          syncSchedule?.id ?? "",
          "--function",
          "institution-sync",
        ]);
        await new Promise((resolve) => setTimeout(resolve, 15_000));
        return await page.evaluate(async () => {
          const collection = window.rational.household?.session?.collections.transactions;
          if (collection === undefined) return "";
          const documents = await collection.find().exec();
          return documents.map((document) => String(document.toJSON().description)).join("|");
        });
      },
      { timeout: 240_000, message: "the bank's ride shares never arrived" },
    )
    .toMatch(/uber/iu);

  // --- The Transactions screen shows what the bank sent. -----------------
  await page
    .getByRole("navigation", { name: "Sections" })
    .getByRole("link", { name: "Transactions" })
    .click();
  const uberRow = page
    .locator('[data-testid^="transaction-txn_"]')
    .filter({ hasText: /uber/iu })
    .first();
  await expect(uberRow).toBeVisible({ timeout: 30_000 });

  // --- The night runs, and the rule files the ride. ----------------------
  const nightly = (
    makoCloud(["schedules", "list", "--function", "nightly"]) as { id: string; name: string }[]
  ).find((entry) => entry.name === "nightly");
  expect(nightly, "the bootstrap created the nightly schedule").toBeDefined();
  makoCloud(["schedules", "run-now", nightly?.id ?? "", "--function", "nightly"]);
  await expect
    .poll(
      () => {
        const listed = makoCloud([
          "schedules",
          "runs",
          nightly?.id ?? "",
          "--function",
          "nightly",
        ]) as {
          items?: { outcome: string | null }[];
        };
        return (listed.items ?? []).filter((run) => run.outcome !== null).length;
      },
      { timeout: 120_000 },
    )
    .toBeGreaterThanOrEqual(1);

  await page.evaluate(() => window.rational.waitForSync());
  await expect(uberRow.getByTestId("filed-by")).toHaveText(/by Uber rides/u, { timeout: 60_000 });
  await expect(uberRow).toContainText("Ride sharing");
});

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
function makoCloud(args: readonly string[]): unknown {
  const cli = join(process.cwd(), "..", "..", "node_modules", ".bin", "mako-cloud");
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
    throw new Error(`mako-cloud ${args.join(" ")} failed (${run.status}): ${run.stderr}`);
  }
  return JSON.parse(run.stdout) as unknown;
}
