import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * Rational against a real data plane, with persistent browser contexts so
 * that IndexedDB survives a reload the way it does for a person. Nothing is
 * intercepted: the app signs in for real, its writes are persisted by the
 * server, and the second device is a second browser profile.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

interface Device {
  readonly context: BrowserContext;
  readonly page: Page;
  readonly directory: string;
}

const devices: Device[] = [];

async function openDevice(
  user: { email: string; password: string },
  options: { startOffline?: boolean; directory?: string } = {},
): Promise<Device> {
  const directory = options.directory ?? mkdtempSync(join(tmpdir(), "rational-device-"));
  const context = await chromium.launchPersistentContext(directory, { headless: true });
  const page = context.pages()[0] ?? (await context.newPage());
  await page.addInitScript(
    (overrides) => {
      window.__RATIONAL__ = overrides;
    },
    {
      mode: "live",
      endpoint: "same-origin",
      projectId: tenant.projectId,
      environmentId: tenant.environmentId,
      publicProjectKey: tenant.publicProjectKey,
      retryTimeMs: 500,
      startOffline: options.startOffline === true,
    },
  );
  await page.goto(tenant.appUrl);
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  if ((await page.evaluate(() => window.rational.state.phase)) === "signed_out") {
    await page.getByLabel("Email").fill(user.email);
    await page.getByLabel("Password").fill(user.password);
    await page.getByRole("button", { name: "Sign in" }).click();
  }
  const device = { context, page, directory };
  devices.push(device);
  return device;
}

/**
 * Open the seeded household, and say so rather than assuming.
 *
 * The owner belongs to more than one household by the time this file runs --
 * the households spec creates another under the same person -- and which one
 * a fresh device opens first is the application's choice, not this suite's.
 * Naming it is both more honest and what a person does.
 */
async function waitForHousehold(page: Page): Promise<void> {
  await page.waitForFunction(
    (householdId) =>
      window.rational.state.memberships.some((entry) => entry.household_id === householdId),
    tenant.householdId,
    { timeout: 60_000 },
  );
  await page.evaluate(
    (householdId) => window.rational.selectHousehold(householdId),
    tenant.householdId,
  );
  await page.waitForFunction(
    (householdId) =>
      window.rational.state.currentHouseholdId === householdId &&
      window.rational.household?.session !== null,
    tenant.householdId,
    { timeout: 60_000 },
  );
}

async function transactionCount(page: Page): Promise<number> {
  return page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.transactions;
    if (collection === undefined) return -1;
    return (await collection.find().exec()).length;
  });
}

const diagnostics = (page: Page) => page.evaluate(() => window.rational.diagnostics());

test.afterEach(async () => {
  for (const device of devices.splice(0)) {
    await device.context.close().catch(() => undefined);
    rmSync(device.directory, { recursive: true, force: true });
  }
});

