#!/usr/bin/env node
// Every alert rule must watch a metric that something actually publishes.
//
// A Prometheus expression over a metric no process emits does not fail loudly. It
// evaluates against an empty vector, so a comparison like `metric < 1` is never true
// and the alert can never fire. A rule written to catch corruption, a tenant isolation
// breach, or a failed audit write then sits green forever while nothing watches. The
// one construct that fails the other way, `unless on() metric`, removes nothing when
// its right side is empty and so fires forever, which trains an operator to ignore the
// channel. Both failure modes are silent to every gate we had: the observability
// validator checks that each rule has an `expr`, a `for`, a severity and a runbook, and
// never asks whether the metric in that `expr` exists.
//
// This reads the rules, extracts the metric names each expression depends on, and
// compares them against the names the workspace and the deployment actually produce:
// string literals in the Rust services, and the `HELP`/`TYPE` lines in the textfile
// collectors that Ansible installs.
//
// Rules whose metrics have no producer today are listed in UNPRODUCED with the reason.
// That list may only shrink. If an entry starts being produced, or names a rule that no
// longer exists, this fails too, so the debt cannot quietly go stale or be forgotten.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** The alert rules, and the trees that may publish a metric. */
export const RULES_FILE = "infra/local/prometheus-rules/mako-cloud-alerts.yaml";
export const CODE_TREES = ["crates", "services"];
export const DEPLOYMENT_TREE = "infra/ansible/roles";

/**
 * Alert rules whose metrics nothing publishes yet, each with the reason. These are
 * real coverage gaps being tracked, not exemptions: the data plane, edge gateway and
 * telemetry-query expose no `/metrics` endpoint at all, so every `mako_storage_*`,
 * `mako_sequencer_*`, `mako_tenant_*`, `mako_policy_*`, `mako_audit_*`,
 * `mako_auth_*`, `mako_function_*`, `mako_http_*` and `mako_request_*` series a rule
 * names is absent. Only the control plane serves metrics today.
 *
 * Remove an entry when its producer lands. Do not add one to make this pass.
 */
export const UNPRODUCED = new Map([
  ["MakoAuditWriteFailure", "no audit metrics exporter"],
  ["MakoAuthRefreshReplayDetected", "no gateway metrics exporter"],
  ["MakoEdgeSandboxIncident", "no edge runtime metrics exporter"],
  ["MakoHttpErrorRateHigh", "no shared HTTP transport metrics exporter"],
  ["MakoPolicyEvaluationFailureRatioHigh", "no policy metrics exporter"],
  ["MakoRecoveryEvidenceMissing", "no recurring restore verification publishes a result"],
  ["MakoRequestLatencyHigh", "no shared HTTP transport metrics exporter"],
  ["MakoRocksDbCorruptionSignal", "no data plane storage metrics exporter"],
  ["MakoRocksDbRecoverySlow", "no data plane storage metrics exporter"],
  ["MakoRocksDbRestoreFailed", "no recurring restore verification publishes a result"],
  ["MakoRocksDbUnavailable", "no data plane storage metrics exporter"],
  ["MakoRocksDbWriteStalled", "no data plane storage metrics exporter"],
  ["MakoSequencerGapBlocking", "no sequencer metrics exporter"],
  ["MakoStorageContractFailed", "no data plane storage metrics exporter"],
  ["MakoTenantIsolationSignal", "no data plane tenant metrics exporter"],
]);

/** `[name, severity, metrics]` for every alert rule in the file, in file order. */
export function alertRules(source) {
  const rules = [];
  const blocks = source.split(/^ {6}- alert: /m).slice(1);
  for (const block of blocks) {
    const newline = block.indexOf("\n");
    const name = block.slice(0, newline).trim();
    const body = block.slice(newline + 1);
    const expression = /expr:\s*([\s\S]*?)(?=\n\s+(?:for|labels|annotations):)/.exec(body);
    const severity = /severity:\s*(\S+)/.exec(body);
    rules.push({
      name,
      severity: severity === null ? "unknown" : severity[1],
      metrics: new Set(
        expression === null ? [] : (expression[1].match(/\bmako_[a-z0-9_]+/g) ?? []),
      ),
    });
  }
  return rules;
}

function* filesUnder(directory, extensions) {
  let entries;
  try {
    entries = readdirSync(directory);
  } catch {
    return;
  }
  for (const entry of entries) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) {
      yield* filesUnder(path, extensions);
    } else if (extensions === null || extensions.some((suffix) => entry.endsWith(suffix))) {
      yield path;
    }
  }
}

/**
 * Every metric name something publishes. Rust sources name theirs in string literals,
 * sometimes inside a format string with label braces, so the name is read wherever it
 * appears rather than only when a quote closes it. A crate path such as `mako_storage`
 * is picked up too; that is harmless, because it never equals a full metric name.
 */
export function producedMetrics(root) {
  const produced = new Set();
  for (const tree of CODE_TREES) {
    for (const file of filesUnder(resolve(root, tree), [".rs"])) {
      for (const name of readFileSync(file, "utf8").match(/\bmako_[a-z0-9_]+/g) ?? []) {
        produced.add(name);
      }
    }
  }
  for (const file of filesUnder(resolve(root, DEPLOYMENT_TREE), null)) {
    if (statSync(file).size > 400_000) continue;
    const text = readFileSync(file, "utf8");
    if (!text.includes("HELP mako_") && !text.includes("TYPE mako_")) continue;
    for (const match of text.matchAll(/(?:HELP|TYPE)\s+(mako_[a-z0-9_]+)/g)) {
      produced.add(match[1]);
    }
  }
  return produced;
}

export function main(root, log = console) {
  const rules = alertRules(readFileSync(resolve(root, RULES_FILE), "utf8"));
  if (rules.length === 0) {
    log.error(`${RULES_FILE}: no alert rules were parsed`);
    return 1;
  }
  const produced = producedMetrics(root);
  const problems = [];
  const dead = new Set();

  for (const rule of rules) {
    const missing = [...rule.metrics].filter((metric) => !produced.has(metric)).sort();
    if (missing.length === 0) continue;
    dead.add(rule.name);
    if (!UNPRODUCED.has(rule.name)) {
      problems.push(
        `${rule.name} (${rule.severity}) watches ${missing.join(", ")}, which nothing publishes; ` +
          "add the producer, or record the gap in UNPRODUCED with a reason",
      );
    }
  }
  for (const [name, reason] of UNPRODUCED) {
    if (!rules.some((rule) => rule.name === name)) {
      problems.push(`UNPRODUCED names ${name} (${reason}), which is not an alert rule any more`);
    } else if (!dead.has(name)) {
      problems.push(
        `${name} now has a producer for every metric it watches; remove it from UNPRODUCED`,
      );
    }
  }

  if (problems.length > 0) {
    for (const problem of problems) log.error(problem);
    return 1;
  }
  const live = rules.length - dead.size;
  const criticals = rules.filter(
    (rule) => dead.has(rule.name) && rule.severity === "critical",
  ).length;
  log.log(
    `validated ${rules.length} alert rules against ${produced.size} published metrics; ` +
      `${live} can fire, ${dead.size} are tracked as unproduced (${criticals} critical)`,
  );
  return 0;
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
  process.exit(main(root));
}
