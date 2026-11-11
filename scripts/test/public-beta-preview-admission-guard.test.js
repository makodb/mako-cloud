import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import test from "node:test";

import {
  buildApproval,
  computeBlockerDigest,
  requiredConfirmation,
} from "../public-beta-preview-approval-lib.js";

const root = resolve(import.meta.dirname, "../..");
const guard = resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission");
const guardSource = readFileSync(guard, "utf8");
const guardUnit = readFileSync(
  resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission.service"),
  "utf8",
);
const planHash = "a".repeat(64);
const releaseDigest = "b".repeat(64);
const blockers = [
  "provide authenticated SMTP relay credentials and prove certificate-expiry alert delivery",
  "auth sign-in p95 latency exceeds its 200 ms single-region beta budget: 631 ms over 100 steady-state samples",
  "RxDB pull p95 latency is unmeasured: 1 of 100 required steady-state samples",
  "hosted benchmark latency is unmeasured under online backups: 1 of 2 required completed online backups during the run",
];
const blockerDigest = computeBlockerDigest(blockers);
const acceptedAt = "2026-08-09T08:00:00.000Z";
const safeguards = {
  trustedHttpsAndHsts: true,
  exactPublicRouteAllowlist: true,
  applicationSecurity: true,
  backupAndRecovery: true,
  zeroAcknowledgedWriteLoss: true,
  zeroIntegrityFailures: true,
  serviceReadiness: true,
  emergencyAdmissionStop: true,
};

function fixture() {
  const directory = mkdtempSync(resolve(tmpdir(), "mako-preview-guard-"));
  const path = (absolute) => resolve(directory, absolute.slice(1));
  for (const item of [
    "/etc/mako",
    "/etc/caddy/mako-admission",
    "/opt/mako/releases",
    "/var/lib/mako-public-preview",
  ]) {
    mkdirSync(path(item), { recursive: true });
  }
  for (const mode of ["pre_gate", "risk_accepted_preview"]) {
    writeFileSync(
      path(`/etc/caddy/mako-admission/Caddyfile.${mode}`),
      "Strict-Transport-Security max-age=31536000\n",
    );
  }
  mkdirSync(path(`/opt/mako/releases/${releaseDigest}`));
  symlinkSync(path(`/opt/mako/releases/${releaseDigest}`), path("/opt/mako/current"));
  symlinkSync(path("/etc/caddy/mako-admission/Caddyfile.pre_gate"), path("/etc/caddy/Caddyfile"));
  const fixtureSafeguards = structuredClone(safeguards);
  const approval = buildApproval({
    operatorIdentity: "operator@example.com",
    planHash,
    releaseDigest,
    blockers,
    acceptedAt,
    nonWaivableSafeguards: fixtureSafeguards,
    typedConfirmation: requiredConfirmation({ releaseDigest, planHash, blockerDigest }),
  });
  const context = {
    schemaVersion: 1,
    publicFqdn: "cloud-test.makodb.com",
    planHash,
    releaseDigest,
    blockerDigest,
    nonWaivableSafeguards: structuredClone(fixtureSafeguards),
  };
  writeJson(path("/etc/mako/public-preview-approval.json"), approval);
  writeJson(path("/etc/mako/public-preview-context.json"), context);
  return { directory, path, approval, context };
}

function run(directory, command, now = "2026-08-10T00:00:00.000Z") {
  return spawnSync(
    "python3",
    [guard, command, "--root", directory, "--static", "--no-reload", "--now", now],
    { encoding: "utf8" },
  );
}

