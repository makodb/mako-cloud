#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

import { evaluateHostedBenchmark, GATED_LATENCY_PATHS } from "./public-beta-benchmark-gate-lib.js";
import {
  assert,
  assertSelectedRelease,
  parseOptions,
  qualificationDeveloper,
  repositoryRoot,
  ssh,
  writeJson,
} from "./public-beta-hosted-lib.js";

// Usage: run-public-beta-hosted-benchmark.js [--reuse-fixture true|false] [--samples N]
//   [--maximum-steady-state-minutes M]
//
// The gated latencies come from a steady-state phase: sequential rounds in which every gated
// path is sampled once, repeated until each path has --samples samples (default: the gate's
// minimumSteadyStateSamples, 100), or until the time bound. Recovery timings are recorded
// separately and are not gated.
const origin = "https://cloud-test.makodb.com";
const options = parseOptions(process.argv.slice(2));
const reuseFixture = options.reuseFixture === "true";
assert(
  options.reuseFixture === undefined || ["true", "false"].includes(options.reuseFixture),
  "--reuse-fixture must be true or false",
);
const statePath = resolve(repositoryRoot, ".local/qualification/public-beta-fixture.json");
const manifestPath = resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json");
const gatesPath = resolve(repositoryRoot, "docs/release-gates.json");
const secretPath = resolve(repositoryRoot, ".local/public-beta-secrets/internal-auth");
const sessionBinary = resolve(repositoryRoot, "target/release/mako-control-session");
const evidencePath = "docs/evidence/public-beta-hosted-benchmark.json";
const targetConcurrency = 16;
const headroomConcurrency = 24;
const saturationProbeConcurrency = 64;
const gateThresholds = JSON.parse(readFileSync(gatesPath, "utf8")).stages.find(
  (stage) => stage.id === "single-region-beta",
).thresholds;
const steadyStateSamples = integerOption(
  "samples",
  gateThresholds.minimumSteadyStateSamples,
  1,
  100_000,
);
const maximumSteadyStateMinutes = integerOption("maximumSteadyStateMinutes", 45, 1, 24 * 60);
// The edge runtime evicts an idle function before this, so the next invocation is cold.
const coldIdleMilliseconds = 6_500;
const pushBatchRows = 8;

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

const steadyState = Object.fromEntries(GATED_LATENCY_PATHS.map(({ evidence }) => [evidence, []]));
const rateLimitedResponses = Object.fromEntries(
  GATED_LATENCY_PATHS.map(({ evidence }) => [evidence, 0]),
);
let liveEventName = null;

// Warm-up: a session, a caught-up pull checkpoint and a resident function. None is a sample.
const expectedDocuments = new Map();
const acknowledgedDocuments = new Set();
let applicationSession = (await sampled(null, () => signInAttempt())).body;
const initialCatchUp = await pullAll(applicationSession.accessToken, expectedDocuments);
let pullCheckpoint = initialCatchUp.checkpoint;
const warmUpFunction = await sampled(null, () => invokeFunction(applicationSession.accessToken));
assert(warmUpFunction.status === 200, "function warm-up failed");
let lastFunctionCompletedAt = performance.now();
const functionBodySha256 = warmUpFunction.bodySha256;
let functionResponseBytes = warmUpFunction.bodyBytes;

