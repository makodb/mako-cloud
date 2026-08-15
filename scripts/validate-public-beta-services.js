#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const read = (path) => readFile(resolve(root, path), "utf8");
const [runtimeTasks, runtimeUnit, storageTasks, releaseBuilder, releaseManager, telemetryService] =
  await Promise.all([
    read("infra/ansible/roles/runtime/tasks/main.yml"),
    read("infra/ansible/roles/runtime/templates/mako-service.service.j2"),
    read("infra/ansible/roles/storage/tasks/main.yml"),
    read("scripts/build-public-beta-release.js"),
    read("infra/ansible/roles/runtime/files/mako-release"),
    read("services/mako-telemetry-query/src/main.rs"),
  ]);

for (const source of [runtimeTasks, storageTasks, releaseBuilder, releaseManager]) {
  assert(
    source.includes("mako-telemetry-query"),
    "telemetry-query is absent from a release/deploy boundary",
  );
}
for (const invariant of [
  "MAKO_TELEMETRY_QUERY_BIND=127.0.0.1:9465",
  "LoadCredential=telemetry-authorization:/etc/mako/credentials/internal-auth",
  "MAKO_TELEMETRY_DATABASE_PATH={{ item.database_path }}",
  "SocketBindAllow=ipv4:tcp:{{ item.port }}",
]) {
  assert(runtimeUnit.includes(invariant), `telemetry unit omits: ${invariant}`);
}
for (const invariant of [
  "TELEMETRY_HEALTH_PATH",
  "TELEMETRY_QUERY_PATH",
  "TELEMETRY_INGEST_PATH",
  "compare_and_write",
  "Durability::Sync",
  "redact_record",
  "decode_cursor",
]) {
  assert(telemetryService.includes(invariant), `telemetry service omits: ${invariant}`);
}
assert(
  storageTasks.includes("rocksdb/telemetry-query") &&
    storageTasks.includes('database_id: "mako-telemetry-query-{{ mako_storage_region }}"'),
  "telemetry RocksDB path is not exclusively provisioned",
);

console.log(
  "validated native public-beta telemetry service release, loopback unit, credential, and RocksDB ownership",
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
