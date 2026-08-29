import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  LocalServeConfigurationError,
  createRuntimeLaunchPlan,
  formatLaunchPlan,
  parseServeArguments,
  redactRuntimeOutput,
} from "../dist/index.js";

const packageDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const repositoryDirectory = dirname(dirname(packageDirectory));
const secretValue = "local-secret-value-that-must-not-be-printed";

test("local serve maps only declared environment and secret values into the pinned runtime", async (t) => {
  const fixture = await createFixture(t);
  const config = parseServeArguments(
    [
      "functions",
      "serve",
      fixture.functionDirectory,
      "--project-id",
      "prj_abcdefgh",
      "--environment-id",
      "env_abcdefgh",
      "--function-name",
      "hello-world",
      "--api-url",
      "http://host.docker.internal:8787/",
      "--env-file",
      fixture.environmentFile,
      "--secret-file",
      fixture.secretFile,
      "--no-verify-jwt",
      "--container-engine",
      "docker",
      "--dry-run",
    ],
    fixture.root,
  );

  const plan = createRuntimeLaunchPlan(config);
  const printable = formatLaunchPlan(plan);
  assert.equal(
    plan.image,
    "docker.io/supabase/edge-runtime@sha256:c52405002a890ca9fcf77978671c57f3a988e03174afb277f84ac65bc917013c",
  );
  assert.deepEqual(plan.exposedEnvironmentNames, ["LOG_LEVEL", "SERVICE_TOKEN"]);
  assert.deepEqual(plan.secretNames, ["SERVICE_TOKEN"]);
  assert.equal(plan.environment.LOG_LEVEL, "debug");
  assert.equal(plan.environment.SERVICE_TOKEN, secretValue);
  assert.equal(plan.environment.MAKO_VERIFY_JWT, "false");
  assert.equal(plan.environment.MAKO_WALL_TIME_MS, "300000");
  assert.equal(plan.environment.DENO_DIR, "/tmp/deno-cache");
  assert.ok(plan.args.includes("SERVICE_TOKEN"));
  assert.ok(plan.args.includes("LOG_LEVEL"));
  assert.equal(plan.args.includes(secretValue), false);
  assert.equal(printable.includes(secretValue), false);
  assert.match(printable, /--read-only/u);
  assert.match(printable, /\/home\/deno\/functions\/user,readonly/u);
  assert.match(printable, /127\.0\.0\.1:9000:9000/u);
  const runtimeTimeoutIndex = plan.args.indexOf("--user-worker-request-idle-timeout");
  assert.ok(runtimeTimeoutIndex >= 0);
  assert.equal(plan.args[runtimeTimeoutIndex + 1], "330000");
});

test("JWT verification is default-on and requires one complete Ed25519 configuration", async (t) => {
  const fixture = await createFixture(t);
  assert.throws(
    () =>
      parseServeArguments(
        [
          "functions",
          "serve",
          fixture.functionDirectory,
          "--project-id",
          "prj_abcdefgh",
          "--environment-id",
          "env_abcdefgh",
        ],
        fixture.root,
      ),
    (error) =>
      error instanceof LocalServeConfigurationError &&
      error.message.includes("JWT verification requires"),
  );

  const config = parseServeArguments(
    [
      "functions",
      "serve",
      fixture.functionDirectory,
      "--project-id",
      "prj_abcdefgh",
      "--environment-id",
      "env_abcdefgh",
      "--jwks-file",
      fixture.jwksFile,
      "--jwt-issuer",
      "https://auth.example.test",
      "--jwt-audience",
      "mako-app",
      "--container-engine",
      "podman",
    ],
    fixture.root,
  );
  const plan = createRuntimeLaunchPlan(config);
  assert.equal(config.verifyJwt, true);
  assert.equal(plan.command, "podman");
  assert.equal(plan.environment.MAKO_JWT_ISSUER, "https://auth.example.test");
  assert.equal(JSON.parse(plan.environment.MAKO_JWKS).keys[0].kid, "local-key");
});

test("reserved, duplicate, and partially configured environment inputs fail closed", async (t) => {
  const fixture = await createFixture(t);
  const reservedFile = join(fixture.root, "reserved.env");
  await writeFile(reservedFile, "MAKO_PROJECT_ID=attacker\n", "utf8");

  const base = [
    "functions",
    "serve",
    fixture.functionDirectory,
    "--project-id",
    "prj_abcdefgh",
    "--environment-id",
    "env_abcdefgh",
    "--no-verify-jwt",
  ];
  const reserved = parseServeArguments([...base, "--env-file", reservedFile], fixture.root);
  assert.throws(() => createRuntimeLaunchPlan(reserved), /reserved name/u);

  const duplicate = parseServeArguments(
    [...base, "--env-file", fixture.environmentFile, "--secret-file", fixture.duplicateFile],
    fixture.root,
  );
  assert.throws(() => createRuntimeLaunchPlan(duplicate), /duplicated/u);

  assert.throws(
    () => parseServeArguments([...base, "--jwks-file", fixture.jwksFile], fixture.root),
    /requires --jwks-file, --jwt-issuer, and --jwt-audience together/u,
  );
});