test("valid activation atomically selects preview and valid guard preserves it", () => {
  const state = fixture();
  const activation = run(state.directory, "activate");
  assert.equal(activation.status, 0, activation.stderr);
  assert.match(activation.stdout, /"mode": "risk_accepted_preview"/);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
  const guarded = run(state.directory, "guard");
  assert.equal(guarded.status, 0, guarded.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
});

test("persistent approval survives time and manual pause is atomic and revocable", () => {
  const state = fixture();
  assert.equal(run(state.directory, "activate").status, 0);
  const future = run(state.directory, "guard", "2126-08-23T08:00:00.000Z");
  assert.equal(future.status, 0, future.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);

  const result = run(state.directory, "pause");
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /"manuallyPaused": true/);
  assert.match(realActive(state.path), /Caddyfile\.pre_gate$/);
  const evidence = JSON.parse(
    readFileSync(state.path("/var/lib/mako-public-preview/last-guard.json"), "utf8"),
  );
  assert.equal(evidence.mode, "pre_gate");
  assert.deepEqual(evidence.reasons, ["manual operator pause"]);
  assert.equal(evidence.approvalInvalidated, true);
  assert.equal(JSON.stringify(evidence).includes("typedConfirmation"), false);

  const rejected = run(state.directory, "activate");
  assert.equal(rejected.status, 1);
  assert.match(rejected.stderr, /issue a new approval/);
  assert.match(realActive(state.path), /Caddyfile\.pre_gate$/);

  const replacementAcceptedAt = "2026-08-10T00:00:00.000Z";
  const replacement = buildApproval({
    operatorIdentity: "operator@example.com",
    planHash,
    releaseDigest,
    blockers,
    acceptedAt: replacementAcceptedAt,
    nonWaivableSafeguards: safeguards,
    typedConfirmation: requiredConfirmation({ releaseDigest, planHash, blockerDigest }),
  });
  writeJson(state.path("/etc/mako/public-preview-approval.json"), replacement);
  const reactivation = run(state.directory, "activate");
  assert.equal(reactivation.status, 0, reactivation.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
});

test("a redeploy does not close admission", () => {
  // Admission used to be bound to the selected release, so shipping any build
  // closed public access within a minute and fell back to the source-restricted
  // allowlist. Deploying a different release must now leave preview open.
  const state = fixture();
  assert.equal(run(state.directory, "activate").status, 0);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);

  const redeployed = "d".repeat(64);
  mkdirSync(state.path(`/opt/mako/releases/${redeployed}`));
  unlinkSync(state.path("/opt/mako/current"));
  symlinkSync(state.path(`/opt/mako/releases/${redeployed}`), state.path("/opt/mako/current"));
  // Ansible renders the context from the deployed release, so it moves too.
  state.context.releaseDigest = redeployed;
  writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);

  const guarded = run(state.directory, "guard");
  assert.equal(guarded.status, 0, guarded.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
});

test("plan, blocker, confirmation, and every safeguard drift fail closed", () => {
  const mutations = [
    ({ context }) => {
      context.planHash = "c".repeat(64);
    },
    ({ context }) => {
      context.blockerDigest = "c".repeat(64);
    },
    ({ approval }) => {
      approval.typedConfirmation = "yes";
    },
    ({ approval }) => {
      approval.expiresAt = "2126-08-23T08:00:00.000Z";
    },
    ...Object.keys(safeguards).map((name) => ({ context }) => {
      context.nonWaivableSafeguards[name] = false;
    }),
  ];
  for (const mutate of mutations) {
    const state = fixture();
    mutate(state);
    writeJson(state.path("/etc/mako/public-preview-approval.json"), state.approval);
    writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);
    unlinkSync(state.path("/etc/caddy/Caddyfile"));
    symlinkSync(
      state.path("/etc/caddy/mako-admission/Caddyfile.risk_accepted_preview"),
      state.path("/etc/caddy/Caddyfile"),
    );
    const result = run(state.directory, "guard");
    assert.equal(result.status, 1, result.stdout);
    assert.match(realActive(state.path), /Caddyfile\.pre_gate$/);
  }
});

test("independent emergency-stop assets still remove public ports and Caddy", () => {
  const stop = readFileSync(resolve(root, "scripts/proxmox/public-beta-admission-stop.sh"), "utf8");
  const nft = readFileSync(
    resolve(root, "infra/proxmox/public-beta/firewall/mako-vm124-admission-stop.nft"),
    "utf8",
  );
  assert.match(stop, /systemctl disable --now caddy\.service/);
  assert.match(nft, /tcp dport \{ 80, 443 \}/);
});

test("guard records fail-closed outcomes without a Caddy restart loop", () => {
  assert.match(guardSource, /if changed and not no_reload:/u);
  assert.match(guardUnit, /Wants=network-online\.target caddy\.service/u);
  assert.doesNotMatch(guardUnit, /^Requires=caddy\.service$/mu);
});

test("a failed or stale backup neither closes nor opens admission", () => {
  // Backups run online and never stop the service, so backup state is an alert, not an
  // admission input. Every backup metric reports failure here, including the retired
  // backupFreshnessRequired field left in an older context.
  const state = fixture();
  state.context.backupFreshnessRequired = true;
  writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);
  const failedBackup = { backupVerified: 0, backupAgeSeconds: 86400 };

  const activation = runGuard(state, "activate", failedBackup);
  assert.equal(activation.status, 0, activation.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
  const guarded = runGuard(state, "guard", failedBackup);
  assert.equal(guarded.status, 0, guarded.stderr);
  assert.match(realActive(state.path), /Caddyfile\.risk_accepted_preview$/);
  for (const result of [activation, guarded]) {
    assert.deepEqual(result.prometheusQueries, []);
  }

  const restricted = fixture();
  const healthy = runGuard(restricted, "guard", { backupVerified: 1, backupAgeSeconds: 0 });
  assert.equal(healthy.status, 0, healthy.stderr);
  assert.match(realActive(restricted.path), /Caddyfile\.pre_gate$/);
  assert.deepEqual(healthy.prometheusQueries, []);
});

