#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const reportPath = resolve(repositoryRoot, process.argv[2] ?? "docs/performance-baseline.json");
const report = JSON.parse(await readFile(reportPath, "utf8"));

const expectedPaths = new Set([
  "write",
  "pull",
  "hidden-change-scan",
  "push-conflict",
  "live-fan-out",
  "auth",
  "policy",
  "index-build",
  "control-plane",
  "edge-cold",
  "edge-warm",
]);

assert(report.schemaVersion === 1, "schemaVersion must be 1");
assert(report.profile === "release", "profile must be release");
assert(
  report.storageAdapter === "local-rocksdb-optimistic-transactiondb",
  "storageAdapter must identify the local RocksDB adapter",
);
assert(report.durability === "sync", "durability must be sync");
assertPositiveInteger(report.generatedAtUnixSeconds, "generatedAtUnixSeconds");
assertPositiveInteger(report.parameters?.iterations, "parameters.iterations");
assertPositiveInteger(report.parameters?.datasetDocuments, "parameters.datasetDocuments");
assertPositiveInteger(report.parameters?.liveSubscribers, "parameters.liveSubscribers");
assert(Array.isArray(report.results), "results must be an array");
assert(
  report.results.length === expectedPaths.size,
  `results must contain exactly ${expectedPaths.size} paths`,
);

const observedPaths = new Set();
for (const result of report.results) {
  assert(
    typeof result.path === "string" && expectedPaths.has(result.path),
    `unexpected benchmark path: ${String(result.path)}`,
  );
  assert(!observedPaths.has(result.path), `duplicate benchmark path: ${result.path}`);
  observedPaths.add(result.path);
  assertPositiveInteger(result.samples, `${result.path}.samples`);
  assert(
    result.samples === report.parameters.iterations,
    `${result.path}.samples must match parameters.iterations`,
  );
  const expectedOperations = result.path === "live-fan-out" ? report.parameters.liveSubscribers : 1;
  assert(
    result.operationsPerSample === expectedOperations,
    `${result.path}.operationsPerSample is inconsistent`,
  );
  assert(
    result.totalOperations === result.samples * result.operationsPerSample,
    `${result.path}.totalOperations is inconsistent`,
  );
  for (const field of [
    "p50Microseconds",
    "p95Microseconds",
    "minMicroseconds",
    "maxMicroseconds",
  ]) {
    assertNonNegativeFinite(result[field], `${result.path}.${field}`);
  }
  assertPositiveFinite(result.operationsPerSecond, `${result.path}.operationsPerSecond`);
  assert(
    result.minMicroseconds <= result.p50Microseconds &&
      result.p50Microseconds <= result.p95Microseconds &&
      result.p95Microseconds <= result.maxMicroseconds,
    `${result.path} percentiles are not ordered`,
  );
}

for (const path of expectedPaths) {
  assert(observedPaths.has(path), `missing benchmark path: ${path}`);
}

console.log(`Validated ${report.results.length} performance paths in ${reportPath}`);

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function assertPositiveInteger(value, field) {
  assert(Number.isInteger(value) && value > 0, `${field} must be a positive integer`);
}

function assertNonNegativeFinite(value, field) {
  assert(Number.isFinite(value) && value >= 0, `${field} must be finite and non-negative`);
}

function assertPositiveFinite(value, field) {
  assert(Number.isFinite(value) && value > 0, `${field} must be finite and positive`);
}
