import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const exampleRoot = resolve(here, "..");
const repositoryRoot = resolve(exampleRoot, "../..");

export const tenantFile = join(exampleRoot, "test-live", ".live-tenant.json");

/** Shared with the smoke harness: 64 hex characters derive the developer session key too. */
const INTERNAL_AUTH_SECRET = "5f4e3d2c1b0a998877665544332211000112233445566778899aabbccddeeff0";
const DEVELOPER_ID = "dev_localboot";
const DEVELOPER_EMAIL = "developer@local.test";

export interface LiveTenant {
  readonly appUrl: string;
  readonly dataEndpoint: string;
  readonly managementEndpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly publicProjectKey: string;
  readonly householdId: string;
  readonly owner: { readonly email: string; readonly password: string; readonly userId: string };
  readonly editor: { readonly email: string; readonly password: string; readonly userId: string };
  /** The edge gateway serving the `households` function, or null when none runs. */
  readonly functionsEndpoint: string | null;
}

/**
 * The hosted function path needs a container runtime holding the pinned edge
 * runtime, and the gateway compiles in the ports it talks to, so it can only
 * run on 8080/8081/8082/9000 and never beside another stack. It is opt-in for
 * exactly that reason; without it the suite runs everything that does not
 * involve a function and the households spec skips.
 */
const RUN_FUNCTIONS = process.env.MAKO_RUN_EDGE_RUNTIME_TESTS === "1";
const DATA_PLANE_PORT = 8080;
const CONTROL_PLANE_PORT = 8081;
const GATEWAY_PORT = 8082;
const SUPERVISOR_PORT = 9000;
const CONTAINER_NAME = "mako-rational-edge-runtime";
const OBJECT_STORE_CONTAINER = "mako-rational-object-store";
const OBJECT_STORE_IMAGE = "docker.io/chrislusf/seaweedfs:4.29";
const OBJECT_STORE_PORT = 8333;
const OBJECT_STORE_ACCESS_KEY = "rational-live-object-store-access-key";
const OBJECT_STORE_SECRET_KEY = "rational-live-object-store-secret-key-0123";
const RUNTIME_STATE_KEY = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

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

async function waitFor(probe: () => Promise<boolean>, what: string, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await probe().catch(() => false)) return;
    await new Promise((wait) => setTimeout(wait, 250));
  }
  throw new Error(`timed out waiting for ${what}`);
}

const processes: ChildProcess[] = [];
let workspace: string | undefined;
let runtimeContainer: string | null = null;
let objectStoreContainer: string | null = null;

/** The pinned edge-runtime image the platform deploys functions onto. */
function pinnedRuntimeImage(): string {
  const pin = JSON.parse(
    readFileSync(join(repositoryRoot, "infra/edge-runtime/runtime-pin.json"), "utf8"),
  ) as { imageRepository: string; imageDigest: string };
  return `${pin.imageRepository}@${pin.imageDigest}`;
}

function containerEngine(): string[] {
  const engine = process.env.MAKO_EDGE_TEST_ENGINE ?? "podman";
  const prefix = process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON;
  return prefix === undefined ? [engine] : [engine, ...(JSON.parse(prefix) as string[])];
}

/**
 * Start the runtime supervisor the control plane registers deployments with.
 * This is the same contract the deployed quadlet uses: the internal auth
 * secret the services share, the region they agree on, and the CLI's `main`
 * worker mounted read-only.
 */
