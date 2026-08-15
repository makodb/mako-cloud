import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import test from "node:test";

import { createRuntimeLaunchPlan } from "../dist/index.js";

const enabled = process.env.MAKO_RUN_EDGE_RUNTIME_TESTS === "1";
const packageDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const functionDirectory = join(packageDirectory, "test/fixtures/compatibility");
const secretValue = "compatibility-secret-value";

test(
  "the pinned local runtime supports the qualified edge-function API surface",
  { skip: !enabled, timeout: 180_000 },
  async (t) => {
    const root = await mkdtemp(join(tmpdir(), "mako-edge-compatibility-"));
    t.after(() => rm(root, { recursive: true, force: true }));
    const environmentFile = join(root, "function.env");
    const secretFile = join(root, "function.secrets");
    await writeFile(environmentFile, "FUNCTION_MODE=compatibility\n", "utf8");
    await writeFile(secretFile, `TEST_SECRET=${secretValue}\n`, "utf8");

    const outbound = createServer((_, response) => {
      response.writeHead(200, { "content-type": "text/plain", "x-test-upstream": "reached" });
      response.end("outbound-ok");
    });
    await listen(outbound);
    t.after(() => closeServer(outbound));
    const outboundAddress = outbound.address();
    assert.ok(outboundAddress !== null && typeof outboundAddress === "object");

    const port = await availablePort();
    const engine = process.env.MAKO_EDGE_TEST_ENGINE === "podman" ? "podman" : "docker";
    const plan = createRuntimeLaunchPlan({
      functionDirectory,
      functionName: "compatibility",
      entrypoint: "index.ts",
      projectId: "prj_abcdefgh",
      environmentId: "env_abcdefgh",
      apiUrl: "http://host.docker.internal:8787",
      port,
      wallTimeMilliseconds: 30_000,
      verifyJwt: false,
      envFiles: [environmentFile],
      secretFiles: [secretFile],
      containerEngine: engine,
      dryRun: false,
    });
    const prefix = parseEnginePrefix();
    const child = spawn(plan.command, [...prefix, ...plan.args], {
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

    const base = `http://127.0.0.1:${port}/prj_abcdefgh/functions/v1/compatibility`;
    const features = await waitForJson(`${base}/features?qualified=true`, child, () => output);
    assert.deepEqual(features, {
      fetchApi: true,
      typeScript: "typescript-ok",
      javaScript: "javascript-module-ok",
      npm: 12,
      webAssembly: 42,
      environment: "compatibility",
      secret: secretValue,
      undeclaredEnvironmentAbsent: true,
      functionPath: "/features",
      query: "?qualified=true",
    });

    const echo = await fetch(`${base}/echo`, { method: "PATCH", body: "request-body" });
    assert.equal(echo.status, 200);
    assert.deepEqual(await echo.json(), { method: "PATCH", body: "request-body" });

    const stream = await fetch(`${base}/stream`);
    assert.equal(stream.headers.get("content-type"), "text/plain");
    assert.equal(await stream.text(), "stream-response");

    const target = encodeURIComponent(
      `http://host.docker.internal:${outboundAddress.port}/qualified`,
    );
    const outboundResponse = await fetch(`${base}/outbound?target=${target}`);
    assert.equal(outboundResponse.status, 200);
    assert.equal(outboundResponse.headers.get("x-upstream-result"), "reached");
    assert.equal(await outboundResponse.text(), "outbound-ok");
  },
);

function parseEnginePrefix() {
  const source = process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON ?? "[]";
  const value = JSON.parse(source);
  if (!Array.isArray(value) || value.some((part) => typeof part !== "string")) {
    throw new Error("MAKO_EDGE_TEST_ENGINE_PREFIX_JSON must be a JSON string array");
  }
  return value;
}

async function waitForJson(url, child, output) {
  let lastFailure = "runtime did not accept a request";
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`runtime exited early:\n${output()}`);
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(15_000) });
      const body = await response.text();
      if (response.ok) return JSON.parse(body);
      lastFailure = `HTTP ${response.status}: ${body}`;
    } catch (error) {
      lastFailure = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`${lastFailure}\n${output()}`);
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