test("a reload keeps the household on the device and resumes from the checkpoint", async () => {
  const device = await openDevice(tenant.owner);
  await waitForHousehold(device.page);
  await device.page.evaluate(() => window.rational.waitForSync());
  await expect
    .poll(() => transactionCount(device.page), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(200);
  const before = await diagnostics(device.page);
  expect(before.received).toBeGreaterThanOrEqual(200);
  const stored = await device.page.evaluate(async () =>
    (await indexedDB.databases()).map((database) => database.name ?? ""),
  );
  expect(stored.some((name) => name.includes("rational-hh_demo"))).toBe(true);

  // Reload: the data is there before any pull, and the pull brings nothing new.
  await device.page.reload();
  await device.page.waitForFunction(() => window.rational !== undefined);
  await waitForHousehold(device.page);
  await expect.poll(() => transactionCount(device.page)).toBeGreaterThanOrEqual(200);
  await device.page.evaluate(() => window.rational.waitForSync());
  const after = await diagnostics(device.page);
  expect(after.received).toBe(0);
  expect(after.pullRequests).toBeGreaterThan(0);

  // Only a change made elsewhere is pulled after the reload.
  const editor = await openDevice(tenant.editor);
  await waitForHousehold(editor.page);
  await editor.page.evaluate(() => window.rational.waitForSync());
  await editor.page.evaluate(() =>
    window.rational.writes?.createTransaction({
      account_id: "acc_demo_cash",
      date: "2026-08-20",
      amount: -1_200,
      currency: "USD",
      description: "Written on another device",
    }),
  );
  await expect
    .poll(() => transactionCount(device.page), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(201);
  const pulled = await diagnostics(device.page);
  expect(pulled.received).toBeGreaterThanOrEqual(1);
  expect(pulled.received).toBeLessThan(10);
});

test("an offline edit is kept locally and pushed after reconnect", async () => {
  const device = await openDevice(tenant.owner);
  await waitForHousehold(device.page);
  await device.page.evaluate(() => window.rational.waitForSync());
  await device.page.goto(`${tenant.appUrl}/#/transactions`);
  await waitForHousehold(device.page);
  const accepted = (await diagnostics(device.page)).acceptedWrites;

  await device.page.evaluate(() => window.rational.setOnline(false));
  await expect(device.page.getByTestId("offline-banner")).toBeVisible();
  await device.page.getByRole("button", { name: "New transaction" }).click();
  const editor = device.page.getByRole("form", { name: "Transaction editor" });
  await editor.getByLabel("Account").selectOption({ label: "Wallet" });
  await editor.getByLabel("Amount (USD)").fill("-3.50");
  await editor.getByLabel("Description").fill("Bus ticket while offline");
  await editor.getByRole("button", { name: "Save transaction" }).click();
  await expect(
    device.page.locator('tr[data-description="Bus ticket while offline"]'),
  ).toBeVisible();
  await expect(device.page.getByTestId("pending-writes")).toContainText("1 change waiting");
  expect((await diagnostics(device.page)).acceptedWrites).toBe(accepted);

  await device.page.evaluate(() => window.rational.setOnline(true));
  await expect
    .poll(async () => (await diagnostics(device.page)).acceptedWrites, { timeout: 60_000 })
    .toBe(accepted + 1);

  // The other member receives it.
  const other = await openDevice(tenant.editor);
  await waitForHousehold(other.page);
  await other.page.goto(`${tenant.appUrl}/#/transactions`);
  await waitForHousehold(other.page);
  await expect(other.page.locator('tr[data-description="Bus ticket while offline"]')).toBeVisible({
    timeout: 60_000,
  });
});

test("conflicting edits on two devices resolve to the same state everywhere", async () => {
  const owner = await openDevice(tenant.owner);
  const editor = await openDevice(tenant.editor);
  await waitForHousehold(owner.page);
  await waitForHousehold(editor.page);
  await owner.page.evaluate(() => window.rational.waitForSync());
  await editor.page.evaluate(() => window.rational.waitForSync());
  const id = "txn_demo_0001";
  const stamp = Date.now();

  // The owner edits offline with an older stamp; the editor edits online with a newer one.
  await owner.page.evaluate(() => window.rational.setOnline(false));
  await owner.page.evaluate(
    ([documentId, at]) =>
      window.rational.writes?.updateTransaction(
        documentId as string,
        { description: "Owner's offline edit" },
        at as number,
      ),
    [id, stamp],
  );
  await editor.page.evaluate(
    ([documentId, at]) =>
      window.rational.writes?.updateTransaction(
        documentId as string,
        { description: "Editor's newer edit" },
        at as number,
      ),
    [id, stamp + 1_000],
  );
  await editor.page.evaluate(() => window.rational.waitForSync());
  await owner.page.evaluate(() => window.rational.setOnline(true));

  const descriptionOn = (page: Page) =>
    page.evaluate(
      async (documentId) =>
        (
          await window.rational.household?.session?.collections.transactions
            ?.findOne(documentId)
            .exec()
        )?.toJSON().description,
      id,
    );
  await expect
    .poll(() => descriptionOn(owner.page), { timeout: 60_000 })
    .toBe("Editor's newer edit");
  await expect
    .poll(() => descriptionOn(editor.page), { timeout: 60_000 })
    .toBe("Editor's newer edit");
  const ownerDiagnostics = await diagnostics(owner.page);
  expect(ownerDiagnostics.conflicts + ownerDiagnostics.conflictResponses).toBeGreaterThan(0);
});

test("an offline restart shows the household from local storage", async () => {
  const first = await openDevice(tenant.owner);
  await waitForHousehold(first.page);
  await first.page.evaluate(() => window.rational.waitForSync());
  await expect
    .poll(() => transactionCount(first.page), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(200);
  const directory = first.directory;
  await first.context.close();
  devices.splice(devices.indexOf(first), 1);

  // The same profile reopens with the network switched off before the first request.
  const restarted = await openDevice(tenant.owner, { startOffline: true, directory });
  await waitForHousehold(restarted.page);
  await expect(restarted.page.getByTestId("offline-banner")).toBeVisible();
  await expect
    .poll(() => transactionCount(restarted.page), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(200);
  const offline = await diagnostics(restarted.page);
  expect(offline.pullRequests).toBe(0);
  expect(offline.received).toBe(0);

  await restarted.page.evaluate(() => window.rational.setOnline(true));
  await expect
    .poll(async () => (await diagnostics(restarted.page)).pullRequests, { timeout: 60_000 })
    .toBeGreaterThan(0);
  await restarted.page.evaluate(() => window.rational.waitForSync());
  expect((await diagnostics(restarted.page)).received).toBe(0);
});
