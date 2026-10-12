import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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
  "measured auth and RxDB latency exceed their beta budgets",
  "complete the 30-day resource, capacity, availability, and cost window",
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

test("backup freshness remains required unless explicitly disabled", () => {
  for (const required of [undefined, true]) {
    const state = fixture();
    if (required !== undefined) state.context.backupFreshnessRequired = required;
    writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);
    const stale = runDynamic(state, { backupAge: 901 });
    assert.equal(stale.status, 1);
    assert.match(stale.stderr, /backup age exceeded 15 minutes/);
    const fresh = runDynamic(state, { backupAge: 900 });
    assert.equal(fresh.status, 0, fresh.stderr);
  }
});

test("approved stale-backup preview still enforces verification, readiness, TLS and HSTS", () => {
  const state = fixture();
  state.context.backupFreshnessRequired = false;
  writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);
  const accepted = runDynamic(state, { backupAge: 86400 });
  assert.equal(accepted.status, 0, accepted.stderr);
  assert.deepEqual(JSON.parse(accepted.stdout).queries, [
    "min(mako_storage_backup_remote_verified)",
  ]);
  for (const [options, reason] of [
    [{ backupVerified: 0 }, /backup verification failed/],
    [{ readiness: 503 }, /service readiness failed/],
    [{ tlsFailure: true }, /TLS rejected/],
  ]) {
    const rejected = runDynamic(state, options);
    assert.equal(rejected.status, 1);
    assert.match(rejected.stderr, reason);
  }
  writeFileSync(state.path("/etc/caddy/mako-admission/Caddyfile.risk_accepted_preview"), "");
  const noHsts = runDynamic(state);
  assert.equal(noHsts.status, 1);
  assert.match(noHsts.stderr, /preview HSTS is absent/);
});

test("malformed backup freshness policy fails closed", () => {
  for (const value of ["false", null, 0]) {
    const state = fixture();
    state.context.backupFreshnessRequired = value;
    writeJson(state.path("/etc/mako/public-preview-context.json"), state.context);
    const result = run(state.directory, "activate");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /backup freshness policy is invalid/);
  }
});

function runDynamic(state, options = {}) {
  const program = `
import contextlib, json, runpy, sys
from unittest.mock import patch
guard = runpy.run_path(sys.argv[1])
paths = guard['Paths'](__import__('pathlib').Path(sys.argv[2]))
options = json.loads(sys.argv[3])
scope = guard['verify_dynamic'].__globals__
queries = []
def metric(query):
    queries.append(query)
    if query == 'min(mako_storage_backup_remote_verified)':
        return options.get('backupVerified', 1)
    return options.get('backupAge', 901)
class Ready:
    status = options.get('readiness', 200)
class TLS:
    def wrap_socket(self, *args, **kwargs):
        if options.get('tlsFailure'):
            raise RuntimeError('TLS rejected')
        return contextlib.nullcontext()
scope['prometheus_value'] = metric
with patch.object(scope['urllib'].request, 'urlopen', return_value=contextlib.nullcontext(Ready())), \\
     patch.object(scope['socket'], 'create_connection', return_value=contextlib.nullcontext()), \\
     patch.object(scope['ssl'], 'create_default_context', return_value=TLS()), \\
     patch.object(scope['time'], 'sleep'):
    _, context = guard['verify_static'](paths, guard['parse_time']('2026-08-10T00:00:00.000Z'))
    guard['verify_dynamic'](paths, context)
print(json.dumps({'queries': queries}))
`;
  return spawnSync("python3", ["-c", program, guard, state.directory, JSON.stringify(options)], {
    encoding: "utf8",
  });
}

function realActive(path) {
  return realpathSync(path("/etc/caddy/Caddyfile"));
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}
