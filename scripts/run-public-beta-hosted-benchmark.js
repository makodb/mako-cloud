#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

import {
  assert,
  assertSelectedRelease,
  parseOptions,
  qualificationDeveloper,
  repositoryRoot,
  ssh,
  writeJson,
} from "./public-beta-hosted-lib.js";

const origin = "https://cloud-test.makodb.com";
const options = parseOptions(process.argv.slice(2));
const reuseFixture = options.reuseFixture === "true";
assert(
  options.reuseFixture === undefined || ["true", "false"].includes(options.reuseFixture),
  "--reuse-fixture must be true or false",
);
const statePath = resolve(repositoryRoot, ".local/qualification/public-beta-fixture.json");
const manifestPath = resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json");
const secretPath = resolve(repositoryRoot, ".local/public-beta-secrets/internal-auth");
const sessionBinary = resolve(repositoryRoot, "target/release/mako-control-session");
const evidencePath = "docs/evidence/public-beta-hosted-benchmark.json";
const targetConcurrency = 16;
const headroomConcurrency = 24;
const saturationProbeConcurrency = 64;

assert(existsSync(statePath), "run the public-beta streaming qualification first");
if (!reuseFixture) {
  assert(existsSync(secretPath), "the protected deployment secret is unavailable");
  assert(existsSync(sessionBinary), "build target/release/mako-control-session first");
}
const state = JSON.parse(readFileSync(statePath, "utf8"));
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const developer = reuseFixture ? null : qualificationDeveloper();
assert(state.schemaVersion === 1, "qualification state schema is invalid");
assertSelectedRelease(manifest.releaseDigest);

const tenantBase = `/v1/projects/${state.projectId}/environments/${state.environmentId}`;
const collectionBase = `${tenantBase}/collections/${state.collectionId}`;
const projectRef = `${state.projectId}--${state.environmentId}`;
const functionPath = `/${projectRef}/functions/v1/qualification-stream`;
const developerToken = reuseFixture ? null : issueDeveloperToken();

const authSamples = [];
let applicationSession;
for (let index = 0; index < 10; index += 1) {
  const started = performance.now();
  applicationSession = await signIn();
  authSamples.push(performance.now() - started);
}
assert(typeof applicationSession?.accessToken === "string", "application sign-in did not succeed");
const accessToken = applicationSession.accessToken;

const controlSamples = [];
if (!reuseFixture) {
  for (let index = 0; index < 10; index += 1) {
    const started = performance.now();
    const response = await fetchJson(`${origin}${collectionBase}`, {
      headers: developerHeaders(),
    });
    assert(response.status === 200, "authenticated control operation failed");
    controlSamples.push(performance.now() - started);
  }
}

const expectedDocuments = new Map();
const acknowledgedDocuments = new Set();
const pushSamples = [];
for (let batch = 0; batch < 4; batch += 1) {
  const rows = [];
  for (let index = 0; index < 8; index += 1) {
    const id = `hosted-benchmark-${randomUUID()}`;
    const value = `batch-${batch}-document-${index}`;
    const mutationId = `hosted_benchmark_mutation_${randomUUID().replaceAll("-", "")}`;
    expectedDocuments.set(id, value);
    rows.push({
      mutationId,
      assumedMasterState: null,
      newDocumentState: { id, owner_id: state.applicationUserId, value },
    });
  }
  const started = performance.now();
  const response = await fetchJson(`${origin}${collectionBase}/replication/push`, {
    method: "POST",
    headers: applicationHeaders(accessToken, {
      "Idempotency-Key": `hosted_benchmark_batch_${randomUUID().replaceAll("-", "")}`,
    }),
    body: { schemaVersion: 1, rows },
  });
  pushSamples.push(performance.now() - started);
  assert(response.status === 200, "RxDB push benchmark failed");
  assert(response.body?.outcomes?.length === rows.length, "RxDB push outcomes are incomplete");
  for (const outcome of response.body.outcomes) {
    if (outcome.status === "accepted") acknowledgedDocuments.add(outcome.mutationId);
  }
  assert(
    rows.every((row) => acknowledgedDocuments.has(row.mutationId)),
    "RxDB push did not acknowledge every benchmark mutation",
  );
}

