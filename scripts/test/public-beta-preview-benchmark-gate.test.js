import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";

import { evaluateHostedBenchmark, GATED_LATENCY_PATHS } from "../public-beta-benchmark-gate-lib.js";
import { normalizeBlockers } from "../public-beta-preview-approval-lib.js";

const root = resolve(import.meta.dirname, "../..");
const guard = resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission");
const gates = JSON.parse(readFileSync(resolve(root, "docs/release-gates.json"), "utf8"));
const beta = gates.stages.find((stage) => stage.id === "single-region-beta");
const thresholds = beta.thresholds;

// A schema 2 run that meets every threshold: 100 steady-state samples per gated path, each p95
// inside its budget, two online backups completed inside the steady-state window.
function passingBenchmark() {
  const latencyMilliseconds = {};
  for (const { path, evidence } of GATED_LATENCY_PATHS) {
    const p95 = thresholds.maximumP95Milliseconds[path] - 1;
    latencyMilliseconds[evidence] = { samples: 100, p50: p95 / 2, p95, maximum: p95 + 5 };
  }
  return {
    schemaVersion: 2,
    target: { capacityHeadroomAtTargetLoadPercent: 50 },
    latencyMilliseconds,
    steadyState: { startedAt: "2026-11-10T10:00:00.000Z", completedAt: "2026-11-10T10:14:00.000Z" },
    onlineBackups: {
      scheduleEnabled: true,
      completedDuringRun: 2,
      completions: ["2026-11-10T10:04:31.000Z", "2026-11-10T10:10:02.000Z"],
    },
    workload: { acknowledgedWriteLoss: 0, integrityFailures: 0 },
    concurrentLoad: { target: { failedRequests: 0 }, requiredHeadroom: { failedRequests: 0 } },
  };
}

test("the benchmark meets every threshold and the gate passes", () => {
  const gate = evaluateHostedBenchmark(passingBenchmark(), thresholds);
  assert.equal(gate.status, "passed");
  assert.deepEqual(gate.blockers, []);
  assert.equal(gate.onlineBackupsCompleted, 2);
  assert.equal(gate.latency.functionCold.samples, 100);
});

test("a path over its budget, a failed request, a lost write or an integrity failure blocks", () => {
  const cases = [
    [
      (run) => {
        Object.assign(run.latencyMilliseconds.authSignIn, { p95: 300.4, maximum: 410 });
      },
      /^auth sign-in p95 latency exceeds its 200 ms single-region beta budget: 300 ms over 100 /,
    ],
    [
      (run) => {
        run.concurrentLoad.requiredHeadroom.failedRequests = 3;
      },
      /^hosted benchmark failed 3 requests at required headroom concurrency$/,
    ],
    [
      (run) => {
        run.workload.acknowledgedWriteLoss = 1;
      },
      /^hosted benchmark lost 1 acknowledged writes$/,
    ],
    [
      (run) => {
        run.workload.integrityFailures = 2;
      },
      /^hosted benchmark found 2 integrity failures$/,
    ],
    [
      (run) => {
        run.target.capacityHeadroomAtTargetLoadPercent = 20;
      },
      /^hosted benchmark capacity headroom at target load is 20 percent, below the required 30/,
    ],
  ];
  for (const [mutate, blocker] of cases) {
    const run = passingBenchmark();
    mutate(run);
    const gate = evaluateHostedBenchmark(run, thresholds);
    assert.equal(gate.status, "blocked");
    assert.equal(gate.blockers.length, 1);
    assert.match(gate.blockers[0], blocker);
  }
});

test("a path with fewer than 100 steady-state samples is unmeasured and blocks", () => {
  const run = passingBenchmark();
  run.latencyMilliseconds.rxdbPull.samples = 99;
  delete run.latencyMilliseconds.coldFunction;
  const gate = evaluateHostedBenchmark(run, thresholds);
  assert.equal(gate.status, "blocked");
  assert.deepEqual(gate.blockers, [
    "RxDB pull p95 latency is unmeasured: 99 of 100 required steady-state samples, " +
      "provisional p95 249 ms against a 250 ms budget",
    "cold function p95 latency is unmeasured: 0 of 100 required steady-state samples",
  ]);
  // Schema 2 evidence must summarize samples; a bare number is not a p95.
  const scalar = passingBenchmark();
  scalar.latencyMilliseconds.warmFunction = 120;
  assert.throws(() => evaluateHostedBenchmark(scalar, thresholds), /not a sample summary/);
});

