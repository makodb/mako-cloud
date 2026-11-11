// Run the actual upgrade/rollback controller and CLI dispatch with fake host
// commands and private release paths. No guest services or databases are used.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const source = readFileSync(
  resolve(import.meta.dirname, "../../infra/ansible/roles/runtime/files/mako-release-operation"),
  "utf8",
);
function definition(name) {
  const body = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "mu"))?.[0];
  assert.ok(body, `missing production function ${name}`);
  return body;
}
const timers = [
  "mako-checkpoint-backup@data-plane.timer",
  "mako-checkpoint-backup@control-plane.timer",
];
const backupUnits = [
  ...timers,
  ...["data-plane", "control-plane"].flatMap((component) => [
    `mako-checkpoint-backup@${component}.service`,
    `mako-checkpoint-backup-orchestrator@${component}.service`,
  ]),
];
const services = [
  "mako-data-plane",
  "mako-control-plane",
  "mako-edge-gateway",
  "mako-telemetry-query",
];
const current = "1".repeat(64);
const target = "2".repeat(64);

function runSwitch(
  kind,
  { enabled = {}, active = {}, failStorage = false, failReadiness = false, extra = [] } = {},
) {
  const root = mkdtempSync(join(tmpdir(), "mako-release-no-backups-"));
  try {
    for (const release of [current, target]) {
      const directory = join(root, "releases", release, "bin");
      mkdirSync(directory, { recursive: true });
      for (const binary of ["mako-storage-ops", "mako-control-storage-ops"]) {
        writeFileSync(join(directory, binary), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      }
    }
    writeFileSync(join(root, "calls.jsonl"), "");
    const shim = `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(process.env.MAKO_TEST_ROOT + "/calls.jsonl", JSON.stringify({command, args}) + "\\n");
if (command === "systemctl") {
  const statePath = process.env.MAKO_TEST_ROOT + "/systemd.json";
  const state = fs.existsSync(statePath)
    ? JSON.parse(fs.readFileSync(statePath, "utf8"))
    : { enabled: JSON.parse(process.env.MAKO_TEST_ENABLED), active: JSON.parse(process.env.MAKO_TEST_ACTIVE) };
  const units = args.filter(a => !a.startsWith("-")).slice(1);
  const save = () => fs.writeFileSync(statePath, JSON.stringify(state));
  if (args[0] === "is-enabled") {
    const value = state.enabled[args.at(-1)] ?? "disabled";
    process.stdout.write(value + "\\n");
    process.exit(value === "enabled" ? 0 : 1);
  }
  if (args[0] === "is-active") {
    const value = state.active[args.at(-1)] ?? "inactive";
    process.stdout.write(value + "\\n");
    process.exit(value === "active" ? 0 : 3);
  }
  // A worker or orchestrator a release operation starts is a backup it ran.
  if (["start", "restart"].includes(args[0]) && args.some(a => a.includes("backup"))) process.exit(90);
  if (args[0] === "disable") for (const unit of units) { state.enabled[unit] = "disabled"; if (args.includes("--now")) state.active[unit] = "inactive"; }
  if (args[0] === "enable") for (const unit of units) { state.enabled[unit] = "enabled"; if (args.includes("--now")) state.active[unit] = "active"; }
  if (args[0] === "stop") for (const unit of units) state.active[unit] = "inactive";
  save();
}
if (command === "install") fs.mkdirSync(args.at(-1), {recursive:true});
`;
    const env = {
      ...process.env,
      MAKO_TEST_ROOT: root,
      MAKO_TEST_ENABLED: JSON.stringify(enabled),
      MAKO_TEST_ACTIVE: JSON.stringify(active),
      MAKO_TEST_FAIL_STORAGE: String(failStorage),
      MAKO_TEST_FAIL_READINESS: String(failReadiness),
    };
    let controller = [
      "usage",
      "record_backup_schedule",
      "backup_schedule_json",
      "pause_backups",
      "require_backups_paused",
      "restore_backup_schedule",
      "record_operation",
      "switch_release",
    ]
      .map(definition)
      .join("\n");
    for (const [command, hostPath] of Object.entries({
      systemctl: "/usr/bin/systemctl",
      release: "/usr/local/sbin/mako-release",
      runuser: "/usr/sbin/runuser",
      install: "/usr/bin/install",
      chown: "/usr/bin/chown",
      event: "/unused/event",
    })) {
      const file = join(root, command);
      writeFileSync(file, shim, { mode: 0o755 });
      const name = `MAKO_TEST_${command.toUpperCase()}`;
      env[name] = file;
      controller = controller.replaceAll(hostPath, `"$${name}"`);
    }
    controller = controller.replaceAll(
      "/run/mako-release-operation.lock",
      '"$MAKO_TEST_ROOT/release.lock"',
    );
    const script = join(root, "operation.sh");
    writeFileSync(
      script,
      `set -euo pipefail
services=(${services.join(" ")})
timers=(${timers.join(" ")})
release_root="$MAKO_TEST_ROOT"
operation_root="$MAKO_TEST_ROOT/operations"
configuration_snapshot_root="$MAKO_TEST_ROOT/configuration"
control_lock="$MAKO_TEST_ROOT/control.lock"
control_database="$MAKO_TEST_ROOT/control.sqlite3"
control_identity=test-control
selected_release() { printf '%s' '${current}'; }
last_known_good() { printf '%s' '${target}'; }
require_safe_storage() { "$MAKO_TEST_EVENT" storage-compatible; [[ "$MAKO_TEST_FAIL_STORAGE" != true ]]; }
validate_configuration_snapshot() { "$MAKO_TEST_EVENT" validate-configuration; }
snapshot_current_configuration() { "$MAKO_TEST_EVENT" snapshot-configuration; }
restore_release_configuration() { "$MAKO_TEST_EVENT" restore-configuration; }
transition_storage() { "$MAKO_TEST_EVENT" "$2"; }
install_release_runtime_worker() { "$MAKO_TEST_EVENT" runtime-worker; }
wait_for_readiness() { "$MAKO_TEST_EVENT" readiness; [[ "$MAKO_TEST_FAIL_READINESS" != true ]]; }
prune_releases() { "$MAKO_TEST_EVENT" prune; }
${controller}
${source.slice(source.indexOf("command_name="))}
`,
    );
    const result = spawnSync(
      "bash",
      [script, kind, target, `--confirm=${kind.toUpperCase()}_RELEASE:${target}`, ...extra],
      { encoding: "utf8", env },
    );
    const calls = readFileSync(join(root, "calls.jsonl"), "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map(JSON.parse);
    const operations = calls.some(({ command }) => command === "install")
      ? readdirSync(join(root, "operations"))
          .filter((name) => name.endsWith(".json"))
          .map((name) => JSON.parse(readFileSync(join(root, "operations", name), "utf8")))
      : [];
    return { result, calls, operations };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function systemctl(calls, verb) {
  return calls.filter(({ command, args }) => command === "systemctl" && args[0] === verb);
}

/** A release operation never runs a backup: it never starts a worker or an
 * orchestrator, and it re-enables only timers that were enabled before. */
function assertNoBackupRuns(calls, enabledBefore = []) {
  assert.deepEqual(
    systemctl(calls, "start").filter(({ args }) => args.some((arg) => arg.includes("backup"))),
    [],
  );
  assert.deepEqual(
    systemctl(calls, "enable").flatMap(({ args }) =>
      args.filter((arg) => !arg.startsWith("-")).slice(1),
    ),
    enabledBefore,
  );
}

function eventIndex(calls, predicate) {
  return calls.findIndex(predicate);
}

for (const kind of ["upgrade", "rollback"]) {
  test(`${kind} with ordinary arguments creates no backup and leaves disabled schedules disabled`, () => {
    const { result, calls, operations } = runSwitch(kind);
    assert.equal(result.status, 0, result.stderr);
    assertNoBackupRuns(calls);
    assert.deepEqual(
      systemctl(calls, "start").flatMap(({ args }) => args.slice(1)),
      services.map((name) => `${name}.service`),
    );
    assert.deepEqual(
      calls.filter(({ command }) => command === "event").map(({ args }) => args[0]),
      [
        "storage-compatible",
        ...(kind === "rollback" ? ["validate-configuration"] : []),
        "snapshot-configuration",
        "fence",
        ...(kind === "rollback" ? ["restore-configuration"] : []),
        "runtime-worker",
        "activate",
        "readiness",
        "prune",
      ],
    );
    assert.equal(operations.length, 1);
    assert.equal(operations[0].kind, kind);
    assert.equal(operations[0].checkpointVerified, false);
    assert.equal(operations[0].checkpointSkipped, true);
    assert.equal(operations[0].checkpointPolicy, "paused-for-deployments");
    assert.deepEqual(operations[0].backupScheduleBefore, {
      [timers[0]]: "disabled",
      [timers[1]]: "disabled",
    });
    assert.deepEqual(operations[0].backupScheduleAfter, operations[0].backupScheduleBefore);
    assert.equal(operations[0].storageFencedBeforeSelection, true);
  });

  test(`${kind} pauses enabled schedules and a running backup, then restores the schedule once ready`, () => {
    const enabled = Object.fromEntries(timers.map((timer) => [timer, "enabled"]));
    const active = Object.fromEntries(backupUnits.map((unit) => [unit, "active"]));
    const { result, calls, operations } = runSwitch(kind, { enabled, active });
    assert.equal(result.status, 0, result.stderr);
    assertNoBackupRuns(calls, timers);
    const paused = eventIndex(
      calls,
      ({ command, args }) => command === "systemctl" && args[0] === "disable",
    );
    const stoppedWorker = eventIndex(
      calls,
      ({ command, args }) =>
        command === "systemctl" &&
        args[0] === "stop" &&
        args.includes("mako-checkpoint-backup@data-plane.service"),
    );
    const stoppedServices = eventIndex(
      calls,
      ({ command, args }) =>
        command === "systemctl" && args[0] === "stop" && args.includes("mako-data-plane.service"),
    );
    const ready = eventIndex(
      calls,
      ({ command, args }) => command === "event" && args[0] === "readiness",
    );
    const restored = eventIndex(
      calls,
      ({ command, args }) => command === "systemctl" && args[0] === "enable",
    );
    assert.ok(paused >= 0 && paused < stoppedServices, "timers pause before services stop");
    assert.ok(stoppedWorker >= 0 && stoppedWorker < stoppedServices, "a running backup is stopped");
    assert.ok(ready >= 0 && ready < restored, "the schedule returns only after readiness");
    assert.equal(operations.length, 1);
    assert.deepEqual(operations[0].backupScheduleBefore, {
      [timers[0]]: "enabled",
      [timers[1]]: "enabled",
    });
    assert.deepEqual(operations[0].backupScheduleAfter, operations[0].backupScheduleBefore);
  });
}

for (const timer of timers) {
  test(`deployment refuses unknown backup state for ${timer} before stopping anything`, () => {
    const { result, calls, operations } = runSwitch("upgrade", { enabled: { [timer]: "masked" } });
    assert.notEqual(result.status, 0);
    assertNoBackupRuns(calls);
    assert.deepEqual(systemctl(calls, "stop"), []);
    assert.equal(
      calls.some(({ command, args }) => command === "release" && args[0] === "select"),
      false,
    );
    assert.deepEqual(operations, []);
  });
}
for (const option of ["failStorage", "failReadiness"]) {
  test(`${option} prevents a successful deployment and leaves the schedule paused`, () => {
    const enabled = Object.fromEntries(timers.map((timer) => [timer, "enabled"]));
    const { result, calls, operations } = runSwitch("upgrade", { [option]: true, enabled });
    assert.notEqual(result.status, 0);
    assertNoBackupRuns(calls);
    assert.deepEqual(operations, []);
    if (option === "failReadiness") {
      assert.match(result.stderr, /schedule before the operation: \{.*"enabled".*\}/u);
    }
  });
}
test("deployment has no per-run checkpoint option", () => {
  for (const option of ["--skip-checkpoints", "--backup"]) {
    const { result, calls } = runSwitch("upgrade", { extra: [option] });
    assert.equal(result.status, 2);
    assert.deepEqual(calls, []);
  }
});
