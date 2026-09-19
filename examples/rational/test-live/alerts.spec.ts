import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { type BrowserContext, chromium, expect, type Page, test } from "@playwright/test";

import { type LiveTenant, tenantFile } from "./global-setup.js";

/**
 * Alerts end to end: the household asks, the server decides, and two things
 * happen — the alert replicates into the app, and the household's webhook
 * endpoint receives it, signed.
 *
 * This is the one story that cannot be told against a fake. The deciding is a
 * scheduled function running under a service credential; the delivery is the
 * control plane reading the environment's committed change log and signing a
 * POST. Both are real here, and the receiver in the global setup verifies the
 * signature the way the User Book (database webhooks) tells a receiver to.
 *
 * It runs only with a container runtime for the pinned edge runtime
 * (`MAKO_RUN_EDGE_RUNTIME_TESTS=1`); without one there is no nightly function
 * to invoke.
 */
const tenant = JSON.parse(readFileSync(tenantFile, "utf8")) as LiveTenant;

interface Delivery {
  readonly event: string | null;
  readonly deliveryId: string | null;
  readonly userAgent: string | null;
  readonly signatureValid: boolean;
  readonly body: {
    readonly collection?: string;
    readonly documentId?: string;
    readonly projectId?: string;
    readonly environmentId?: string;
  } | null;
}

function deliveries(): readonly Delivery[] {
  return readFileSync(tenant.webhookDeliveriesFile, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as Delivery);
}

const devices: { context: BrowserContext; page: Page; directory: string }[] = [];

async function openDevice(user: { email: string; password: string }): Promise<Page> {
  const directory = mkdtempSync(join(tmpdir(), "rational-alerts-device-"));
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
  tenant.functionsEndpoint === null,
  "the nightly function is not deployed: no container runtime for the pinned edge runtime, or the deployment failed — the setup prints which on stderr",
);

test.afterEach(async () => {
  for (const device of devices.splice(0)) {
    await device.context.close().catch(() => undefined);
    rmSync(device.directory, { recursive: true, force: true });
  }
});

