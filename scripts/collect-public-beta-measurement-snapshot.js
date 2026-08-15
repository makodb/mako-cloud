#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
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
const output = options.output ?? ".local/qualification/public-beta-measurement-snapshot.json";
assertSelectedRelease(releaseDigest);

const guest = JSON.parse(
  ssh(`python3 - <<'PY'
import json
import os
import shutil
import time

def read_int(path):
  try:
    with open(path, "r", encoding="utf-8") as handle:
      return int(handle.read().strip())
  except (FileNotFoundError, ValueError):
    return None

disk = shutil.disk_usage("/srv/mako-data")
network = {}
for name in os.listdir("/sys/class/net"):
  if name == "lo":
    continue
  network[name] = {
    "receivedBytes": read_int(f"/sys/class/net/{name}/statistics/rx_bytes"),
    "transmittedBytes": read_int(f"/sys/class/net/{name}/statistics/tx_bytes"),
    "receivedErrors": read_int(f"/sys/class/net/{name}/statistics/rx_errors"),
    "transmittedErrors": read_int(f"/sys/class/net/{name}/statistics/tx_errors"),
  }
with open("/proc/uptime", "r", encoding="utf-8") as handle:
  uptime = float(handle.read().split()[0])
print(json.dumps({
  "recordedEpochSeconds": int(time.time()),
  "uptimeSeconds": uptime,
  "storage": {"totalBytes": disk.total, "usedBytes": disk.used, "availableBytes": disk.free},
  "network": network,
}))
PY`),
);
const prometheus = JSON.parse(
  ssh(`python3 - <<'PY'
import json
import urllib.parse
import urllib.request

queries = {
  "servicesReady": "sum(mako_service_readiness)",
  "dependenciesActive": "sum(mako_podman_dependency_active)",
  "backupVerified": "min(mako_storage_backup_remote_verified)",
  "backupAgeMaximumSeconds": "max(time() - mako_storage_backup_last_success_unixtime)",
  "cpuBusyRatio": '1 - avg(rate(node_cpu_seconds_total{mode="idle"}[5m]))',
  "memoryUsedRatio": "1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes",
}
results = {}
for name, query in queries.items():
  url = "http://127.0.0.1:9090/api/v1/query?" + urllib.parse.urlencode({"query": query})
  with urllib.request.urlopen(url, timeout=5) as response:
    payload = json.load(response)
  values = payload.get("data", {}).get("result", [])
  results[name] = float(values[0]["value"][1]) if values else None
print(json.dumps(results))
PY`),
);
assertSelectedRelease(releaseDigest);

writeJson(output, {
  schemaVersion: 1,
  recordedAt: new Date().toISOString(),
  environment: "public-beta",
  vmId: 124,
  address: "130.245.173.11",
  planHash: manifest.planHash,
  releaseDigest,
  guest,
  prometheus,
  publicAdmissionEnabled: false,
});
console.log(`recorded read-only public-beta measurement snapshot: ${output}`);
