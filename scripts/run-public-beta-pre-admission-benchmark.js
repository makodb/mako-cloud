#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  assert,
  assertSelectedRelease,
  parseOptions,
  repositoryRoot,
  ssh,
  writeJson,
} from "./public-beta-hosted-lib.js";

const options = parseOptions(process.argv.slice(2));
const manifest = JSON.parse(
  readFileSync(resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json"), "utf8"),
);
const releaseDigest = options.releaseDigest ?? manifest.releaseDigest;
const output = options.output ?? ".local/qualification/public-beta-pre-admission-benchmark.json";
assertSelectedRelease(releaseDigest);

const result = JSON.parse(
  ssh(`python3 - <<'PY'
import concurrent.futures
import json
import math
import time
import urllib.error
import urllib.request

routes = [
  ("auth-gate", "POST", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/auth/signin", 400),
  ("rxdb-pull-gate", "POST", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/collections/documents/replication/pull", 400),
  ("rxdb-push-gate", "POST", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/collections/documents/replication/push", 400),
  ("rxdb-live-gate", "GET", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/collections/documents/replication/live", 404),
  ("control-gate", "GET", "http://127.0.0.1:8081/v1/projects", 400),
  ("function-resolution", "GET", "http://127.0.0.1:8082/prj_beta/functions/v1/missing", 503),
]

def one(route):
  name, method, url, expected = route
  request = urllib.request.Request(url, data=b"{}" if method == "POST" else None, method=method)
  request.add_header("content-type", "application/json")
  started = time.perf_counter_ns()
  try:
    with urllib.request.urlopen(request, timeout=5) as response:
      status = response.status
  except urllib.error.HTTPError as error:
    status = error.code
  return name, status, expected, (time.perf_counter_ns() - started) / 1_000_000

samples = []
for route in routes:
  for _ in range(30):
    samples.append(one(route))
with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
  concurrent_samples = list(executor.map(one, [routes[index % len(routes)] for index in range(600)]))

def summarize(rows):
  output = []
  for route in routes:
    name = route[0]
    selected = sorted(row[3] for row in rows if row[0] == name)
    statuses = sorted(set(row[1] for row in rows if row[0] == name))
    p95 = selected[max(0, math.ceil(len(selected) * 0.95) - 1)]
    output.append({
      "name": name,
      "samples": len(selected),
      "p50Milliseconds": selected[len(selected) // 2],
      "p95Milliseconds": p95,
      "maximumMilliseconds": selected[-1],
      "statuses": statuses,
      "expectedStatus": route[3],
      "statusIntegrityPassed": statuses == [route[3]],
    })
  return output

print(json.dumps({
  "sequential": summarize(samples),
  "concurrent16": summarize(concurrent_samples),
  "concurrentRequests": len(concurrent_samples),
}))
PY`),
);
assert(
  [...result.sequential, ...result.concurrent16].every((entry) => entry.statusIntegrityPassed),
  "a pre-admission route returned an unexpected status",
);
assertSelectedRelease(releaseDigest);

writeJson(output, {
  schemaVersion: 1,
  recordedAt: new Date().toISOString(),
  environment: "public-beta",
  vmId: 124,
  publicOrigin: "https://cloud-test.makodb.com",
  planHash: manifest.planHash,
  releaseDigest,
  publicAdmissionEnabled: false,
  privateLoopbackRouteLoad: result,
  acknowledgedWriteLoss: null,
  integrityFailures: null,
  capacityHeadroomAtTargetLoadPercent: null,
  endToEndHttpsLatency: null,
  warmFunctionSuccessLatency: null,
  coldFunctionSuccessLatency: null,
  gateQualified: false,
  blockers: [
    "trusted HTTPS admission is not approved, so end-to-end public-origin latency is unmeasured",
    "no qualification tenant credentials or successful RxDB/function workload were admitted",
    "the beta target load and saturation point have not been established",
  ],
});
console.log(`pre-admission route-load benchmark passed; beta load gate remains blocked: ${output}`);
