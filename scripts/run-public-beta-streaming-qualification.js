#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";

import {
  assert,
  assertSelectedRelease,
  betaSshArguments,
  parseOptions,
  qualificationDeveloper,
  repositoryRoot,
  writeJson,
} from "./public-beta-hosted-lib.js";

const origin = "https://cloud-test.makodb.com";
const options = parseOptions(process.argv.slice(2));
const reuseFixture = options.reuseFixture === "true";
assert(
  options.reuseFixture === undefined || ["true", "false"].includes(options.reuseFixture),
  "--reuse-fixture must be true or false",
);
const manifest = JSON.parse(
  readFileSync(resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json"), "utf8"),
);
const releaseDigest = options.releaseDigest ?? manifest.releaseDigest;
const evidencePath = options.output ?? "docs/evidence/public-beta-streaming-qualification.json";
const privateRoot = resolve(repositoryRoot, ".local/qualification");
const statePath = resolve(privateRoot, "public-beta-fixture.json");
const secretPath = resolve(repositoryRoot, ".local/public-beta-secrets/internal-auth");
const sessionBinary = resolve(repositoryRoot, "target/release/mako-control-session");
const collectionId = "qualification_documents";
const functionName = "qualification-stream";
const schema = {
  type: "object",
  required: ["id", "owner_id", "value"],
  properties: {
    id: { type: "string", minLength: 1, maxLength: 128 },
    owner_id: { type: "string", minLength: 1, maxLength: 128 },
    value: { type: "string", maxLength: 4096 },
  },
  additionalProperties: false,
};

assertSelectedRelease(releaseDigest);
mkdirSync(privateRoot, { recursive: true, mode: 0o700 });
chmodSync(privateRoot, 0o700);
let state = readState();
let developerToken = null;
let organization;
let project;
let environment;

if (reuseFixture) {
  validateReusableState(state);
  organization = { id: state.organizationId };
  project = { id: state.projectId };
  environment = { id: state.environmentId };
} else {
  assert(existsSync(secretPath), "the protected deployment secret is unavailable");
  assert(existsSync(sessionBinary), "build target/release/mako-control-session first");
  developerToken = issueDeveloperToken(qualificationDeveloper());

  organization = await findOrCreate(
    "/v1/organizations",
    (item) => item.name === "Public Beta Qualification",
    {
      method: "POST",
      developer: true,
      body: { name: "Public Beta Qualification" },
      expected: [201],
    },
  );
  project = await findOrCreate(
    `/v1/projects?organizationId=${encodeURIComponent(organization.id)}`,
    (item) => item.name === "Public Beta Qualification",
    {
      path: "/v1/projects",
      method: "POST",
      developer: true,
      headers: { "Idempotency-Key": "qualification-project-v1" },
      body: {
        organizationId: organization.id,
        name: "Public Beta Qualification",
        region: "us-east-1-beta",
      },
      expected: [202],
    },
  );
  environment = await findOrCreate(
    `/v1/projects/${project.id}/environments`,
    (item) => item.name === "Public Beta Qualification",
    {
      method: "POST",
      developer: true,
      headers: { "Idempotency-Key": "qualification-environment-v1" },
      body: { name: "Public Beta Qualification" },
      expected: [202],
    },
  );
}
assertIdentifier(project.id, "prj_");
assertIdentifier(environment.id, "env_");

const tenantBase = `/v1/projects/${project.id}/environments/${environment.id}`;
const collectionPath = `${tenantBase}/collections/${collectionId}`;
if (!reuseFixture) {
  const existingCollection = await api(collectionPath, {
    developer: true,
    expected: [200, 404],
  });
  if (existingCollection.status === 404) {
    await api(`${tenantBase}/collections`, {
      method: "POST",
      developer: true,
      headers: { "Idempotency-Key": "qualification-collection-v1" },
      body: {
        id: collectionId,
        schemaVersion: 1,
        jsonSchema: schema,
        primaryKey: { kind: "field", field: "id" },
      },
      expected: [201],
    });
  }
}

state = {
  ...state,
  schemaVersion: 1,
  releaseDigest,
  organizationId: organization.id,
  projectId: project.id,
  environmentId: environment.id,
  collectionId,
  applicationEmail: state.applicationEmail ?? `qualification+${project.id.slice(4, 16)}@makodb.com`,
  applicationPassword: state.applicationPassword ?? `${randomBytes(24).toString("base64url")}Aa1!`,
};
writeState(state);

if (!reuseFixture && !validPublicProjectKey(state.publicProjectKey)) {
  const credentialId = `qualification_public_${Date.now()}`;
  const issued = await api(`${tenantBase}/credentials/public`, {
    method: "POST",
    developer: true,
    headers: { "Idempotency-Key": `qualification-public-key-${Date.now()}` },
    body: { id: credentialId },
    expected: [201],
  });
  assert(validPublicProjectKey(issued.body.value), "public project credential was not issued");
  state.publicCredentialId = credentialId;
  state.publicProjectKey = issued.body.value;
  writeState(state);
}

console.log("qualification stage: data-plane fixture");
installDataPlaneFixture(project.id, environment.id);
console.log("qualification stage: application auth");
await api(`${tenantBase}/auth/signup`, {
  method: "POST",
  headers: { "X-Mako-Key": state.publicProjectKey },
  body: { email: state.applicationEmail, password: state.applicationPassword },
  expected: [202],
});
const session = await signIn();
state.applicationUserId = session.user.id;
writeState(state);

console.log("qualification stage: function deployment");
const functionSetup = reuseFixture
  ? { name: functionName, existingFixture: true, verifiedByExternalInvocation: true }
  : await ensureFunction();
console.log("qualification stage: external HTTPS checks");
const checks = await runExternalChecks(session.accessToken);

assertSelectedRelease(releaseDigest);
writeJson(evidencePath, {
  schemaVersion: 1,
  recordedAt: new Date().toISOString(),
  environment: "public-beta",
  vmId: 124,
  publicOrigin: origin,
  releaseDigest,
  planHash: manifest.planHash,
  qualificationMode: reuseFixture ? "existing-isolated-fixture" : "developer-managed-fixture",
  tenant: {
    organizationId: organization.id,
    projectId: project.id,
    environmentId: environment.id,
    collectionId,
    applicationUserId: state.applicationUserId,
    publicCredentialId: state.publicCredentialId,
    secretsRetainedIn: ".local/qualification/public-beta-fixture.json",
  },
  function: functionSetup,
  external: checks,
  passed:
    checks.httpRedirect.status === 308 &&
    checks.https.status === 200 &&
    checks.https.tlsVerified &&
    checks.internalRouteStatus === 404 &&
    checks.unknownRouteStatus === 404 &&
    checks.sse.status === 200 &&
    checks.sse.contentType.startsWith("text/event-stream") &&
    checks.sse.firstEvent !== null &&
    checks.function.status === 200 &&
    checks.function.bodyBytes >= 64 * 1024,
});
assert(checks.sse.firstEvent !== null, "the external SSE stream did not emit an event");
assert(checks.function.status === 200, "the external function invocation failed");
console.log(`public-beta HTTPS streaming qualification passed: ${evidencePath}`);

function issueDeveloperToken(developer) {
  const temporary = mkdtempSync(resolve(privateRoot, "session-"));
  chmodSync(temporary, 0o700);
  const tokenPath = resolve(temporary, "developer.jwt");
  try {
    execFileSync(
      sessionBinary,
      [
        "--secret-file",
        secretPath,
        "--output",
        tokenPath,
        "--issuer",
        `${origin}/control-identity`,
        "--identity-id",
        developer.developerIdentityId,
        "--email",
        developer.email,
        "--display-name",
        developer.displayName,
        "--authorization-epoch",
        String(developer.authorizationEpoch),
        "--ttl-seconds",
        "3600",
      ],
      { cwd: repositoryRoot, stdio: ["ignore", "ignore", "inherit"] },
    );
    return readFileSync(tokenPath, "utf8");
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

async function findOrCreate(listPath, predicate, create) {
  const listed = await api(listPath, { developer: true, expected: [200] });
  const matches = listed.body.items.filter(predicate);
  assert(matches.length <= 1, `multiple qualification resources match ${listPath}`);
  if (matches.length === 1) return matches[0];
  const path = create.path ?? listPath;
  return (await api(path, create)).body;
}

async function signIn() {
  const result = await api(`${tenantBase}/auth/signin`, {
    method: "POST",
    headers: { "X-Mako-Key": state.publicProjectKey },
    body: { email: state.applicationEmail, password: state.applicationPassword },
    expected: [200],
  });
  assert(typeof result.body.accessToken === "string", "application access token was not issued");
  assert(typeof result.body.user?.id === "string", "application user identity was not issued");
  return result.body;
}

function installDataPlaneFixture(projectId, environmentId) {
  for (const value of [projectId, environmentId, collectionId, state.publicCredentialId]) {
    assertShellIdentifier(value);
  }
  const command = `set -u
sudo systemctl stop mako-data-plane.service
fixture_status=0
sudo -u mako-data-plane /opt/mako/current/bin/mako-qualification-fixture \\
  --database-path /srv/mako-data/rocksdb/data-plane \\
  --database-id mako-data-plane-us-east-1-beta \\
  --disk-warning-free-bytes 17179869184 \\
  --disk-critical-free-bytes 8589934592 \\
  --project ${projectId} \\
  --environment ${environmentId} \\
  --collection ${collectionId} \\
  --public-credential-id ${state.publicCredentialId} >/dev/null || fixture_status=$?
sudo systemctl reset-failed mako-data-plane.service || true
sudo systemctl start mako-data-plane.service
for attempt in $(seq 1 30); do
  if curl --silent --fail --max-time 2 http://127.0.0.1:8080/readyz >/dev/null; then break; fi
  sleep 1
done
sudo systemctl is-active --quiet mako-data-plane.service
exit $fixture_status`;
  execFileSync("ssh", [...betaSshArguments, command], {
    cwd: repositoryRoot,
    input: state.publicProjectKey,
    stdio: ["pipe", "ignore", "inherit"],
    timeout: 90_000,
  });
}

async function ensureFunction() {
  const functions = await api(`${tenantBase}/functions`, {
    developer: true,
    expected: [200],
  });
  let record = functions.body.items.find((item) => item.name === functionName);
  if (record === undefined) {
    record = (
      await api(`${tenantBase}/functions`, {
        method: "POST",
        developer: true,
        headers: { "Idempotency-Key": "qualification-function-v1" },
        body: {
          name: functionName,
          configuration: {
            verifyJwt: true,
            regions: ["us-east-1-beta"],
            secretNames: [],
            limits: {
              cpuMilliseconds: 10_000,
              wallMilliseconds: 30_000,
              memoryBytes: 268_435_456,
              requestBytes: 1_048_576,
              responseBytes: 1_048_576,
              concurrency: 32,
            },
          },
        },
        expected: [201],
      })
    ).body;
  }
  const source = `const encoder = new TextEncoder();\nDeno.serve(() => {\n  const stream = new ReadableStream({\n    start(controller) {\n      for (let index = 0; index < 128; index += 1) {\n        controller.enqueue(encoder.encode(\`mako-stream-\${String(index).padStart(4, "0")}:\${"x".repeat(1000)}\\n\`));\n      }\n      controller.close();\n    },\n  });\n  return new Response(stream, { headers: { "content-type": "text/plain; charset=utf-8" } });\n});\n`;
  const bundle = await api(`${tenantBase}/function-bundles`, {
    method: "POST",
    developer: true,
    headers: { "Idempotency-Key": "qualification-function-bundle-v1" },
    body: {
      kind: "source",
      entrypoint: "index.ts",
      files: [{ path: "index.ts", contentBase64: Buffer.from(source).toString("base64") }],
      dependencies: {},
    },
    expected: [200],
  });
  assert(bundle.body.status === "ready", "qualification function bundle was rejected");
  const digest = bundle.body.artifact?.digest;
  assert(/^sha256:[0-9a-f]{64}$/u.test(digest), "qualification function digest is invalid");
  const versions = await api(`${tenantBase}/functions/${functionName}/versions`, {
    developer: true,
    expected: [200],
  });
  if (!versions.body.items.some((item) => item.version === 1)) {
    await api(`${tenantBase}/functions/${functionName}/versions`, {
      method: "POST",
      developer: true,
      headers: { "Idempotency-Key": "qualification-function-version-v1" },
      body: {
        version: 1,
        bundleDigest: digest,
        entrypoint: "index.ts",
        runtimeVersion: "v1.74.3",
      },
      expected: [202],
    });
  }
  if (record.activeVersion !== 1) {
    record = (
      await api(`${tenantBase}/functions/${functionName}/versions/1/actions/promote`, {
        method: "POST",
        developer: true,
        headers: { "Idempotency-Key": "qualification-function-promote-v1" },
        expected: [200],
      })
    ).body;
  }
  return {
    name: functionName,
    version: 1,
    bundleDigest: digest,
    verifyJwt: true,
    active: record.activeVersion === 1,
  };
}

async function runExternalChecks(accessToken) {
  console.log("qualification stage: external routing");
  const redirect = await fetch(`http://cloud-test.makodb.com/`, {
    redirect: "manual",
    signal: AbortSignal.timeout(10_000),
  });
  const https = await fetch(`${origin}/`, { signal: AbortSignal.timeout(10_000) });
  const internal = await fetch(`${origin}/_internal/v1/identity/verify`, {
    signal: AbortSignal.timeout(10_000),
  });
  const unknown = await fetch(`${origin}/v1/qualification-not-a-route`, {
    signal: AbortSignal.timeout(10_000),
  });
  const sseUrl = `${origin}${tenantBase}/collections/${collectionId}/replication/stream?schemaVersion=1`;
  console.log("qualification stage: authenticated SSE");
  const sseStarted = performance.now();
  const sseResponse = await fetch(sseUrl, {
    headers: {
      Accept: "text/event-stream",
      Authorization: `Bearer ${accessToken}`,
      "X-Mako-Key": state.publicProjectKey,
    },
    signal: AbortSignal.timeout(20_000),
  });
  const firstFrame = await readFirstSseFrame(sseResponse);
  const projectRef = `${project.id}--${environment.id}`;
  console.log("qualification stage: authenticated function streaming");
  const functionStarted = performance.now();
  const functionResponse = await fetch(`${origin}/${projectRef}/functions/v1/${functionName}`, {
    headers: { Authorization: `Bearer ${accessToken}` },
    signal: AbortSignal.timeout(30_000),
  });
  const functionBytes = Buffer.from(await functionResponse.arrayBuffer());
  return {
    httpRedirect: {
      status: redirect.status,
      location: redirect.headers.get("location"),
    },
    https: {
      status: https.status,
      tlsVerified: true,
      hsts: https.headers.get("strict-transport-security"),
    },
    internalRouteStatus: internal.status,
    unknownRouteStatus: unknown.status,
    sse: {
      status: sseResponse.status,
      contentType: sseResponse.headers.get("content-type") ?? "",
      bufferingDisabled: sseResponse.headers.get("x-accel-buffering") === "no",
      firstEvent: firstFrame.event,
      firstFrameBytes: firstFrame.bytes,
      firstFrameMilliseconds: performance.now() - sseStarted,
    },
    function: {
      status: functionResponse.status,
      contentType: functionResponse.headers.get("content-type") ?? "",
      bodyBytes: functionBytes.length,
      bodySha256: createHash("sha256").update(functionBytes).digest("hex"),
      elapsedMilliseconds: performance.now() - functionStarted,
      requestIdPresent: functionResponse.headers.has("x-mako-request-id"),
    },
  };
}

async function readFirstSseFrame(response) {
  assert(response.status === 200, `SSE returned HTTP ${response.status}`);
  const reader = response.body?.getReader();
  assert(reader !== undefined, "SSE response has no readable body");
  let accumulated = Buffer.alloc(0);
  try {
    while (accumulated.length <= 1024 * 1024) {
      const next = await reader.read();
      if (next.done) break;
      accumulated = Buffer.concat([accumulated, Buffer.from(next.value)]);
      const boundary = accumulated.indexOf("\n\n");
      if (boundary >= 0) {
        const frame = accumulated.subarray(0, boundary + 2).toString("utf8");
        return {
          event: /^event: ([a-z_]+)$/mu.exec(frame)?.[1] ?? null,
          bytes: Buffer.byteLength(frame),
        };
      }
    }
  } finally {
    await reader.cancel();
  }
  return { event: null, bytes: accumulated.length };
}

async function api(path, request = {}) {
  const headers = { Accept: "application/json", ...(request.headers ?? {}) };
  if (request.developer) headers.Authorization = `Bearer ${developerToken}`;
  if (request.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(`${origin}${path}`, {
    method: request.method ?? "GET",
    headers,
    body: request.body === undefined ? undefined : JSON.stringify(request.body),
    signal: AbortSignal.timeout(30_000),
  });
  const text = await response.text();
  let body = null;
  if (text !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      throw new Error(`${path} returned a non-JSON response`);
    }
  }
  const expected = request.expected ?? [200];
  if (!expected.includes(response.status)) {
    const code = body?.error?.code ?? "unknown";
    const message = body?.error?.message ?? "no safe error message";
    throw new Error(`${path} returned HTTP ${response.status} (${code}: ${message})`);
  }
  return { status: response.status, body, headers: response.headers };
}

function readState() {
  if (!existsSync(statePath)) return {};
  const value = JSON.parse(readFileSync(statePath, "utf8"));
  assert(value.schemaVersion === 1, "qualification fixture state schema is invalid");
  return value;
}

function validateReusableState(value) {
  assert(value.schemaVersion === 1, "qualification fixture state schema is invalid");
  assertIdentifier(value.organizationId, "org_");
  assertIdentifier(value.projectId, "prj_");
  assertIdentifier(value.environmentId, "env_");
  assert(value.collectionId === collectionId, "qualification collection is invalid");
  assertShellIdentifier(value.publicCredentialId);
  assert(validPublicProjectKey(value.publicProjectKey), "qualification public key is invalid");
  assert(
    typeof value.applicationEmail === "string" &&
      value.applicationEmail.includes("@") &&
      !/[\r\n\0]/u.test(value.applicationEmail),
    "qualification application email is invalid",
  );
  assert(
    typeof value.applicationPassword === "string" && value.applicationPassword.length >= 12,
    "qualification application password is invalid",
  );
}

function writeState(value) {
  const temporaryPath = `${statePath}.tmp-${process.pid}`;
  writeFileSync(temporaryPath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  chmodSync(temporaryPath, 0o600);
  renameSync(temporaryPath, statePath);
}

function assertIdentifier(value, prefix) {
  assert(
    new RegExp(`^${prefix}[A-Za-z0-9_-]{8,64}$`, "u").test(value),
    `${prefix} resource identifier is invalid`,
  );
}

function assertShellIdentifier(value) {
  assert(/^[A-Za-z0-9_-]{1,128}$/u.test(value), "fixture identifier is unsafe");
}

function validPublicProjectKey(value) {
  return typeof value === "string" && /^mako_pk\.[A-Za-z0-9_-]{1,128}\.[0-9a-f]{64}$/u.test(value);
}