test("the packaged pin and main worker preserve the hosted request boundary", async () => {
  const packagedPin = JSON.parse(
    await readFile(join(packageDirectory, "runtime/runtime-pin.json"), "utf8"),
  );
  const hostedPin = JSON.parse(
    await readFile(join(repositoryDirectory, "infra/edge-runtime/runtime-pin.json"), "utf8"),
  );
  assert.deepEqual(packagedPin, hostedPin);

  const mainWorker = await readFile(join(packageDirectory, "runtime/main/index.ts"), "utf8");
  const supervisor = await readFile(
    join(packageDirectory, "runtime/main/supervisor.ts"),
    "utf8",
  );
  assert.match(mainWorker, /EdgeRuntime\.userWorkers\.create/u);
  assert.match(mainWorker, /worker\.fetch\(forwarded/u);
  assert.match(mainWorker, /EdgeRuntime\.applySupabaseTag\(request, forwarded\)/u);
  assert.match(mainWorker, /crypto\.subtle\.verify/u);
  assert.match(mainWorker, /headers\.delete\("authorization"\)/u);
  assert.match(mainWorker, /x-mako-caller-authorization/u);
  assert.match(mainWorker, /pathname\.slice\(stablePrefix\.length\)/u);
  assert.match(mainWorker, /RuntimeSupervisor\.open\(\)/u);
  assert.match(mainWorker, /pathname\.startsWith\("\/_mako\/runtime\/"\)/u);
  assert.doesNotMatch(mainWorker, /port: 9001/u);
  assert.match(supervisor, /\/_mako\/runtime\/v1\/deployments\/load/u);
  assert.match(supervisor, /\/_mako\/runtime\/v1\/deployments\/retire/u);
  assert.match(supervisor, /crypto\.subtle\.encrypt/u);
  assert.match(supervisor, /crypto\.subtle\.decrypt/u);
  assert.match(supervisor, /deployment_loaded/u);

  // Deny by default, spelled the way the runtime reads it. An empty list is
  // Deno's "granted without restriction", so a worker started with
  // `allow_net: []` reaches every host and one with `allow_write: []` writes
  // anywhere the container can; `null` is the absence of a grant. Neither
  // worker may hand out an empty list for anything.
  const emptyGrant = /allow_(?:all|env|net|read|write|import|run|ffi|sys): \[\]/u;
  assert.doesNotMatch(supervisor, emptyGrant);
  assert.doesNotMatch(mainWorker, emptyGrant);
  for (const grant of ["write", "import", "run", "ffi", "sys"]) {
    assert.match(supervisor, new RegExp(`allow_${grant}: null`, "u"));
    assert.match(mainWorker, new RegExp(`allow_${grant}: null`, "u"));
  }
  // What remains is bounded: the worker's own directory, the names it was
  // given, and the origins its egress policy allows.
  assert.match(supervisor, /allow_net: networkGrants\(limits\.outboundNetwork\)/u);
  assert.match(supervisor, /allow_read: \[directory\]/u);
  assert.match(mainWorker, /allow_net: \[originGrant\(requiredEnvironment\("MAKO_API_URL"\)\)\]/u);
  assert.match(mainWorker, /allow_read: \[functionPath\]/u);
});

test("runtime output redaction covers every exact active secret", () => {
  assert.equal(
    redactRuntimeOutput(
      `before ${secretValue} middle shorter-secret after`,
      ["shorter-secret", secretValue],
    ),
    "before [REDACTED] middle [REDACTED] after",
  );
});

async function createFixture(t) {
  const root = await mkdtemp(join(tmpdir(), "mako-cli-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const functionDirectory = join(root, "hello-world");
  await mkdir(functionDirectory);
  await writeFile(
    join(functionDirectory, "index.ts"),
    "Deno.serve(() => new Response('hello'));\n",
    "utf8",
  );
  const environmentFile = join(root, "local.env");
  const secretFile = join(root, "local.secrets");
  const duplicateFile = join(root, "duplicate.secrets");
  const jwksFile = join(root, "jwks.json");
  await writeFile(environmentFile, "LOG_LEVEL=debug\n", "utf8");
  await writeFile(secretFile, `SERVICE_TOKEN=${secretValue}\n`, "utf8");
  await writeFile(duplicateFile, "LOG_LEVEL=secret-debug\n", "utf8");
  await writeFile(
    jwksFile,
    JSON.stringify({
      keys: [
        {
          kty: "OKP",
          crv: "Ed25519",
          alg: "EdDSA",
          use: "sig",
          kid: "local-key",
          x: "A".repeat(43),
        },
      ],
    }),
    "utf8",
  );
  return {
    root,
    functionDirectory,
    environmentFile,
    secretFile,
    duplicateFile,
    jwksFile,
  };
}