test("the run must span two completed online backups inside its steady-state window", () => {
  const one = passingBenchmark();
  one.onlineBackups.completions.pop();
  one.onlineBackups.completedDuringRun = 1;
  assert.deepEqual(evaluateHostedBenchmark(one, thresholds).blockers, [
    "hosted benchmark latency is unmeasured under online backups: 1 of 2 required completed " +
      "online backups during the run",
  ]);

  const none = passingBenchmark();
  delete none.onlineBackups;
  assert.match(
    evaluateHostedBenchmark(none, thresholds).blockers[0],
    /^hosted benchmark latency is unmeasured under online backups: the run recorded no online/,
  );

  const outside = passingBenchmark();
  outside.onlineBackups.completions[0] = "2026-11-10T09:59:59.000Z";
  assert.throws(() => evaluateHostedBenchmark(outside, thresholds), /outside the steady-state/);

  const disabled = passingBenchmark();
  disabled.onlineBackups.scheduleEnabled = false;
  assert.throws(() => evaluateHostedBenchmark(disabled, thresholds), /without the backup schedule/);

  const miscounted = passingBenchmark();
  miscounted.onlineBackups.completedDuringRun = 3;
  assert.throws(() => evaluateHostedBenchmark(miscounted, thresholds), /inconsistent/);
});

test("informational window, backup-age and cost observations do not affect the gate", () => {
  for (const field of [
    "maximumBackupAgeSeconds",
    "maximumRecoveryPointExposureSeconds",
    "maximumMonthlyFixedInfrastructureUsd",
    "maximumCostPerMillionReplicationOperationsUsd",
  ]) {
    assert.equal(field in thresholds, false, `${field} is still a beta threshold`);
  }
  for (const field of ["measurementWindow", "monthlyFixedInfrastructureUsd"]) {
    assert.equal(field in beta.observed, false, `${field} is still a beta observation`);
  }
  // None of them recorded at all, and a run that carries them anyway, both pass.
  assert.equal(evaluateHostedBenchmark(passingBenchmark(), thresholds).status, "passed");
  const informational = passingBenchmark();
  informational.measurementWindow = { status: "in-progress", completedAt: null };
  informational.backupAgeSeconds = 86400;
  informational.monthlyFixedInfrastructureUsd = null;
  assert.equal(evaluateHostedBenchmark(informational, thresholds).status, "passed");
});

test("the gate thresholds cannot be lowered below the specified sample and backup floors", () => {
  for (const [field, value] of [
    ["minimumSteadyStateSamples", 99],
    ["minimumOnlineBackupsDuringRun", 1],
  ]) {
    assert.throws(
      () => evaluateHostedBenchmark(passingBenchmark(), { ...thresholds, [field]: value }),
      new RegExp(field),
    );
  }
});

test("the recorded beta gate is the result computed from its hosted benchmark", () => {
  const benchmark = JSON.parse(
    readFileSync(resolve(root, gates.sources.publicBetaHostedBenchmark), "utf8"),
  );
  const gate = evaluateHostedBenchmark(benchmark, thresholds);
  assert.equal(beta.status, gate.status);
  assert.deepEqual([...beta.blockers].sort(), [...gate.blockers].sort());
});

test("latency blockers are waivable and correctness blockers are not, in both checkers", () => {
  // One path over budget, one with a single schema 1 observation, the rest never measured, and
  // no online backup activity.
  const latency = evaluateHostedBenchmark(
    {
      ...passingBenchmark(),
      schemaVersion: 1,
      latencyMilliseconds: {
        authSignIn: { samples: 100, p50: 400, p95: 900, maximum: 950 },
        rxdbPullBeforeRecovery: 2000,
      },
      onlineBackups: undefined,
    },
    thresholds,
  ).blockers;
  assert.match(latency[0], /^auth sign-in p95 latency exceeds/);
  assert.match(latency[1], /^RxDB pull p95 latency is unmeasured: 1 of 100 .* 2000 ms/);
  const correctness = [
    "hosted benchmark lost 1 acknowledged writes",
    "hosted benchmark found 1 integrity failures",
    "hosted benchmark failed 1 requests at target concurrency",
    "hosted benchmark capacity headroom at target load is 10 percent, below the required 30 percent",
    "complete the 30-day resource, capacity, availability, and cost window with provider or facility evidence",
  ];
  assert.equal(latency.length, GATED_LATENCY_PATHS.length + 1);
  assert.deepEqual(normalizeBlockers(latency), [...latency].sort());
  for (const blocker of correctness) {
    assert.throws(() => normalizeBlockers([blocker]), /non-waivable blocker/);
  }
  const program = `
import json, runpy, sys
patterns = runpy.run_path(sys.argv[1])['WAIVABLE_BLOCKERS']
print(json.dumps([any(p.search(b) for p in patterns) for b in json.loads(sys.argv[2])]))
`;
  const result = spawnSync(
    "python3",
    ["-c", program, guard, JSON.stringify([...latency, ...correctness])],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), [
    ...latency.map(() => true),
    ...correctness.map(() => false),
  ]);
});
