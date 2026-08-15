import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const exampleRoot = resolve(here, "..");
const repositoryRoot = resolve(exampleRoot, "../..");

export const tenantFile = join(exampleRoot, "test-live", ".live-tenant.json");

export interface LiveTenant {
  readonly appUrl: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly publicProjectKey: string;
}

function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        reject(new Error("could not allocate a port"));
        return;
      }
      const { port } = address;
      server.close(() => resolvePort(port));
    });
  });
}

async function waitFor(probe: () => Promise<boolean>, what: string, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await probe().catch(() => false)) return;
    await new Promise((wait) => setTimeout(wait, 200));
  }
  throw new Error(`timed out waiting for ${what}`);
}

const processes: ChildProcess[] = [];
let workspace: string | undefined;

export default async function globalSetup(): Promise<void> {
  const binaries = process.env.MAKO_SMOKE_BINARY_DIR ?? join(repositoryRoot, "target", "debug");
  workspace = mkdtempSync(join(process.env.MAKO_STORAGE_TMPDIR ?? tmpdir(), "mako-live-"));

  const path = (segment: string) => join(workspace as string, segment);
  const environment = {
    ...process.env,
    MAKO_ENVIRONMENT: "local",
    MAKO_REGION: "local",
    MAKO_ROCKSDB_PATH: path("rocksdb"),
    MAKO_ROCKSDB_BACKUP_DESTINATION: path("backups"),
    MAKO_CONTROL_SQLITE_PATH: path("control/control.sqlite3"),
    MAKO_CONTROL_SQLITE_LOCK_PATH: path("control/control.sqlite3.lock"),
    MAKO_CONTROL_SQLITE_IDENTITY: "mako-control-live",
    MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE: path("control/migration"),
    MAKO_CONTROL_SQLITE_BACKUP_STAGING: path("control/staging"),
    MAKO_CONTROL_SQLITE_BACKUP_PUBLISH: path("control/published"),
    MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE: path("control/restore"),
    MAKO_CONTROL_SQLITE_RESERVE_PATH: path("control/reserve"),
    MAKO_INTERNAL_AUTH_SECRET: "live-example-internal-auth-secret-0123456789",
    MAKO_INTERNAL_AUTH_SECRET_REF: "env:MAKO_INTERNAL_AUTH_SECRET",
    MAKO_OBJECT_STORE_ACCESS_KEY: "live-example-object-store-access-key",
    MAKO_OBJECT_STORE_ACCESS_KEY_REF: "env:MAKO_OBJECT_STORE_ACCESS_KEY",
    MAKO_OBJECT_STORE_SECRET_KEY: "live-example-object-store-secret-key-0123",
    MAKO_OBJECT_STORE_SECRET_KEY_REF: "env:MAKO_OBJECT_STORE_SECRET_KEY",
  };

  // Seed the tenant while nothing holds the database locks.
  const bootstrap = spawnSync(join(binaries, "mako-local-bootstrap"), { env: environment });
  if (bootstrap.status !== 0) {
    throw new Error(
      `bootstrap failed: ${bootstrap.stderr?.toString() ?? "no output"}. Build the workspace ` +
        "binaries first (cargo build --workspace --bins) or set MAKO_SMOKE_BINARY_DIR.",
    );
  }
  const tenant = JSON.parse(bootstrap.stdout.toString()) as {
    projectId: string;
    environmentId: string;
    collectionId: string;
    publicProjectKey: string;
  };

  const dataPort = await freePort();
  const dataPlane = spawn(join(binaries, "mako-data-plane"), {
    env: { ...environment, MAKO_BIND_ADDR: `127.0.0.1:${dataPort}` },
    stdio: "ignore",
  });
  processes.push(dataPlane);
  await waitFor(
    async () => (await fetch(`http://127.0.0.1:${dataPort}/readyz`)).status === 200,
    "the data plane to become ready",
  );

  // Serve the application from one origin that also proxies the API, matching
  // how a deployment fronts both behind a reverse proxy.
  const appPort = await freePort();
  const vite = spawn(
    "npx",
    ["vite", "--host", "127.0.0.1", "--port", String(appPort), "--strictPort"],
    {
      cwd: exampleRoot,
      env: { ...process.env, MAKO_LIVE_ENDPOINT: `http://127.0.0.1:${dataPort}` },
      stdio: "ignore",
    },
  );
  processes.push(vite);
  const appUrl = `http://127.0.0.1:${appPort}`;
  await waitFor(async () => (await fetch(appUrl)).ok, "the application to be served");

  const live: LiveTenant = { appUrl, ...tenant };
  writeFileSync(tenantFile, JSON.stringify(live, null, 2));
}

export async function globalTeardown(): Promise<void> {
  for (const child of processes) child.kill();
  if (workspace !== undefined) rmSync(workspace, { recursive: true, force: true });
  rmSync(tenantFile, { force: true });
}