test("an alert the nightly job decided reaches the app and the household's endpoint, signed", async () => {
  const page = await openDevice(tenant.owner);
  const householdId = await page.evaluate(() => window.rational.createHousehold("Alerting", "USD"));
  await page.waitForFunction((id) => window.rational.state.currentHouseholdId === id, householdId, {
    timeout: 60_000,
  });
  await page.waitForFunction(() => window.rational.writes !== null, undefined, { timeout: 60_000 });

  // What the household wants to hear about, and one charge that qualifies.
  const transactionId = await page.evaluate(async () => {
    const writes = window.rational.writes;
    if (writes === null) throw new Error("no household is open");
    await writes.saveAlertSetting({
      alert_kind: "large_transaction",
      threshold: 40_000,
      enabled: true,
    });
    const account = await writes.createAccount({
      name: "Everyday",
      type: "checking",
      currency: "USD",
      opening_balance: 500_000,
      opening_date: "2026-01-01",
    });
    const transaction = await writes.createTransaction({
      account_id: account.id,
      date: "2026-08-29",
      amount: -50_000,
      currency: "USD",
      description: "ROOF REPAIR",
      tags: [],
      splits: [],
    });
    return String(transaction.id);
  });
  await page.evaluate(() => window.rational.waitForSync());

  // The night runs -- started the way the platform starts it, through the
  // schedule, so what is exercised is the real path: the scheduler's internal
  // hop, the run key its schedule carries, and the function's own refusal to
  // run for anybody who does not have it.
  const nightlyUrl = `/${tenant.projectId}--${tenant.environmentId}/functions/v1/nightly`;
  const anonymous = await page.evaluate(
    async (url) => (await fetch(url, { method: "POST" })).status,
    nightlyUrl,
  );
  expect(anonymous, "a function's route is not open to an unauthenticated caller").toBe(401);

  // `schedules list --json` prints the schedules themselves; `schedules runs`
  // prints a page. Both shapes are the CLI's, and both are read as they are.
  const schedules = mako(["schedules", "list", "--function", "nightly"]) as {
    id: string;
    name: string;
  }[];
  const nightly = schedules.find((entry) => entry.name === "nightly");
  expect(nightly, "the bootstrap did not create the nightly schedule").toBeDefined();
  mako(["schedules", "run-now", nightly?.id ?? "", "--function", "nightly"]);
  // The run's own record says what happened; without checking it, a function
  // that refused or crashed looks exactly like one that found nothing to do.
  await expect
    .poll(() => finished(nightly?.id ?? "").length, { timeout: 90_000 })
    .toBeGreaterThanOrEqual(1);
  const [first] = finished(nightly?.id ?? "");
  expect(first?.outcome, `the run did not succeed: ${JSON.stringify(first)}`).toBe("succeeded");

  // The alert replicates into the app like every other document.
  const alertId = `alr_${householdId}.large.${transactionId}`;
  await expect
    .poll(
      () =>
        page.evaluate(async () => {
          const collection = window.rational.household?.session?.collections.alerts;
          if (collection === undefined) return "the alerts collection is not open";
          const documents = await collection.find().exec();
          return documents
            .map((document) => `${document.toJSON().id}/${String(document.toJSON().kind)}`)
            .join(" ");
        }),
      { timeout: 90_000, message: "the nightly job's alert never arrived" },
    )
    .toContain(`${alertId}/alert`);
  await page
    .getByRole("navigation", { name: "Sections" })
    .getByRole("link", { name: "Settings" })
    .click();
  await page
    .getByRole("navigation", { name: "Settings pages" })
    .getByRole("link", { name: "Notifications" })
    .click();
  await expect(page.getByTestId("alerts-screen")).toBeVisible();
  const row = page.getByTestId(`alert-${alertId}`);
  await expect(row.getByTestId("alert-kind")).toHaveText("A large transaction");
  await expect(row.getByTestId("alert-message")).toContainText("ROOF REPAIR");

  // And the household's endpoint received it: exactly one delivery for that
  // document, signed with the secret the registration handed over.
  await expect
    .poll(() => deliveries().filter((entry) => entry.body?.documentId === alertId).length, {
      timeout: 60_000,
      message: "the alert was never delivered to the household's webhook endpoint",
    })
    .toBe(1);
  const [delivered] = deliveries().filter((entry) => entry.body?.documentId === alertId);
  expect(delivered?.signatureValid, "the delivery's signature did not verify").toBe(true);
  expect(delivered?.event).toBe("insert");
  expect(delivered?.userAgent).toBe("mako-cloud-webhooks/1");
  expect(delivered?.body?.collection).toBe("alerts");
  expect(delivered?.body?.projectId).toBe(tenant.projectId);
  expect(delivered?.body?.environmentId).toBe(tenant.environmentId);

  // A delivery says what changed, never what it contains: nothing a member
  // could not read leaves the environment on this channel.
  // (The document id is necessarily in it -- that is what a delivery names.)
  const payload = JSON.stringify(delivered?.body ?? {});
  expect(payload).not.toContain("ROOF REPAIR");
  expect(payload).not.toContain("50000");
  expect(payload).not.toContain("Everyday");

  // Running the night again does not fire, or deliver, the same alert twice.
  mako(["schedules", "run-now", nightly?.id ?? "", "--function", "nightly"]);
  await expect.poll(() => finished(nightly?.id ?? "").length, { timeout: 90_000 }).toBe(2);
  expect(deliveries().filter((entry) => entry.body?.documentId === alertId)).toHaveLength(1);
});

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
  readonly error?: string | null;
}

/** Runs that are over, whatever they concluded. */
function finished(scheduleId: string): readonly ScheduleRun[] {
  if (scheduleId === "") return [];
  const listed = mako(["schedules", "runs", scheduleId, "--function", "nightly"]) as {
    items?: ScheduleRun[];
  };
  return (listed.items ?? []).filter((run) => run.outcome !== null);
}