async function startRuntimeContainer(state: string, canary: string): Promise<boolean> {
  const engine = containerEngine();
  mkdirSync(state, { recursive: true });
  // The main worker serves both the supervisor and `mako functions serve`, so
  // it insists on a function to serve even when only the supervisor is used.
  mkdirSync(canary, { recursive: true });
  writeFileSync(
    join(canary, "index.ts"),
    "export default { fetch: () => new Response('{\"ok\":true}') };\n",
  );
  const environment: Record<string, string> = {
    DENO_DIR: "/tmp/deno-cache",
    EDGE_RUNTIME_PORT: String(SUPERVISOR_PORT),
    MAKO_API_URL: `http://host.containers.internal:${DATA_PLANE_PORT}`,
    MAKO_ENTRYPOINT: "index.ts",
    MAKO_ENVIRONMENT_ID: "env_rationalcanary",
    MAKO_FUNCTION_NAME: "canary",
    MAKO_FUNCTION_PATH: "/home/deno/functions/canary",
    MAKO_JWKS: "",
    MAKO_JWT_AUDIENCE: "",
    MAKO_JWT_ISSUER: "",
    MAKO_PROJECT_ID: "prj_rationalcanary",
    MAKO_RUNTIME_AUTHORIZATION: INTERNAL_AUTH_SECRET,
    MAKO_RUNTIME_REGION: "local",
    MAKO_RUNTIME_STATE_PATH: "/var/lib/mako-runtime-supervisor",
    MAKO_RUNTIME_STATE_KEY: RUNTIME_STATE_KEY,
    MAKO_RUNTIME_WORKER_PATH: "/var/lib/mako-runtime-workers",
    MAKO_USER_ENV_NAMES: "[]",
    MAKO_VERIFY_JWT: "false",
    MAKO_WALL_TIME_MS: "5000",
  };
  spawnSync(engine[0] as string, [...engine.slice(1), "rm", "-f", CONTAINER_NAME], {
    stdio: "ignore",
  });
  const started = spawnSync(
    engine[0] as string,
    [
      ...engine.slice(1),
      "run",
      "--detach",
      "--rm",
      "--name",
      CONTAINER_NAME,
      "--init",
      "--read-only",
      "--publish",
      `127.0.0.1:${SUPERVISOR_PORT}:${SUPERVISOR_PORT}`,
      "--mount",
      `type=bind,src=${join(repositoryRoot, "packages/cli/runtime/main")},dst=/home/deno/functions/main,readonly`,
      "--mount",
      `type=bind,src=${canary},dst=/home/deno/functions/canary,readonly`,
      "--mount",
      `type=bind,src=${state},dst=/var/lib/mako-runtime-supervisor`,
      "--tmpfs",
      "/tmp:rw,noexec,nosuid,size=128m",
      "--tmpfs",
      "/var/lib/mako-runtime-workers:rw,noexec,nosuid,size=128m",
      ...Object.entries(environment).flatMap(([name, value]) => ["--env", `${name}=${value}`]),
      pinnedRuntimeImage(),
      "start",
      "--policy",
      "per_request",
      "--user-worker-request-idle-timeout",
      "5000",
      "--main-service",
      "/home/deno/functions/main",
    ],
    { encoding: "utf8" },
  );
  if (started.status !== 0) {
    console.warn(
      `rational live: the pinned edge runtime did not start; functions are skipped.\n${started.stderr}`,
    );
    return false;
  }
  runtimeContainer = CONTAINER_NAME;
  try {
    await waitFor(
      async () => {
        const response = await fetch(
          `http://127.0.0.1:${SUPERVISOR_PORT}/_mako/runtime/v1/health`,
          {
            headers: {
              "x-mako-runtime-authorization": INTERNAL_AUTH_SECRET,
              "x-mako-runtime-protocol": "1",
              "x-mako-request-id": "req_rationallive0000000000000000",
            },
          },
        );
        return response.status === 200 && (await response.text()).includes('"ready":true');
      },
      "the runtime supervisor to become ready",
      120_000,
    );
  } catch (error) {
    console.warn(`rational live: ${String(error)}; functions are skipped.`);
    return false;
  }
  return true;
}

/**
 * A function bundle is uploaded to the environment's object store, so the
 * management path needs one running — the in-memory store `mako-local-bootstrap`
 * uses is not on this path. The local compose serves it on 8333; when nothing
 * answers there, this starts the same image for the run.
 */
async function startObjectStore(configPath: string): Promise<boolean> {
  const answers = async () =>
    (await fetch(`http://127.0.0.1:${OBJECT_STORE_PORT}/`).then(
      () => true,
      () => false,
    )) as boolean;
  if (await answers()) return true;
  // The services sign their S3 requests with the configured key, and a store
  // with no identity configured refuses an unknown one — so the identity is
  // written here rather than assumed.
  writeFileSync(
    configPath,
    JSON.stringify({
      identities: [
        {
          name: "mako",
          credentials: [
            {
              accessKey: OBJECT_STORE_ACCESS_KEY,
              secretKey: OBJECT_STORE_SECRET_KEY,
            },
          ],
          actions: ["Admin", "Read", "Write", "List", "Tagging"],
        },
      ],
    }),
  );
  const engine = containerEngine();
  spawnSync(engine[0] as string, [...engine.slice(1), "rm", "-f", OBJECT_STORE_CONTAINER], {
    stdio: "ignore",
  });
  const started = spawnSync(
    engine[0] as string,
    [
      ...engine.slice(1),
      "run",
      "--detach",
      "--rm",
      "--name",
      OBJECT_STORE_CONTAINER,
      "--publish",
      `127.0.0.1:${OBJECT_STORE_PORT}:${OBJECT_STORE_PORT}`,
      "--mount",
      `type=bind,src=${configPath},dst=/etc/mako-s3.json,readonly`,
      OBJECT_STORE_IMAGE,
      "server",
      "-s3",
      "-s3.config=/etc/mako-s3.json",
      "-dir=/data",
    ],
    { encoding: "utf8" },
  );
  if (started.status !== 0) {
    console.warn(
      `rational live: the object store did not start; functions are skipped.\n${started.stderr}`,
    );
    return false;
  }
  objectStoreContainer = OBJECT_STORE_CONTAINER;
  try {
    await waitFor(answers, "the object store to answer", 120_000);
  } catch (error) {
    console.warn(`rational live: ${String(error)}; functions are skipped.`);
    return false;
  }
  return true;
}

