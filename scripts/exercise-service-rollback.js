#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const manifestPath = "infra/production/storage-statefulsets.yaml";
const currentImage = "ghcr.io/mako-cloud/mako-cloud:0.1.0";
const previousImage = "ghcr.io/mako-cloud/mako-cloud:0.0.9";
const source = await readFile(resolve(root, manifestPath), "utf8");

assert(currentImage !== previousImage, "rollback images must differ");
assert(
  count(source, currentImage) === 5,
  "expected two RocksDB init containers plus three services",
);

// Exercise the deployment mutation in memory. The checked manifest is not changed.
const rolledBack = source.replaceAll(currentImage, previousImage);
assert(count(rolledBack, currentImage) === 0, "candidate image remained after rollback");
assert(count(rolledBack, previousImage) === 5, "previous image was not applied atomically");
assert(
  withoutImages(rolledBack) === withoutImages(source),
  "service rollback changed something other than container images",
);

const statefulSets = rolledBack
  .split(/^---$/m)
  .filter((document) => document.includes("kind: StatefulSet"));
assert(statefulSets.length === 3, "expected three stateful service owners");
for (const document of statefulSets) {
  assert(/^ {2}replicas: 1$/m.test(document), "rollback must retain a single storage owner");
  assert(
    document.includes("whenDeleted: Retain") && document.includes("whenScaled: Retain"),
    "rollback must retain persistent claims",
  );
  assert(document.includes('accessModes: ["ReadWriteOnce"]'), "rollback must retain RWO storage");
  const control = document.includes("name: mako-control-plane");
  assert(count(document, previousImage) === (control ? 1 : 2), "rollback image count is incorrect");
  assert(
    control
      ? document.includes("MAKO_CONTROL_SQLITE_PATH")
      : document.includes("value: /var/lib/mako/rocksdb"),
    "rollback must retain the owned storage path",
  );
}

console.log(
  `exercised in-memory SQLite-compatible service rollback for ${statefulSets.length} StatefulSets; only 5 image references changed`,
);

function withoutImages(value) {
  return value.replace(/^(\s*image:\s*).+$/gm, "$1<IMAGE>");
}

function count(value, needle) {
  return value.split(needle).length - 1;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
