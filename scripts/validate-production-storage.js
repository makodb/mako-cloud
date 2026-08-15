import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const manifest = await readFile(
  resolve(root, "infra/production/storage-statefulsets.yaml"),
  "utf8",
);
const statefulSets = manifest
  .split(/^---$/m)
  .filter((document) => /^kind: StatefulSet$/m.test(document));

if (statefulSets.length !== 3) {
  throw new Error("production must declare exactly three stateful storage owners");
}

const expected = new Map([
  ["mako-data-plane", "data-plane-rocksdb"],
  ["mako-control-plane", "control-plane-sqlite"],
  ["mako-edge-gateway", "edge-gateway-rocksdb"],
]);
const claims = new Set();
for (const document of statefulSets) {
  const name = document.match(/^ {2}name: (mako-(?:data-plane|control-plane|edge-gateway))$/m)?.[1];
  const claim = document.match(
    /^ {8}name: ((?:data-plane|edge-gateway)-rocksdb|control-plane-sqlite)$/m,
  )?.[1];
  if (name === undefined || claim !== expected.get(name) || claims.has(claim)) {
    throw new Error("stateful services must use distinct named storage claims");
  }
  claims.add(claim);
  for (const required of [
    "  replicas: 1",
    "persistentVolumeClaimRetentionPolicy:",
    "whenDeleted: Retain",
    "whenScaled: Retain",
    'accessModes: ["ReadWriteOnce"]',
    "storageClassName: mako-retain-encrypted",
  ]) {
    if (!document.includes(required)) {
      throw new Error(`${name} is missing production storage invariant: ${required}`);
    }
  }
  if (name === "mako-control-plane") {
    for (const required of [
      "MAKO_CONTROL_SQLITE_PATH",
      "MAKO_CONTROL_SQLITE_LOCK_PATH",
      "MAKO_CONTROL_SQLITE_IDENTITY",
      "MAKO_CONTROL_SQLITE_MIGRATION_WORKSPACE",
      "MAKO_CONTROL_SQLITE_BACKUP_STAGING",
      "MAKO_CONTROL_SQLITE_BACKUP_PUBLISH",
      "MAKO_CONTROL_SQLITE_RESTORE_WORKSPACE",
      "MAKO_CONTROL_SQLITE_RESERVE_PATH",
      "value: /var/lib/mako/control/live/control.sqlite3",
    ])
      assert(document.includes(required), `${name} is missing SQLite invariant: ${required}`);
    for (const forbidden of ["MAKO_ROCKSDB_PATH", "provision-rocksdb-volume"])
      assert(
        !document.includes(forbidden),
        `${name} retains legacy RocksDB configuration: ${forbidden}`,
      );
  } else {
    for (const required of [
      "value: /var/lib/mako/rocksdb",
      "--accept-matching-marker",
      "--confirm=PROVISION",
    ])
      assert(document.includes(required), `${name} is missing RocksDB invariant: ${required}`);
  }
  for (const forbidden of [
    "replicas: 2",
    "emptyDir:",
    "MAKO_STORAGE_BACKEND",
    "MAKO_STORAGE_ENDPOINT",
    "MAKO_STORAGE_CREDENTIAL",
  ]) {
    if (document.includes(forbidden)) {
      throw new Error(`${name} contains forbidden storage configuration: ${forbidden}`);
    }
  }
}

if (
  !manifest.includes("reclaimPolicy: Retain") ||
  !manifest.includes('encrypted: "true"') ||
  !manifest.includes("allowVolumeExpansion: true")
) {
  throw new Error("production storage class must retain, encrypt, and expand volumes");
}

console.log("validated one control SQLite and two tenant-facing RocksDB single-owner volumes");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
