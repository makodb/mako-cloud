import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { createRuntimeLaunchPlan, redactRuntimeOutput } from "../dist/index.js";

const enabled = process.env.MAKO_RUN_EDGE_RUNTIME_TESTS === "1";
const packageDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const functionDirectory = join(packageDirectory, "test/fixtures/adversarial");

test(
  "the pinned runtime contains adversarial failures within one project worker",
  { skip: !enabled, timeout: 180_000 },
  async (t) => {
    const outboundAttempts = [];
    const outbound = createServer((request, response) => {
      outboundAttempts.push(request.url);
      response.end("unexpected");
    });
    await listen(outbound);
    t.after(() => closeServer(outbound));
    const outboundAddress = outbound.address();
    assert.ok(outboundAddress !== null && typeof outboundAddress === "object");

    const first = await startRuntime(t, {
      projectId: "prj_adversary1",
      environmentId: "env_adversary1",
      marker: "project-a",
      secret: "project-a-secret-canary",
    });
    const second = await startRuntime(t, {
      projectId: "prj_adversary2",
      environmentId: "env_adversary2",
      marker: "project-b",
      secret: "project-b-secret-canary",
    });

    const firstInspection = await getJson(`${first.base}/inspect`, first);
    const secondInspection = await getJson(`${second.base}/inspect`, second);
    assert.deepEqual(firstInspection, {
      projectMarker: "project-a",
      projectSecret: "project-a-secret-canary",
      projectId: "prj_adversary1",
      environmentId: "env_adversary1",
      undeclaredAbsent: true,
      processEnvironmentDenied: true,
    });
    assert.deepEqual(secondInspection, {
      projectMarker: "project-b",
      projectSecret: "project-b-secret-canary",
      projectId: "prj_adversary2",
      environmentId: "env_adversary2",
      undeclaredAbsent: true,
      processEnvironmentDenied: true,
    });
    assert.equal(JSON.stringify(firstInspection).includes("project-b-secret-canary"), false);
    assert.equal(JSON.stringify(secondInspection).includes("project-a-secret-canary"), false);

    const leak = await fetch(`${first.base}/leak`);
    const leakBody = await leak.text();
    assert.equal(leak.status, 500);
    assert.equal(leakBody.includes("project-a-secret-canary"), false);
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(first.output().includes("project-a-secret-canary"), false);

    const wall = await fetch(`${first.base}/wall`, { signal: AbortSignal.timeout(10_000) });
    assert.equal(wall.status, 500);
    assert.equal((await wall.text()).includes("wall limit failed"), false);
    assert.equal(await responseText(`${first.base}/safe`), "safe:project-a");

    const memory = await fetch(`${first.base}/memory`, { signal: AbortSignal.timeout(30_000) });
    assert.equal(memory.status, 500);
    assert.equal(await responseText(`${first.base}/safe`), "safe:project-a");

    const crash = await fetch(`${first.base}/crash`, { signal: AbortSignal.timeout(10_000) });
    const crashBody = await crash.text();
    assert.ok(crash.status >= 400, `${crashBody}\n${first.output()}`);
    assert.equal(crashBody.includes("project-a-secret-canary"), false);
    assert.equal(await responseText(`${first.base}/safe`), "safe:project-a");

    const target = encodeURIComponent(
      `http://host.docker.internal:${outboundAddress.port}/after-invocation`,
    );
    const background = await fetch(`${first.base}/background?target=${target}`);
    assert.equal(background.status, 200);
    assert.equal(await background.text(), "scheduled");
    await new Promise((resolve) => setTimeout(resolve, 2_500));
    assert.deepEqual(outboundAttempts, []);
    assert.equal(await responseText(`${first.base}/safe`), "safe:project-a");
  },
);

async function startRuntime(t, identity) {
  const root = await mkdtemp(join(tmpdir(), "mako-edge-adversarial-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const environmentFile = join(root, "function.env");
  const secretFile = join(root, "function.secrets");
  await writeFile(environmentFile, `PROJECT_MARKER=${identity.marker}\n`, "utf8");
  await writeFile(secretFile, `PROJECT_SECRET=${identity.secret}\n`, "utf8");
  const port = await availablePort();
  const engine = process.env.MAKO_EDGE_TEST_ENGINE === "podman" ? "podman" : "docker";
  const plan = createRuntimeLaunchPlan({
    functionDirectory,
    functionName: "adversarial",
    entrypoint: "index.ts",
    projectId: identity.projectId,
    environmentId: identity.environmentId,
    apiUrl: "http://host.docker.internal:8787",
    port,
    wallTimeMilliseconds: 1_000,
    verifyJwt: false,
    envFiles: [environmentFile],
    secretFiles: [secretFile],
    containerEngine: engine,
    dryRun: false,
  });
  const child = spawn(plan.command, [...parseEnginePrefix(), ...plan.args], {
    env: { ...process.env, ...plan.environment },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => {
    output += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    output += chunk.toString();
  });
  t.after(() => stopChild(child));
  const base = `http://127.0.0.1:${port}/${identity.projectId}/functions/v1/adversarial`;
  const runtime = { child, base, output: () => redactRuntimeOutput(output, [identity.secret]) };
  await waitUntilReady(runtime);
  return runtime;
}

function parseEnginePrefix() {
  const source = process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON ?? "[]";
  const value = JSON.parse(source);
  if (!Array.isArray(value) || value.some((part) => typeof part !== "string")) {
    throw new Error("MAKO_EDGE_TEST_ENGINE_PREFIX_JSON must be a JSON string array");
  }
  return value;
}

async function waitUntilReady(runtime) {
  const deadline = Date.now() + 60_000;
  let lastFailure = "runtime did not accept a request";
  while (Date.now() < deadline) {
    if (runtime.child.exitCode !== null) {
      throw new Error(`runtime exited early:\n${runtime.output()}`);
    }
    try {
      const response = await fetch(`${runtime.base}/safe`, {
        signal: AbortSignal.timeout(5_000),
      });
      if (response.ok) return;
      lastFailure = `HTTP ${response.status}: ${await response.text()}`;
    } catch (error) {
      lastFailure = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`${lastFailure}\n${runtime.output()}`);
}

async function getJson(url, runtime) {
  const response = await fetch(url, { signal: AbortSignal.timeout(10_000) });
  if (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}\n${runtime.output()}`);
  return response.json();
}

async function responseText(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(10_000) });
  assert.equal(response.status, 200);
  return response.text();
}

async function availablePort() {
  const server = createServer();
  await listen(server, "127.0.0.1");
  const address = server.address();
  assert.ok(address !== null && typeof address === "object");
  await closeServer(server);
  return address.port;
}

function listen(server, host = "0.0.0.0") {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, host, resolve);
  });
}

function closeServer(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error === undefined ? resolve() : reject(error)));
  });
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  if (child.exitCode === null) child.kill("SIGKILL");
}