test("active preview still fails closed on readiness, TLS and HSTS", () => {
  for (const [prepare, options, reason] of [
    [() => {}, { readiness: 503 }, /service readiness failed/],
    [() => {}, { tlsFailure: true }, /TLS rejected/],
    [
      (state) =>
        writeFileSync(state.path("/etc/caddy/mako-admission/Caddyfile.risk_accepted_preview"), ""),
      {},
      /preview HSTS is absent/,
    ],
  ]) {
    const state = fixture();
    assert.equal(runGuard(state, "activate").status, 0);
    prepare(state);
    const rejected = runGuard(state, "guard", options);
    assert.equal(rejected.status, 1);
    assert.match(rejected.stderr, reason);
    assert.match(realActive(state.path), /Caddyfile\.pre_gate$/);
  }
});

test("retired 30-day and cost blockers are no longer waivable", () => {
  const state = fixture();
  const retired = [
    ...blockers,
    "complete the 30-day resource, capacity, availability, and cost window with provider or facility evidence",
  ].sort();
  // The approval library refuses to build this record, so write it as an operator could by hand.
  const digest = createHash("sha256").update(JSON.stringify(retired)).digest("hex");
  const approval = {
    ...state.approval,
    blockers: retired,
    blockerDigest: digest,
    typedConfirmation: requiredConfirmation({ releaseDigest, planHash, blockerDigest: digest }),
  };
  writeJson(state.path("/etc/mako/public-preview-approval.json"), approval);
  writeJson(state.path("/etc/mako/public-preview-context.json"), {
    ...state.context,
    blockerDigest: digest,
  });
  const result = run(state.directory, "activate");
  assert.equal(result.status, 1);
  assert.match(result.stderr, /approval contains a non-waivable blocker/);
  assert.match(realActive(state.path), /Caddyfile\.pre_gate$/);
});

// Runs the guard's own entry point with readiness, TLS and Prometheus replaced. Prometheus
// answers every backup query with the requested (failed) backup state and records the query,
// so a test can prove the guard never consults it.
function runGuard(state, command, options = {}) {
  const program = `
import contextlib, json, runpy, sys, urllib.parse
from unittest.mock import patch
guard = runpy.run_path(sys.argv[1])
root, command, options = sys.argv[2], sys.argv[3], json.loads(sys.argv[4])
scope = guard['main'].__globals__
prometheus_queries = []
class Response:
    def __init__(self, status, body=b''):
        self.status = status
        self.body = body
    def read(self, *args):
        return self.body
def urlopen(request, *args, **kwargs):
    url = getattr(request, 'full_url', request)
    if url.endswith('/readyz'):
        return contextlib.nullcontext(Response(options.get('readiness', 200)))
    query = urllib.parse.parse_qs(urllib.parse.urlparse(url).query).get('query', [''])[0]
    prometheus_queries.append(query)
    if 'remote_verified' in query:
        value = options.get('backupVerified', 0)
    elif 'time()' in query:
        value = options.get('backupAgeSeconds', 86400)
    else:
        value = 0
    payload = {'status': 'success', 'data': {'resultType': 'vector', 'result': [
        {'metric': {}, 'value': [0, str(value)]}]}}
    return contextlib.nullcontext(Response(200, json.dumps(payload).encode()))
class TLS:
    def wrap_socket(self, *args, **kwargs):
        if options.get('tlsFailure'):
            raise RuntimeError('TLS rejected')
        return contextlib.nullcontext()
argv = ['mako-public-preview-admission', command, '--root', root, '--no-reload',
        '--now', '2026-08-10T00:00:00.000Z']
with patch.object(scope['urllib'].request, 'urlopen', side_effect=urlopen), \\
     patch.object(scope['socket'], 'create_connection', return_value=contextlib.nullcontext()), \\
     patch.object(scope['ssl'], 'create_default_context', return_value=TLS()), \\
     patch.object(scope['time'], 'sleep'), \\
     patch.object(sys, 'argv', argv):
    status = guard['main']()
print(json.dumps({'status': status, 'prometheusQueries': prometheus_queries}))
`;
  const result = spawnSync(
    "python3",
    ["-c", program, guard, state.directory, command, JSON.stringify(options)],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  const outcome = JSON.parse(result.stdout.trim().split("\n").at(-1));
  return { ...outcome, stderr: result.stderr };
}

function realActive(path) {
  return realpathSync(path("/etc/caddy/Caddyfile"));
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}
