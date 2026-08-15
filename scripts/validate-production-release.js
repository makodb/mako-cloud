#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const qualificationPath = resolve(root, "docs/production-rocksdb-qualification.json");
const performancePath = resolve(root, "docs/performance-baseline.json");
const qualification = JSON.parse(await readFile(qualificationPath, "utf8"));
const performance = JSON.parse(await readFile(performancePath, "utf8"));

assert(qualification.schemaVersion === 1, "qualification schemaVersion must be 1");
assert(
  qualification.topology === "single-node-local-rocksdb",
  "qualification topology must be single-node-local-rocksdb",
);
assert(qualification.storage?.filesystemType, "the qualified filesystem type must be recorded");
assert(qualification.storage?.persistent === true, "qualification must run on persistent storage");
assert(
  qualification.storage?.availableBytes > qualification.thresholds?.capacityWarningFreeBytes,
  "qualified storage must have more free capacity than the warning reserve",
);
assert(
  qualification.thresholds?.capacityWarningFreeBytes >
    qualification.thresholds?.capacityCriticalFreeBytes,
  "capacity warning reserve must exceed the critical reserve",
);
assert(
  qualification.thresholds?.capacityCriticalFreeBytes >= 64 * 1024 * 1024,
  "capacity critical reserve must be at least 64 MiB",
);

const evidence = qualification.evidence ?? {};
for (const name of [
  "exactProductionConfiguration",
  "semanticConformance",
  "processCrashRestart",
  "diskAndIoFailure",
  "lockAndEmptyFallback",
  "compactionAndRetention",
  "tenantIsolation",
  "backupCorruption",
  "restoreAndPromotion",
  "previousBinaryRollback",
]) {
  assert(evidence[name] === true, `missing passing qualification evidence: ${name}`);
}
assert(evidence.soakIterations >= 25, "at least 25 complete soak iterations are required");
assert(evidence.acknowledgedWriteLoss === 0, "acknowledged write loss must be zero");
assert(evidence.integrityFailures === 0, "integrity failures must be zero");

assert(
  qualification.observed?.backupAgeSeconds <= qualification.thresholds?.maximumBackupAgeSeconds,
  "observed backup age exceeds the release threshold",
);
assert(
  qualification.observed?.recoveryPointExposureSeconds <=
    qualification.thresholds?.maximumRecoveryPointExposureSeconds,
  "observed recovery-point exposure exceeds the release threshold",
);
assert(
  qualification.observed?.sameVolumeRecoverySeconds <=
    qualification.thresholds?.maximumSameVolumeRecoverySeconds,
  "same-volume recovery exceeds the release threshold",
);
assert(
  qualification.observed?.replacementRestoreSeconds <=
    qualification.thresholds?.maximumReplacementRestoreSeconds,
  "replacement restore exceeds the release threshold",
);

assert(
  performance.storageAdapter === "local-rocksdb-optimistic-transactiondb",
  "performance report must use the production RocksDB adapter",
);
assert(performance.durability === "sync", "performance report must use sync durability");
assert(performance.parameters?.iterations >= 25, "performance report needs at least 25 samples");
const results = new Map(performance.results.map((result) => [result.path, result]));
for (const [path, maximumP95Microseconds] of Object.entries(
  qualification.thresholds?.maximumP95Microseconds ?? {},
)) {
  const result = results.get(path);
  assert(result !== undefined, `performance report is missing ${path}`);
  assert(
    result.p95Microseconds <= maximumP95Microseconds,
    `${path} p95 ${result.p95Microseconds}us exceeds ${maximumP95Microseconds}us`,
  );
}

assert(qualification.releaseEligible === true, "qualification did not declare release eligibility");
console.log(
  `validated production RocksDB release evidence: ${evidence.soakIterations} soak cycles, ` +
    `0 acknowledged losses, 0 integrity failures, ${results.size} performance paths`,
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