const pullBeforeRecovery = await pullAll(accessToken, expectedDocuments);
const live = await firstLiveEvent(accessToken);

await delay(6_500);
const coldFunction = await invokeFunction(accessToken);
const warmFunction = await invokeFunction(accessToken);
assert(coldFunction.status === 200 && warmFunction.status === 200, "function benchmark failed");
assert(coldFunction.bodySha256 === warmFunction.bodySha256, "cold and warm function bodies differ");

const targetLoad = await runConcurrentPhase(targetConcurrency, targetConcurrency * 4, accessToken);
const headroomLoad = await runConcurrentPhase(
  headroomConcurrency,
  headroomConcurrency * 4,
  accessToken,
);
const saturationProbe = await runConcurrentPhase(
  saturationProbeConcurrency,
  saturationProbeConcurrency * 4,
  accessToken,
);
console.log(
  JSON.stringify({
    target: targetLoad.statusCounts,
    headroom: headroomLoad.statusCounts,
    saturation: saturationProbe.statusCounts,
  }),
);
assert(targetLoad.failedRequests === 0, "target concurrent load returned an error");
assert(headroomLoad.failedRequests === 0, "the required headroom load returned an error");
assert(saturationProbe.serverErrors === 0, "the saturation probe returned a server error");

const recoveryStarted = performance.now();
ssh(`sudo systemctl restart mako-data-plane.service
for attempt in $(seq 1 60); do
  if curl --silent --fail --max-time 2 http://127.0.0.1:8080/readyz >/dev/null; then
    exit 0
  fi
  sleep 0.25
done
exit 1`);
const recoveryMilliseconds = performance.now() - recoveryStarted;
const recoveredSession = await signIn();
const pullAfterRecovery = await pullAll(recoveredSession.accessToken, expectedDocuments);

const acknowledgedWriteLoss = expectedDocuments.size - pullAfterRecovery.matchedDocuments;
const integrityFailures = pullAfterRecovery.integrityFailures;
const capacityHeadroomAtTargetLoadPercent =
  ((headroomConcurrency - targetConcurrency) / targetConcurrency) * 100;
assert(acknowledgedWriteLoss === 0, "an acknowledged document was lost after recovery");
assert(integrityFailures === 0, "a recovered document failed integrity verification");
assert(capacityHeadroomAtTargetLoadPercent >= 30, "target-load headroom is below 30 percent");
assertSelectedRelease(manifest.releaseDigest);

writeJson(evidencePath, {
  schemaVersion: 1,
  recordedAt: new Date().toISOString(),
  environment: "public-beta",
  vmId: 124,
  publicOrigin: origin,
  planHash: manifest.planHash,
  releaseDigest: manifest.releaseDigest,
  publicAdmissionMode: "restricted-pre-gate",
  qualificationMode: reuseFixture ? "existing-isolated-fixture" : "developer-managed-fixture",
  managementControlCoveredBy: reuseFixture
    ? "docs/evidence/public-beta-hosted-qualification.json"
    : null,
  target: {
    concurrency: targetConcurrency,
    qualifiedHeadroomConcurrency: headroomConcurrency,
    saturationProbeConcurrency,
    capacityHeadroomAtTargetLoadPercent,
  },
  latencyMilliseconds: {
    authSignIn: summarize(authSamples),
    rxdbPushBatch: summarize(pushSamples),
    rxdbPullBeforeRecovery: pullBeforeRecovery.latency,
    rxdbLiveFirstEvent: live.elapsedMilliseconds,
    controlRead: controlSamples.length === 0 ? null : summarize(controlSamples),
    coldFunction: coldFunction.elapsedMilliseconds,
    warmFunction: warmFunction.elapsedMilliseconds,
    recoveryToReady: recoveryMilliseconds,
    rxdbPullAfterRecovery: pullAfterRecovery.latency,
  },
  workload: {
    acknowledgedDocuments: expectedDocuments.size,
    acknowledgedWriteLoss,
    integrityFailures,
    liveEvent: live.event,
    functionResponseBytes: warmFunction.bodyBytes,
    functionBodyStableAcrossColdAndWarm: true,
  },
  concurrentLoad: {
    target: targetLoad,
    requiredHeadroom: headroomLoad,
    saturationProbe,
    saturationConcurrencyLowerBound:
      saturationProbe.failedRequests === 0 ? saturationProbeConcurrency : headroomConcurrency,
  },
  passed: true,
});
console.log(`public-beta hosted benchmark passed: ${evidencePath}`);

