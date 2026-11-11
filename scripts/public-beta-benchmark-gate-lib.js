// Decides the single-region beta gate from the hosted benchmark evidence.
//
// The gate is the benchmark: every gated p95 within its budget, computed from enough
// steady-state samples, with the capacity, failed-request, loss and integrity thresholds met. A calendar window, backup age,
// recovery-point exposure and cost are not inputs. The validator and the preview approval
// both derive the blocker set from here, so the recorded gate cannot drift from its evidence.

export const MINIMUM_STEADY_STATE_SAMPLES = 100;

// Gate path, the benchmark evidence key under `latencyMilliseconds`, and the blocker label.
export const GATED_LATENCY_PATHS = [
  { path: "auth", evidence: "authSignIn", label: "auth sign-in" },
  { path: "pull", evidence: "rxdbPull", label: "RxDB pull" },
  { path: "push", evidence: "rxdbPushBatch", label: "RxDB push batch" },
  { path: "liveDelivery", evidence: "rxdbLiveFirstEvent", label: "RxDB live first-event delivery" },
  { path: "functionWarm", evidence: "warmFunction", label: "warm function" },
  { path: "functionCold", evidence: "coldFunction", label: "cold function" },
];

export function evaluateHostedBenchmark(benchmark, thresholds) {
  assert(benchmark && typeof benchmark === "object", "hosted benchmark evidence is required");
  assert([1, 2].includes(benchmark.schemaVersion), "hosted benchmark schemaVersion is invalid");
  const minimumSamples = thresholds?.minimumSteadyStateSamples;
  assert(
    Number.isSafeInteger(minimumSamples) && minimumSamples >= MINIMUM_STEADY_STATE_SAMPLES,
    `beta minimumSteadyStateSamples must be at least ${MINIMUM_STEADY_STATE_SAMPLES}`,
  );

  const blockers = [];
  const latency = {};
  for (const { path, evidence, label } of GATED_LATENCY_PATHS) {
    const budget = thresholds.maximumP95Milliseconds?.[path];
    assert(Number.isFinite(budget) && budget > 0, `beta ${path} latency budget is invalid`);
    const measured = latencyMeasurement(benchmark, evidence);
    latency[path] = measured;
    if (measured.samples < minimumSamples) {
      const provisional =
        measured.p95 === null
          ? ""
          : `, provisional p95 ${milliseconds(measured.p95)} ms against a ${budget} ms budget`;
      blockers.push(
        `${label} p95 latency is unmeasured: ${measured.samples} of ${minimumSamples} required ` +
          `steady-state samples${provisional}`,
      );
    } else if (measured.p95 > budget) {
      blockers.push(
        `${label} p95 latency exceeds its ${budget} ms single-region beta budget: ` +
          `${milliseconds(measured.p95)} ms over ${measured.samples} steady-state samples`,
      );
    }
  }

  // These are not latency blockers and match no waivable pattern, so a run that records any of
  // them blocks the gate and cannot be accepted for preview.
  const headroom = benchmark.target?.capacityHeadroomAtTargetLoadPercent;
  if (!(Number.isFinite(headroom) && headroom >= thresholds.minimumCapacityHeadroomPercent)) {
    blockers.push(
      "hosted benchmark capacity headroom at target load is " +
        `${Number.isFinite(headroom) ? `${headroom} percent` : "unrecorded"}, ` +
        `below the required ${thresholds.minimumCapacityHeadroomPercent} percent`,
    );
  }
  for (const [phase, name] of [
    ["target", "target"],
    ["requiredHeadroom", "required headroom"],
  ]) {
    const failed = benchmark.concurrentLoad?.[phase]?.failedRequests;
    if (failed !== 0) {
      blockers.push(`hosted benchmark failed ${recorded(failed)} requests at ${name} concurrency`);
    }
  }
  const loss = benchmark.workload?.acknowledgedWriteLoss;
  if (!(Number.isSafeInteger(loss) && loss <= thresholds.acknowledgedWriteLoss)) {
    blockers.push(`hosted benchmark lost ${recorded(loss)} acknowledged writes`);
  }
  const integrity = benchmark.workload?.integrityFailures;
  if (!(Number.isSafeInteger(integrity) && integrity <= thresholds.integrityFailures)) {
    blockers.push(`hosted benchmark found ${recorded(integrity)} integrity failures`);
  }

  return {
    status: blockers.length === 0 ? "passed" : "blocked",
    blockers,
    latency,
  };
}

function latencyMeasurement(benchmark, key) {
  const recordedLatency = benchmark.latencyMilliseconds ?? {};
  let value = recordedLatency[key];
  if (benchmark.schemaVersion === 1) {
    // Schema 1 timed one pull of the whole collection before the recovery restart, and recorded
    // single observations as plain numbers. Each plain number is one sample.
    if (key === "rxdbPull") value = recordedLatency.rxdbPullBeforeRecovery;
    if (typeof value === "number") {
      assert(Number.isFinite(value) && value >= 0, `hosted benchmark ${key} latency is invalid`);
      return { samples: 1, p95: value };
    }
  }
  if (value === undefined || value === null) return { samples: 0, p95: null };
  assert(typeof value === "object", `hosted benchmark ${key} latency is not a sample summary`);
  assert(
    Number.isSafeInteger(value.samples) && value.samples >= 1,
    `hosted benchmark ${key} sample count is invalid`,
  );
  assert(
    Number.isFinite(value.p95) &&
      value.p95 >= 0 &&
      Number.isFinite(value.maximum) &&
      value.p95 <= value.maximum,
    `hosted benchmark ${key} p95 is invalid`,
  );
  return { samples: value.samples, p95: value.p95 };
}

function milliseconds(value) {
  return Math.round(value);
}

function recorded(value) {
  return Number.isFinite(value) ? String(value) : "an unrecorded number of";
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