let steadyStateIntegrityFailures = 0;
let rounds = 0;
const steadyStateStartedAt = hostSeconds();
const steadyStateDeadline = performance.now() + maximumSteadyStateMinutes * 60_000;
while (true) {
  applicationSession = (await sampled("authSignIn", () => signInAttempt())).body;
  const token = applicationSession.accessToken;

  const rows = [];
  for (let index = 0; index < pushBatchRows; index += 1) {
    const id = `hosted-benchmark-${randomUUID()}`;
    const value = `round-${rounds}-document-${index}`;
    const mutationId = `hosted_benchmark_mutation_${randomUUID().replaceAll("-", "")}`;
    expectedDocuments.set(id, value);
    rows.push({
      mutationId,
      assumedMasterState: null,
      newDocumentState: { id, owner_id: state.applicationUserId, value },
    });
  }
  // A retried batch reuses its idempotency key, so throttling cannot duplicate writes.
  const idempotencyKey = `hosted_benchmark_batch_${randomUUID().replaceAll("-", "")}`;
  const pushed = await sampled("rxdbPushBatch", () =>
    fetchJson(`${origin}${collectionBase}/replication/push`, {
      method: "POST",
      headers: applicationHeaders(token, { "Idempotency-Key": idempotencyKey }),
      body: { schemaVersion: 1, rows },
      expected: [200, 429],
    }),
  );
  assert(pushed.body?.outcomes?.length === rows.length, "RxDB push outcomes are incomplete");
  for (const outcome of pushed.body.outcomes) {
    if (outcome.status === "accepted") acknowledgedDocuments.add(outcome.mutationId);
  }
  assert(
    rows.every((row) => acknowledgedDocuments.has(row.mutationId)),
    "RxDB push did not acknowledge every benchmark mutation",
  );

  // Steady-state pull: a caught-up client fetching the changes since its checkpoint.
  const pulled = await sampled("rxdbPull", () =>
    fetchJson(`${origin}${collectionBase}/replication/pull`, {
      method: "POST",
      headers: applicationHeaders(token),
      body: { checkpoint: pullCheckpoint, schemaVersion: 1, batchSize: 100 },
      expected: [200, 429],
    }),
  );
  assert(Array.isArray(pulled.body?.documents), "RxDB pull documents are invalid");
  assert(typeof pulled.body?.checkpoint === "string", "RxDB pull checkpoint is invalid");
  for (const document of pulled.body.documents) {
    const value = expectedDocuments.get(document.id);
    if (
      value !== undefined &&
      (document.value !== value || document.owner_id !== state.applicationUserId)
    ) {
      steadyStateIntegrityFailures += 1;
    }
  }
  pullCheckpoint = pulled.body.checkpoint;

  await sampled("rxdbLiveFirstEvent", () => firstLiveEvent(token));

  const idle = lastFunctionCompletedAt + coldIdleMilliseconds - performance.now();
  if (idle > 0) await delay(idle);
  const cold = await sampled("coldFunction", () => invokeFunction(token));
  const warm = await sampled("warmFunction", () => invokeFunction(token));
  lastFunctionCompletedAt = performance.now();
  assert(cold.status === 200 && warm.status === 200, "function benchmark failed");
  assert(
    cold.bodySha256 === functionBodySha256 && warm.bodySha256 === functionBodySha256,
    "cold and warm function bodies differ",
  );
  functionResponseBytes = warm.bodyBytes;

  rounds += 1;
  if (performance.now() >= steadyStateDeadline) break;
  if (rounds >= steadyStateSamples) break;
}
const steadyStateCompletedAt = hostSeconds();
console.log(
  JSON.stringify({
    steadyStateRounds: rounds,
    rateLimitedResponses,
  }),
);

const accessToken = applicationSession.accessToken;
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
const recoveredSession = (await sampled(null, () => signInAttempt())).body;
const pullAfterRecovery = await pullAll(recoveredSession.accessToken, expectedDocuments);

const acknowledgedWriteLoss = expectedDocuments.size - pullAfterRecovery.matchedDocuments;
const integrityFailures = pullAfterRecovery.integrityFailures + steadyStateIntegrityFailures;
const capacityHeadroomAtTargetLoadPercent =
  ((headroomConcurrency - targetConcurrency) / targetConcurrency) * 100;
assert(acknowledgedWriteLoss === 0, "an acknowledged document was lost after recovery");
assert(integrityFailures === 0, "a recovered document failed integrity verification");
assert(capacityHeadroomAtTargetLoadPercent >= 30, "target-load headroom is below 30 percent");
assertSelectedRelease(manifest.releaseDigest);