async function portIsFree(port: number): Promise<boolean> {
  return new Promise((done) => {
    const server = createServer();
    server.once("error", () => done(false));
    server.listen(port, "127.0.0.1", () => server.close(() => done(true)));
  });
}

/**
 * Boot a throwaway stack the way the smoke harness does — bootstrap the
 * stores while nothing holds them, start the data plane and the control
 * plane, mint a developer session — then run Rational's own bootstrap and
 * seed scripts against it and serve the app same-origin through Vite.
 */
export default async function globalSetup(): Promise<void> {
  const binaries = process.env.MAKO_SMOKE_BINARY_DIR ?? join(repositoryRoot, "target", "debug");
  workspace = mkdtempSync(join(process.env.MAKO_STORAGE_TMPDIR ?? tmpdir(), "rational-live-"));
  const path = (segment: string) => join(workspace as string, segment);
  for (const directory of [
    "rocksdb",
    "control",
    "control/migration",
    "control/staging",
    "control/published",
    "control/restore",
    "control/reserve",
    "backups",
  ]) {
    mkdirSync(path(directory), { recursive: true });
  }
  const environment: Record<string, string | undefined> = {
    ...process.env,
    MAKO_ENVIRONMENT: "local",
    MAKO_REGION: "local",
    MAKO_ROCKSDB_PATH: path("rocksdb"),
    MAKO_ROCKSDB_BACKUP_DESTINATION: path("backups"),
    MAKO_CONTROL_SQLITE_PATH: path("control/control.sqlite3"),
    MAKO_CONTROL_SQLITE_LOCK_PATH: path("control/control.sqlite3.lock"),
    MAKO_CONTROL_SQLITE_IDENTITY: "mako-control-rational-live",
    MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE: path("control/migration"),
    MAKO_CONTROL_SQLITE_BACKUP_STAGING: path("control/staging"),
    MAKO_CONTROL_SQLITE_BACKUP_PUBLISH: path("control/published"),
    MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE: path("control/restore"),
    MAKO_CONTROL_SQLITE_RESERVE_PATH: path("control/reserve"),
    MAKO_INTERNAL_AUTH_SECRET: INTERNAL_AUTH_SECRET,
    MAKO_INTERNAL_AUTH_SECRET_REF: "env:MAKO_INTERNAL_AUTH_SECRET",
    MAKO_OBJECT_STORE_ACCESS_KEY: OBJECT_STORE_ACCESS_KEY,
    MAKO_OBJECT_STORE_ACCESS_KEY_REF: "env:MAKO_OBJECT_STORE_ACCESS_KEY",
    MAKO_OBJECT_STORE_SECRET_KEY: OBJECT_STORE_SECRET_KEY,
    MAKO_OBJECT_STORE_SECRET_KEY_REF: "env:MAKO_OBJECT_STORE_SECRET_KEY",
  };

  // The hosted function path needs the supervisor the control plane registers
  // deployments with, and the gateway compiles in the ports it uses.
  const functionsWanted =
    RUN_FUNCTIONS && (await freePorts()) && (await startObjectStore(path("object-store-s3.json")));
  const runtimeReady = functionsWanted
    ? await startRuntimeContainer(path("runtime-state"), path("runtime-canary"))
    : false;
  if (runtimeReady) {
    environment.MAKO_RUNTIME_SUPERVISOR_ENDPOINT = `127.0.0.1:${SUPERVISOR_PORT}`;
  }

  const bootstrap = spawnSync(join(binaries, "mako-local-bootstrap"), { env: environment });
  if (bootstrap.status !== 0) {
    throw new Error(
      `mako-local-bootstrap failed: ${bootstrap.stderr?.toString() ?? "no output"}. Build the ` +
        "workspace binaries first (cargo build --workspace --bins) or set MAKO_SMOKE_BINARY_DIR.",
    );
  }

  const [dataPort, controlPort] = runtimeReady
    ? [DATA_PLANE_PORT, CONTROL_PLANE_PORT]
    : [await freePort(), await freePort()];
  // Service output goes to files in the workspace so a failure can be read.
  const logFile = (name: string) => openSync(path(`${name}.log`), "w");
  const dataLog = logFile("data-plane");
  const dataPlane = spawn(join(binaries, "mako-data-plane"), {
    env: { ...environment, MAKO_BIND_ADDR: `127.0.0.1:${dataPort}` },
    stdio: ["ignore", dataLog, dataLog],
  });
  processes.push(dataPlane);
  await waitFor(
    async () => (await fetch(`http://127.0.0.1:${dataPort}/readyz`)).status === 200,
    "the data plane to become ready",
  );
  const controlLog = logFile("control-plane");
  const controlPlane = spawn(join(binaries, "mako-control-plane"), {
    env: {
      ...environment,
      MAKO_BIND_ADDR: `127.0.0.1:${controlPort}`,
      MAKO_DATA_PLANE_ENDPOINT: `127.0.0.1:${dataPort}`,
    },
    stdio: ["ignore", controlLog, controlLog],
  });
  processes.push(controlPlane);
  await waitFor(
    async () => (await fetch(`http://127.0.0.1:${controlPort}/readyz`)).status === 200,
    "the control plane to become ready",
  );

  // A developer session for the seeded developer, minted the way deployment
  // tooling does: the control plane never issues one over HTTP locally.
  const secretFile = path("internal-auth");
  writeFileSync(secretFile, INTERNAL_AUTH_SECRET);
  chmodSync(secretFile, 0o600);
  const sessionFile = path("developer-session.jwt");
  const minted = spawnSync(join(binaries, "mako-control-session"), [
    "--secret-file",
    secretFile,
    "--output",
    sessionFile,
    "--issuer",
    `http://127.0.0.1:${controlPort}/control-identity`,
    "--identity-id",
    DEVELOPER_ID,
    "--email",
    DEVELOPER_EMAIL,
    "--display-name",
    "Rational Live",
    "--credential-epoch",
    "1",
    "--authorization-epoch",
    "1",
    "--ttl-seconds",
    "3600",
  ]);
  if (minted.status !== 0) {
    throw new Error(`developer session was not issued: ${minted.stderr?.toString()}`);
  }
  const developerToken = readFileSync(sessionFile, "utf8").trim();

  // The gateway keeps its own storage; it cannot share the data plane's.
  let functionsEndpoint: string | null = null;
  if (runtimeReady) {
    for (const directory of ["gateway-rocksdb", "gateway-backups"]) {
      mkdirSync(path(directory), { recursive: true });
    }
    const gatewayLog = logFile("edge-gateway");
    const gateway = spawn(join(binaries, "mako-edge-gateway"), {
      env: {
        ...environment,
        MAKO_BIND_ADDR: `127.0.0.1:${GATEWAY_PORT}`,
        MAKO_ROCKSDB_PATH: path("gateway-rocksdb"),
        MAKO_ROCKSDB_BACKUP_DESTINATION: path("gateway-backups"),
      },
      stdio: ["ignore", gatewayLog, gatewayLog],
    });
    processes.push(gateway);
    try {
      await waitFor(
        async () => (await fetch(`http://127.0.0.1:${GATEWAY_PORT}/readyz`)).status === 200,
        "the edge gateway to become ready",
      );
      functionsEndpoint = `http://127.0.0.1:${GATEWAY_PORT}`;
    } catch (error) {
      console.warn(`rational live: ${String(error)}; functions are skipped.`);
    }
  }

  const dataEndpoint = `http://127.0.0.1:${dataPort}`;
  const managementEndpoint = `http://127.0.0.1:${controlPort}`;
  const envFile = path("mako.env.json");
  const rationalBootstrap = spawnSync(
    "node",
    [
      join(exampleRoot, "scripts", "bootstrap.mjs"),
      "--endpoint",
      managementEndpoint,
      "--data-endpoint",
      dataEndpoint,
      "--token",
      developerToken,
      "--output",
      envFile,
      "--config-dir",
      path("cli-config"),
      ...(functionsEndpoint === null
        ? []
        : ["--functions", "--functions-endpoint", functionsEndpoint]),
    ],
    { env: { ...process.env, MAKO_WAIT_INTERVAL_MS: "250" }, encoding: "utf8" },
  );
  if (rationalBootstrap.status !== 0) {
    throw new Error(
      `Rational bootstrap failed:\n${rationalBootstrap.stderr}\n${rationalBootstrap.stdout}\n` +
        `data plane log tail:\n${tail(path("data-plane.log"))}\ncontrol plane log tail:\n${tail(
          path("control-plane.log"),
        )}`,
    );
  }
  const bootstrapped = JSON.parse(rationalBootstrap.stdout) as Omit<
    LiveTenant,
    "appUrl" | "dataEndpoint" | "managementEndpoint"
  > & { endpoint: string };
  if (functionsEndpoint !== null && bootstrapped.functionsEndpoint == null) {
    // The gateway is up but nothing was deployed onto it; say why rather than
    // letting the households spec skip silently. The runtime's own log is
    // where a worker that could not boot says so.
    const engine = containerEngine();
    const runtimeLog = spawnSync(
      engine[0] as string,
      [...engine.slice(1), "logs", "--tail", "40", CONTAINER_NAME],
      { encoding: "utf8" },
    );
    const boot = `${runtimeLog.stdout ?? ""}${runtimeLog.stderr ?? ""}`
      .split("\n")
      .filter((line) => line.includes("error") || line.includes("Error"))
      .slice(-4)
      .join("\n");
    console.warn(
      `rational live: the households function was not deployed.\n${rationalBootstrap.stderr
        .split("\n")
        .filter((line) => line.includes("function") || line.includes("deploy"))
        .join("\n")}\n${boot}`,
    );
  }

  const seed = spawnSync(
    "node",
    [
      join(exampleRoot, "scripts", "seed.mjs"),
      "--env-file",
      envFile,
      "--email",
      bootstrapped.owner.email,
      "--password",
      bootstrapped.owner.password,
      "--household-id",
      bootstrapped.householdId,
    ],
    { encoding: "utf8" },
  );
  if (seed.status !== 0) {
    throw new Error(`Rational seed failed:\n${seed.stderr}\n${seed.stdout}`);
  }

  // Serve the application from one origin that also proxies the API, matching
  // how a deployment fronts both behind a reverse proxy.
  const appPort = await freePort();
  const vite = spawn(
    "npx",
    ["vite", "--host", "127.0.0.1", "--port", String(appPort), "--strictPort"],
    {
      cwd: exampleRoot,
      env: {
        ...process.env,
        MAKO_LIVE_ENDPOINT: dataEndpoint,
        ...(functionsEndpoint === null ? {} : { MAKO_FUNCTIONS_ENDPOINT: functionsEndpoint }),
      },
      stdio: "ignore",
    },
  );
  processes.push(vite);
  const appUrl = `http://127.0.0.1:${appPort}`;
  await waitFor(async () => (await fetch(appUrl)).ok, "the application to be served");

  const live: LiveTenant = {
    appUrl,
    dataEndpoint,
    managementEndpoint,
    projectId: bootstrapped.projectId,
    environmentId: bootstrapped.environmentId,
    publicProjectKey: bootstrapped.publicProjectKey,
    householdId: bootstrapped.householdId,
    owner: bootstrapped.owner,
    editor: bootstrapped.editor,
    // The bootstrap reports what it actually deployed: a runtime that would
    // not start leaves this null and the households spec skips.
    functionsEndpoint:
      (bootstrapped as { functionsEndpoint?: string | null }).functionsEndpoint ?? null,
  };
  writeFileSync(tenantFile, JSON.stringify(live, null, 2));
}

function tail(file: string, lines = 40): string {
  try {
    return readFileSync(file, "utf8").split("\n").slice(-lines).join("\n");
  } catch {
    return "(no log)";
  }
}

/** Every fixed port the hosted function path needs must be free. */
async function freePorts(): Promise<boolean> {
  for (const port of [DATA_PLANE_PORT, CONTROL_PLANE_PORT, GATEWAY_PORT, SUPERVISOR_PORT]) {
    if (!(await portIsFree(port))) {
      console.warn(`rational live: port ${port} is in use; functions are skipped.`);
      return false;
    }
  }
  return true;
}

export async function globalTeardown(): Promise<void> {
  for (const child of processes) child.kill();
  const engine = containerEngine();
  for (const container of [runtimeContainer, objectStoreContainer]) {
    if (container === null) continue;
    spawnSync(engine[0] as string, [...engine.slice(1), "rm", "-f", container], {
      stdio: "ignore",
    });
  }
  runtimeContainer = null;
  objectStoreContainer = null;
  if (workspace !== undefined && process.env.RATIONAL_KEEP_WORKSPACE === undefined) {
    rmSync(workspace, { recursive: true, force: true });
  }
  rmSync(tenantFile, { force: true });
}
