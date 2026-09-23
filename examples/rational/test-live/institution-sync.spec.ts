import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * The simulated institution, synced the way the platform syncs it: an
 * app-created connection, the schedule driving /sync, twice. This spec exists
 * because /sync had no end-to-end coverage at all -- the wire-mocked fake
 * does not enforce indexes, and the beta showed what that hid: the sync's
 * own query (`kind eq institution`) had no covering index, so every hosted
 * run since the first real connection answered 500 (finding #42). If a model
 * change ever strands this function's queries again, this is the test that
 * says so before a deployment does.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

const devices: { context: BrowserContext; page: Page; directory: string }[] = [];

async function openDevice(user: { email: string; password: string }): Promise<Page> {
  const directory = mkdtempSync(join(tmpdir(), "rational-syncrepro-"));
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

test.skip(tenant.functionsEndpoint === null, "the functions are not deployed");

test.afterEach(async () => {
  for (const device of devices.splice(0)) {
    await device.context.close().catch(() => undefined);
    rmSync(device.directory, { recursive: true, force: true });
  }
});

test("a simulated connection syncs through the schedule, twice", async () => {
  test.setTimeout(240_000);
  const page = await openDevice(tenant.owner);
  const householdId = await page.evaluate(() => window.rational.createHousehold("Synced", "USD"));
  await page.waitForFunction((id) => window.rational.state.currentHouseholdId === id, householdId, {
    timeout: 60_000,
  });
  await page.waitForFunction(() => window.rational.writes !== null, undefined, { timeout: 60_000 });
  await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 500_000,
      opening_date: "2026-01-01",
    });
    await writes.connectInstitution({
      account_id: String(account.id),
      institution: "Simulated Bank",
      external_id: "acct-1",
    });
  });
  await page.evaluate(() => window.rational.waitForSync());

  const schedules = makoCloud(["schedules", "list", "--function", "institution-sync"]) as {
    id: string;
    name: string;
  }[];
  const schedule = schedules.find((entry) => entry.name === "every-fifteen-minutes");
  expect(schedule).toBeDefined();
  for (const round of [1, 2]) {
    makoCloud(["schedules", "run-now", schedule?.id ?? "", "--function", "institution-sync"]);
    await expect
      .poll(() => finished(schedule?.id ?? "").length, { timeout: 90_000 })
      .toBeGreaterThanOrEqual(round);
    const runs = finished(schedule?.id ?? "");
    const run = runs[runs.length - round] ?? runs[0];
    if (run?.outcome !== "succeeded") {
      const outcomes = await page.evaluate(async () => {
        const collection = window.rational.household?.session?.collections.connections;
        if (collection === undefined) return "no connections collection";
        const documents = await collection.find().exec();
        return JSON.stringify(documents.map((entry) => entry.toJSON()));
      });
      const logs = spawnSync(
        join(process.cwd(), "..", "..", "node_modules", ".bin", "mako-cloud"),
        ["functions", "logs", "institution-sync", "--all"],
        {
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
        },
      ).stdout;
      throw new Error(
        `round ${round} failed: ${JSON.stringify(run)}\nconnections: ${outcomes}\nlogs:\n${logs}`,
      );
    }
  }
});

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

interface ScheduleRun {
  readonly outcome: string | null;
  readonly responseStatus: number | null;
}

function finished(scheduleId: string): readonly ScheduleRun[] {
  if (scheduleId === "") return [];
  const listed = makoCloud(["schedules", "runs", scheduleId, "--function", "institution-sync"]) as {
    items?: ScheduleRun[];
  };
  return (listed.items ?? []).filter((run) => run.outcome !== null);
}