function issueDeveloperToken() {
  const privateRoot = resolve(repositoryRoot, ".local/qualification");
  const temporary = mkdtempSync(resolve(privateRoot, "benchmark-session-"));
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

async function signIn() {
  const response = await fetchJson(`${origin}${tenantBase}/auth/signin`, {
    method: "POST",
    headers: { "X-Mako-Key": state.publicProjectKey },
    body: { email: state.applicationEmail, password: state.applicationPassword },
  });
  assert(response.status === 200, "application sign-in benchmark failed");
  assert(typeof response.body?.accessToken === "string", "application access token is missing");
  return response.body;
}

async function pullAll(token, expected) {
  const started = performance.now();
  const found = new Map();
  let checkpoint = null;
  for (let page = 0; page < 1_000; page += 1) {
    let response;
    for (let attempt = 0; attempt < 6; attempt += 1) {
      response = await fetchJson(`${origin}${collectionBase}/replication/pull`, {
        method: "POST",
        headers: applicationHeaders(token),
        body: { checkpoint, schemaVersion: 1, batchSize: 100 },
        expected: [200, 429],
      });
      if (response.status === 200) break;
      // Honor the platform's own retry contract in full: the rate window is
      // a minute, so a wait capped below it can burn every attempt inside
      // one exhausted window and report a healthy limiter as a failure.
      const retryAfterSeconds = Number(response.headers.get("retry-after") ?? "1");
      await delay(Math.min(Math.max(retryAfterSeconds, 0.25), 61) * 1_000);
    }
    assert(response?.status === 200, "RxDB pull remained rate limited after bounded retries");
    assert(Array.isArray(response.body?.documents), "RxDB pull documents are invalid");
    assert(typeof response.body?.checkpoint === "string", "RxDB pull checkpoint is invalid");
    for (const document of response.body.documents) {
      if (expected.has(document.id)) found.set(document.id, document);
    }
    checkpoint = response.body.checkpoint;
    if (response.body.documents.length === 0) break;
    assert(page < 999, "RxDB pull pagination did not terminate");
  }
  let integrityFailures = 0;
  for (const [id, value] of expected) {
    const document = found.get(id);
    if (
      document !== undefined &&
      (document.value !== value || document.owner_id !== state.applicationUserId)
    ) {
      integrityFailures += 1;
    }
  }
  return {
    matchedDocuments: found.size,
    integrityFailures,
    latency: performance.now() - started,
  };
}

async function firstLiveEvent(token) {
  const started = performance.now();
  const response = await fetch(`${origin}${collectionBase}/replication/stream?schemaVersion=1`, {
    headers: applicationHeaders(token, { Accept: "text/event-stream" }),
    signal: AbortSignal.timeout(10_000),
  });
  assert(response.status === 200, "RxDB live benchmark failed");
  const reader = response.body?.getReader();
  assert(reader !== undefined, "RxDB live response has no body");
  let bytes = Buffer.alloc(0);
  try {
    while (bytes.length < 1024 * 1024) {
      const next = await reader.read();
      if (next.done) break;
      bytes = Buffer.concat([bytes, Buffer.from(next.value)]);
      const boundary = bytes.indexOf("\n\n");
      if (boundary >= 0) {
        const frame = bytes.subarray(0, boundary + 2).toString("utf8");
        const event = /^event: ([a-z_]+)$/mu.exec(frame)?.[1] ?? null;
        assert(event !== null, "RxDB live frame has no event name");
        return { event, elapsedMilliseconds: performance.now() - started };
      }
    }
  } finally {
    await reader.cancel();
  }
  throw new Error("RxDB live benchmark emitted no event");
}

async function invokeFunction(token) {
  const started = performance.now();
  const response = await fetch(`${origin}${functionPath}`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  const body = Buffer.from(await response.arrayBuffer());
  return {
    status: response.status,
    bodyBytes: body.length,
    bodySha256: createHash("sha256").update(body).digest("hex"),
    elapsedMilliseconds: performance.now() - started,
  };
}

async function runConcurrentPhase(concurrency, requests, token) {
  const samples = { authUser: [], control: [], function: [], pull: [] };
  const operations = reuseFixture
    ? ["authUser", "authUser", "function", "pull"]
    : ["authUser", "control", "function", "pull"];
  const statuses = [];
  let nextRequest = 0;
  const started = performance.now();
  await Promise.all(
    Array.from({ length: concurrency }, async () => {
      while (true) {
        const index = nextRequest;
        nextRequest += 1;
        if (index >= requests) return;
        const operation = operations[index % operations.length];
        const sampleStarted = performance.now();
        const response = await loadRequest(operation, token);
        samples[operation].push(performance.now() - sampleStarted);
        statuses.push({ operation, status: response.status });
      }
    }),
  );
  return {
    concurrency,
    requests,
    elapsedMilliseconds: performance.now() - started,
    requestsPerSecond: requests / ((performance.now() - started) / 1_000),
    failedRequests: statuses.filter(({ status }) => status !== 200).length,
    serverErrors: statuses.filter(({ status }) => status >= 500).length,
    rateLimitedRequests: statuses.filter(({ status }) => status === 429).length,
    statusCounts: Object.fromEntries(
      Object.keys(samples).map((operation) => [
        operation,
        Object.fromEntries(
          [
            ...new Set(
              statuses
                .filter((entry) => entry.operation === operation)
                .map((entry) => entry.status),
            ),
          ]
            .sort((left, right) => left - right)
            .map((status) => [
              String(status),
              statuses.filter((entry) => entry.operation === operation && entry.status === status)
                .length,
            ]),
        ),
      ]),
    ),
    latencyMilliseconds: Object.fromEntries(
      Object.entries(samples).map(([name, values]) => [
        name,
        values.length === 0 ? null : summarize(values),
      ]),
    ),
  };
}

async function loadRequest(operation, token) {
  if (operation === "authUser") {
    return await fetchJson(`${origin}${tenantBase}/auth/user`, {
      headers: applicationHeaders(token),
      expected: [200, 429, 503],
    });
  }
  if (operation === "control") {
    return await fetchJson(`${origin}${collectionBase}`, {
      headers: developerHeaders(),
      expected: [200, 429, 503],
    });
  }
  if (operation === "pull") {
    return await fetchJson(`${origin}${collectionBase}/replication/pull`, {
      method: "POST",
      headers: applicationHeaders(token),
      body: { checkpoint: null, schemaVersion: 1, batchSize: 10 },
      expected: [200, 429, 503],
    });
  }
  const response = await fetch(`${origin}${functionPath}`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  await response.arrayBuffer();
  return { status: response.status };
}

async function fetchJson(url, options = {}) {
  const headers = { Accept: "application/json", ...(options.headers ?? {}) };
  const response = await fetch(url, {
    method: options.method ?? "GET",
    headers:
      options.body === undefined ? headers : { ...headers, "Content-Type": "application/json" },
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
    signal: AbortSignal.timeout(30_000),
  });
  const text = await response.text();
  const body = text === "" ? null : JSON.parse(text);
  const expected = options.expected ?? [200];
  assert(
    expected.includes(response.status),
    `${new URL(url).pathname} returned HTTP ${response.status}`,
  );
  return { status: response.status, body, headers: response.headers };
}

function applicationHeaders(token, additional = {}) {
  return {
    Authorization: `Bearer ${token}`,
    "X-Mako-Key": state.publicProjectKey,
    ...additional,
  };
}

function developerHeaders() {
  assert(developerToken !== null, "developer token is unavailable in fixture-reuse mode");
  return { Authorization: `Bearer ${developerToken}` };
}

function summarize(values) {
  assert(values.length > 0, "latency sample is empty");
  const sorted = [...values].sort((left, right) => left - right);
  return {
    samples: sorted.length,
    p50: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
    maximum: sorted.at(-1),
  };
}

function percentile(sorted, fraction) {
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
}