const benchmarkEvidence = {
  schemaVersion: 2,
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
  // The gated steady-state paths, each summarized as samples, p50, p95 and maximum.
  latencyMilliseconds: {
    ...Object.fromEntries(
      GATED_LATENCY_PATHS.map(({ evidence }) => [evidence, summarize(steadyState[evidence])]),
    ),
    controlRead: controlSamples.length === 0 ? null : summarize(controlSamples),
  },
  steadyState: {
    startedAt: isoFromSeconds(steadyStateStartedAt),
    completedAt: isoFromSeconds(steadyStateCompletedAt),
    clock: "beta host",
    rounds,
    requiredSamplesPerPath: steadyStateSamples,
    coldIdleMilliseconds,
    pushBatchRows,
    rateLimitedResponses,
    initialCatchUpPullMilliseconds: initialCatchUp.latency,
  },
  // Recovery belongs to the recovery drill, not the latency gate.
  recovery: {
    toReadyMilliseconds: recoveryMilliseconds,
    rxdbPullAfterRecoveryMilliseconds: pullAfterRecovery.latency,
  },
  workload: {
    acknowledgedDocuments: expectedDocuments.size,
    acknowledgedWriteLoss,
    integrityFailures,
    liveEvent: liveEventName,
    functionResponseBytes,
    functionBodyStableAcrossColdAndWarm: true,
  },
  concurrentLoad: {
    target: targetLoad,
    requiredHeadroom: headroomLoad,
    saturationProbe,
    saturationConcurrencyLowerBound:
      saturationProbe.failedRequests === 0 ? saturationProbeConcurrency : headroomConcurrency,
  },
  // The run completed with no lost write, no integrity failure and no failed target or headroom
  // request. Whether the release passes the gate is computed from this evidence by
  // validate-release-gates.js.
  passed: true,
};
writeJson(evidencePath, benchmarkEvidence);
const gate = evaluateHostedBenchmark(benchmarkEvidence, gateThresholds);
console.log(`recorded public-beta hosted benchmark: ${evidencePath}`);
console.log(JSON.stringify({ gate: gate.status, blockers: gate.blockers }, null, 2));

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

// Times one served request of a path. A throttled attempt is not a latency sample of the path:
// it is counted, the platform's retry advice is honored, and the request is retried.
async function sampled(path, attempt) {
  for (let tries = 0; tries < 6; tries += 1) {
    const started = performance.now();
    const response = await attempt();
    const elapsed = response.elapsedMilliseconds ?? performance.now() - started;
    if (response.status !== 429) {
      if (path !== null) steadyState[path].push(elapsed);
      return response;
    }
    if (path !== null) rateLimitedResponses[path] += 1;
    await delay(retryDelayMilliseconds(response));
  }
  throw new Error(`${path ?? "warm-up request"} remained rate limited after bounded retries`);
}

function retryDelayMilliseconds(response) {
  // A throttled request says how long to wait in its error envelope (retry.afterMs); the
  // Retry-After header exists only on the transport's overload path. The rate window is a
  // minute, so the wait is capped just above it.
  const advisedMilliseconds = Number(response.body?.error?.retry?.afterMs);
  const retryAfterSeconds = Number.isFinite(advisedMilliseconds)
    ? advisedMilliseconds / 1_000
    : Number(response.headers?.get("retry-after") ?? "1");
  return Math.min(Math.max(retryAfterSeconds, 0.25), 61) * 1_000;
}

async function signInAttempt() {
  const response = await fetchJson(`${origin}${tenantBase}/auth/signin`, {
    method: "POST",
    headers: { "X-Mako-Key": state.publicProjectKey },
    body: { email: state.applicationEmail, password: state.applicationPassword },
    expected: [200, 429],
  });
  if (response.status === 200) {
    assert(typeof response.body?.accessToken === "string", "application access token is missing");
  }
  return response;
}

function hostSeconds() {
  const seconds = Number(ssh("date +%s.%N"));
  assert(Number.isFinite(seconds) && seconds > 0, "the beta host clock is unreadable");
  return seconds;
}

function isoFromSeconds(seconds) {
  return new Date(seconds * 1_000).toISOString();
}

function integerOption(name, fallback, minimum, maximum) {
  const value = options[name] === undefined ? fallback : Number(options[name]);
  assert(
    Number.isSafeInteger(value) && value >= minimum && value <= maximum,
    `--${name.replace(/[A-Z]/gu, (letter) => `-${letter.toLowerCase()}`)} must be an integer from ${minimum} to ${maximum}`,
  );
  return value;
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
      await delay(retryDelayMilliseconds(response));
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
    checkpoint,
    latency: performance.now() - started,
  };
}

async function firstLiveEvent(token) {
  const started = performance.now();
  const response = await fetch(`${origin}${collectionBase}/replication/stream?schemaVersion=1`, {
    headers: applicationHeaders(token, { Accept: "text/event-stream" }),
    signal: AbortSignal.timeout(10_000),
  });
  if (response.status === 429) {
    await response.body?.cancel();
    return { status: 429, headers: response.headers };
  }
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
        liveEventName = event;
        return { status: 200, event, elapsedMilliseconds: performance.now() - started };
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
    headers: response.headers,
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
