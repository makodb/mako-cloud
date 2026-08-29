import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * The `households` edge function against the real stack: the gateway routes
 * the invocation, the function writes the claim with its service credential,
 * and the data plane's own policies decide what the new member may do. It
 * runs only when the suite was started with a container runtime for the
 * pinned edge runtime (`MAKO_RUN_EDGE_RUNTIME_TESTS=1`); without one there is
 * no function to invoke and the story is covered by the wire-mocked suite.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;
const MEMBER = { email: "joiner@rational.test", password: "RationalDemo1!" };

interface Device {
  readonly context: BrowserContext;
  readonly page: Page;
  readonly directory: string;
}

const devices: Device[] = [];

async function openDevice(user: { email: string; password: string }, register = false) {
  const directory = mkdtempSync(join(tmpdir(), "rational-hh-device-"));
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
  await page.goto(tenant.appUrl);
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  if (register) {
    await page.evaluate(
      async (credentials) => {
        await window.rational
          .signUp(credentials.email, credentials.password)
          .catch(() => undefined);
        await window.rational.signIn(credentials.email, credentials.password);
      },
      { email: user.email, password: user.password },
    );
  } else {
    await page.getByLabel("Email").fill(user.email);
    await page.getByLabel("Password").fill(user.password);
    await page.getByRole("button", { name: "Sign in" }).click();
  }
  await page.waitForFunction(() => window.rational.state.phase === "ready", undefined, {
    timeout: 60_000,
  });
  const device = { context, page, directory };
  devices.push(device);
  return device;
}

const diagnostics = (page: Page) => page.evaluate(() => window.rational.diagnostics());

async function settled(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 60_000 },
  );
}

async function accountCount(page: Page): Promise<number> {
  return page.evaluate(async () => {
    const collection = window.rational.household?.session?.collections.accounts;
    if (collection === undefined) return -1;
    return (await collection.find().exec()).length;
  });
}

test.skip(
  tenant.functionsEndpoint === null,
  "the households function is not deployed: no container runtime for the pinned edge runtime, or the deployment failed — the setup prints which on stderr",
);

test.afterEach(async () => {
  for (const device of devices.splice(0)) {
    await device.context.close().catch(() => undefined);
    rmSync(device.directory, { recursive: true, force: true });
  }
});

test("an invitation is accepted, replicates, and a removal clears the member's copy", async () => {
  const owner = await openDevice(tenant.owner);
  await settled(owner.page);

  // A household created through the function, with something in it to share.
  const householdId = await owner.page.evaluate(() =>
    window.rational.createHousehold("Beach house", "USD"),
  );
  expect(householdId).toMatch(/^hh_/u);
  await owner.page.waitForFunction(
    (id) => window.rational.state.currentHouseholdId === id,
    householdId,
    { timeout: 60_000 },
  );
  await owner.page.evaluate(() =>
    window.rational.writes?.createAccount({
      name: "Beach chequing",
      type: "checking",
      currency: "USD",
      opening_balance: 100_000,
      opening_date: "2026-08-01",
    }),
  );
  await owner.page.evaluate(() => window.rational.waitForSync());

  await owner.page.evaluate(
    ([id, email]) => window.rational.inviteMember(id as string, email as string, "viewer"),
    [householdId, MEMBER.email],
  );

  // The invited person signs up and finds the invitation addressed to them.
  const member = await openDevice(MEMBER, true);
  await settled(member.page);
  await member.page.waitForFunction(
    (id) => window.rational.state.invitations.some((entry) => entry.household_id === id),
    householdId,
    { timeout: 60_000 },
  );
  await member.page.evaluate((id) => window.rational.acceptInvitation(id), householdId);
  await member.page.waitForFunction(
    (id) => window.rational.state.currentHouseholdId === id,
    householdId,
    { timeout: 60_000 },
  );

  // The household's data replicates to the new member.
  await expect.poll(() => accountCount(member.page), { timeout: 60_000 }).toBeGreaterThanOrEqual(1);

  // A viewer's write is refused by the document policy.
  const before = (await diagnostics(member.page)).deniedWrites;
  await member.page.evaluate(async () => {
    const accounts = window.rational.household?.session?.collections.accounts;
    const documents = accounts === undefined ? [] : await accounts.find().exec();
    await window.rational.writes?.createTransaction({
      account_id: documents[0]?.toJSON().id ?? "",
      date: "2026-08-20",
      amount: -1_000,
      currency: "USD",
      description: "A viewer should not be able to write this",
    });
  });
  await expect
    .poll(async () => (await diagnostics(member.page)).deniedWrites, { timeout: 60_000 })
    .toBeGreaterThan(before);

  // Promoted to editor, the same write is accepted.
  const memberId = await owner.page.evaluate(
    async (email) =>
      (
        await (window.rational.directory?.session?.collections.memberships?.find().exec() ??
          Promise.resolve([]))
      )
        .map((document) => document.toJSON())
        .find((membership) => membership.email === email && membership.status === "active")
        ?.user_id ?? null,
    MEMBER.email,
  );
  expect(memberId).not.toBeNull();
  await owner.page.evaluate(
    ([id, userId]) => window.rational.changeMemberRole(id as string, userId as string, "editor"),
    [householdId, memberId as string],
  );
  await member.page.waitForFunction(
    (id) =>
      window.rational.state.memberships.some(
        (entry) => entry.household_id === id && entry.role === "editor",
      ),
    householdId,
    { timeout: 60_000 },
  );
  await member.page.waitForFunction(() => window.rational.household?.session !== null, undefined, {
    timeout: 60_000,
  });
  const accepted = (await diagnostics(member.page)).acceptedWrites;
  await member.page.evaluate(async () => {
    const accounts = window.rational.household?.session?.collections.accounts;
    const documents = accounts === undefined ? [] : await accounts.find().exec();
    await window.rational.writes?.createTransaction({
      account_id: documents[0]?.toJSON().id ?? "",
      date: "2026-08-21",
      amount: -2_500,
      currency: "USD",
      description: "An editor may write this",
    });
  });
  await expect
    .poll(async () => (await diagnostics(member.page)).acceptedWrites, { timeout: 60_000 })
    .toBeGreaterThan(accepted);

  // Removal clears the removed member's copy of the household.
  await owner.page.evaluate(
    ([id, userId]) => window.rational.removeMember(id as string, userId as string),
    [householdId, memberId as string],
  );
  await member.page.waitForFunction(
    (id) => !window.rational.state.memberships.some((entry) => entry.household_id === id),
    householdId,
    { timeout: 90_000 },
  );
  await expect
    .poll(
      async () =>
        member.page.evaluate(async (id) => {
          const databases = await indexedDB.databases();
          const name = `rational-${id.toLowerCase()}`;
          return databases.some((database) => database.name === name);
        }, householdId),
      { timeout: 90_000 },
    )
    .toBe(false);
  // The owner still has it.
  await expect.poll(() => accountCount(owner.page), { timeout: 60_000 }).toBeGreaterThanOrEqual(1);
});
