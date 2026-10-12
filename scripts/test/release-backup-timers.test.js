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
  if (args[0] === "is-enabled") {
    const state = JSON.parse(process.env.MAKO_TEST_ENABLED)[args.at(-1)] ?? "disabled";
    process.stdout.write(state + "\\n");
    process.exit(state === "enabled" ? 0 : 1);
  }
  if (args[0] === "is-active") {
    const state = JSON.parse(process.env.MAKO_TEST_ACTIVE)[args.at(-1)] ?? "inactive";
    process.stdout.write(state + "\\n");
    process.exit(state === "active" ? 0 : 3);
  }
  if (["start", "restart", "enable"].includes(args[0]) && args.some(a => a.includes("backup"))) process.exit(90);
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
    let controller = ["usage", "require_backups_disabled", "record_operation", "switch_release"]
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

function assertNoBackupStarts(calls) {
  assert.deepEqual(
    calls.filter(
      ({ command, args }) =>
        command === "systemctl" &&
        ["start", "restart", "enable"].includes(args[0]) &&
        args.some((arg) => arg.includes("backup")),
    ),
    [],
  );
}

for (const kind of ["upgrade", "rollback"]) {
  test(`${kind} with ordinary arguments creates no backup and leaves schedules stopped`, () => {
    const { result, calls, operations } = runSwitch(kind);
    assert.equal(result.status, 0, result.stderr);
    assertNoBackupStarts(calls);
    assert.deepEqual(
      calls
        .filter(({ command, args }) => command === "systemctl" && args[0] === "start")
        .flatMap(({ args }) => args.slice(1)),
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
    assert.equal(operations[0].checkpointPolicy, "disabled-for-deployments");
    assert.equal(operations[0].storageFencedBeforeSelection, true);
  });
}

for (const timer of timers) {
  for (const state of ["enabled", "unknown"]) {
    test(`deployment refuses ${state} ${timer}`, () => {
      const { result, calls, operations } = runSwitch("upgrade", { enabled: { [timer]: state } });
      assert.notEqual(result.status, 0);
      assertNoBackupStarts(calls);
      assert.equal(
        calls.some(({ command, args }) => command === "release" && args[0] === "select"),
        false,
      );
      assert.deepEqual(operations, []);
    });
  }
}
for (const unit of backupUnits) {
  test(`deployment refuses a running backup unit: ${unit}`, () => {
    const { result, calls, operations } = runSwitch("upgrade", { active: { [unit]: "active" } });
    assert.notEqual(result.status, 0);
    assertNoBackupStarts(calls);
    assert.equal(
      calls.some(({ command, args }) => command === "release" && args[0] === "select"),
      false,
    );
    assert.deepEqual(operations, []);
  });
}
for (const option of ["failStorage", "failReadiness"]) {
  test(`${option} still prevents a successful deployment without checkpoints`, () => {
    const { result, calls, operations } = runSwitch("upgrade", { [option]: true });
    assert.notEqual(result.status, 0);
    assertNoBackupStarts(calls);
    assert.deepEqual(operations, []);
  });
}
test("deployment has no per-run checkpoint option", () => {
  for (const option of ["--skip-checkpoints", "--backup"]) {
    const { result, calls } = runSwitch("upgrade", { extra: [option] });
    assert.equal(result.status, 2);
    assert.deepEqual(calls, []);
  }
});
