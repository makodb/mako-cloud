#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  assert,
  assertSelectedRelease,
  parseOptions,
  repositoryRoot,
  runLogged,
  ssh,
  writeJson,
} from "./public-beta-hosted-lib.js";

const options = parseOptions(process.argv.slice(2));
const manifest = JSON.parse(
  readFileSync(resolve(repositoryRoot, "docs/evidence/public-beta-release-manifest.json"), "utf8"),
);
const releaseDigest = options.releaseDigest ?? manifest.releaseDigest;
const output = options.output ?? ".local/qualification/public-beta-hosted-security.json";
const startedAt = new Date().toISOString();
const edgeEnvironment = {
  MAKO_RUN_EDGE_RUNTIME_TESTS: "1",
  MAKO_EDGE_TEST_ENGINE: process.env.MAKO_EDGE_TEST_ENGINE ?? "podman",
};
if (process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON !== undefined) {
  edgeEnvironment.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON = process.env.MAKO_EDGE_TEST_ENGINE_PREFIX_JSON;
}
assert(
  releaseDigest === manifest.releaseDigest,
  "requested digest differs from the release manifest",
);
assertSelectedRelease(releaseDigest);

const suites = [
  ["auth", "bash", ["scripts/run-auth-security-qualification.sh"]],
  ["policy", "bash", ["scripts/run-policy-security-qualification.sh"]],
  ["rxdb-chaos", "bash", ["scripts/run-rxdb-chaos-qualification.sh"]],
  ["tenant-boundaries", "bash", ["scripts/run-tenant-boundary-qualification.sh"]],
  ["edge-runtime", "bash", ["scripts/run-edge-security-qualification.sh"], edgeEnvironment],
  ["dependency-pins", "npm", ["run", "validate:public-beta-containers"]],
  ["production-rocksdb", "bash", ["scripts/run-production-rocksdb-qualification.sh"]],
  ["rational", "bash", ["scripts/run-rational-smoke-qualification.sh"]],
];
const results = [];
for (const [name, command, args, environment] of suites) {
  const log = `.local/qualification/public-beta-${releaseDigest.slice(0, 12)}-${name}.log`;
  process.stdout.write(`running ${name} qualification...\n`);
  runLogged(command, args, log, environment);
  results.push({ name, passed: true, log });
}

const hostedProbe = JSON.parse(
  ssh(`python3 - <<'PY'
import json
import urllib.error
import urllib.request

probes = [
  ("data-readiness", "GET", "http://127.0.0.1:8080/readyz", 200),
  ("control-readiness", "GET", "http://127.0.0.1:8081/readyz", 200),
  ("edge-readiness", "GET", "http://127.0.0.1:8082/readyz", 200),
  ("telemetry-readiness", "GET", "http://127.0.0.1:9465/readyz", 200),
  ("auth-gate", "POST", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/auth/signin", 400),
  ("rxdb-pull-gate", "POST", "http://127.0.0.1:8080/v1/projects/prj_beta/environments/env_beta/collections/documents/replication/pull", 400),
  ("control-gate", "GET", "http://127.0.0.1:8081/v1/projects", 400),
  ("function-resolution", "GET", "http://127.0.0.1:8082/prj_beta/functions/v1/missing", 503),
]
results = []
for name, method, url, expected in probes:
  request = urllib.request.Request(url, data=b"{}" if method == "POST" else None, method=method)
  request.add_header("content-type", "application/json")
  try:
    with urllib.request.urlopen(request, timeout=5) as response:
      status = response.status
  except urllib.error.HTTPError as error:
    status = error.code
  results.append({"name": name, "status": status, "expected": expected, "passed": status == expected})
print(json.dumps(results))
PY`),
);
assert(
  hostedProbe.every((probe) => probe.passed),
  "a hosted composition probe failed",
);
assertSelectedRelease(releaseDigest);
const serviceState = ssh(
  "systemctl is-active mako-data-plane mako-control-plane mako-edge-gateway mako-telemetry-query",
)
  .split("\n")
  .filter(Boolean);
assert(
  serviceState.length === 4 && serviceState.every((state) => state === "active"),
  "a Mako service is inactive",
);

// What the runtime is actually running. The main worker authenticates
// deployments, supplies the SDK module to every user worker, and decides each
// worker's permissions -- so a host whose worker is not the release's is a
// host running tenant code under rules no digest describes. It used to reach
// the host only through the provisioning role, and the beta consequently ran a
// worker four releases old without anything noticing.
const runtimeWorkers = manifest.artifacts.files.filter((artifact) =>
  artifact.path.startsWith("runtime-main/"),
);
assert(runtimeWorkers.length > 0, "the release carries no edge runtime worker");
const installedWorkers = ssh(
  `sudo sha256sum ${runtimeWorkers
    .map(
      (artifact) =>
        `/home/mako-runtime/.config/mako-cloud/edge-runtime/main/${artifact.path.slice("runtime-main/".length)}`,
    )
    .join(" ")}`,
)
  .split("\n")
  .filter(Boolean)
  .map((line) => line.split(/\s+/u)[0]);
assert(
  installedWorkers.length === runtimeWorkers.length &&
    runtimeWorkers.every((artifact, index) => artifact.sha256 === installedWorkers[index]),
  "the installed edge runtime worker is not the release's",
);
const runtimeContainer = ssh(
  "sudo -u mako-runtime sh -c 'cd /home/mako-runtime && XDG_RUNTIME_DIR=/run/user/$(id -u mako-runtime)" +
    " systemctl --user is-active mako-edge-runtime.service'",
).trim();
assert(
  runtimeContainer === "active",
  `the edge runtime is not running the release's worker (unit is ${runtimeContainer})`,
);

const evidence = {
  schemaVersion: 1,
  startedAt,
  completedAt: new Date().toISOString(),
  environment: "public-beta",
  vmId: 124,
  address: "130.245.173.11",
  publicOrigin: "https://cloud-test.makodb.com",
  planHash: manifest.planHash,
  releaseDigest,
  sourceDigest: manifest.source.digest,
  runtimeDigest: manifest.runtime.digest,
  dependencyDigest: manifest.dependencies.digest,
  artifactDigest: manifest.artifacts.digest,
  selectedReleaseVerifiedBeforeAndAfter: true,
  publicAdmissionEnabled: false,
  suites: results,
  hostedCompositionProbes: hostedProbe,
  servicesActive: true,
  passed: true,
  scope:
    "security and durability suites from the exact deployed source plus private loopback composition probes; trusted public HTTPS remains a separate gate",
};
writeJson(output, evidence);
console.log(`hosted qualification passed; evidence: ${output}`);
