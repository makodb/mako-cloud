#!/usr/bin/env node

import { execFile } from "node:child_process";
import { access, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { Resolver } from "node:dns/promises";

import { buildTeardownPlan } from "./public-beta-teardown-lib.js";

const execFileAsync = promisify(execFile);
const root = resolve(import.meta.dirname, "../..");
const options = parseOptions(process.argv.slice(2));
const discovery = options.fixture
  ? JSON.parse(await readFile(resolve(root, options.fixture), "utf8"))
  : await discover();
const plan = buildTeardownPlan(discovery);
const output = resolve(root, options.output ?? "docs/evidence/public-beta-teardown-plan.json");
await writeFile(output, `${JSON.stringify(plan, null, 2)}\n`, { mode: 0o644 });
console.log(`wrote non-executing teardown plan ${plan.planHash}`);
console.log(`plan: ${output}`);
console.log("public admission stop: verified active; no resource was deleted or revoked");

async function discover() {
  await access(resolve(root, ".local/ansible/public-beta-known-hosts"));
  const [host, guest, dns] = await Promise.all([discoverHost(), discoverGuest(), discoverDns()]);
  return {
    vm: host.vm,
    admission: host.admission,
    backups: host.backups,
    guest: guest.guest,
    dns,
    retainedEvidence: [
      ...host.evidence,
      ...guest.evidence,
      "docs/evidence/public-beta-teardown-plan.json",
    ].sort(),
  };
}

async function discoverHost() {
  const [status, config, admissionActive, admissionEnabled, edgeFilter, backups] =
    await Promise.all([
      hostCommand(["qm", "status", "124"]),
      hostCommand(["qm", "config", "124", "--current"]),
      hostCommand(["systemctl", "is-active", "mako-vm124-admission-stop.service"]),
      hostCommand(["systemctl", "is-enabled", "mako-vm124-admission-stop.service"]),
      hostCommand(["nft", "list", "table", "bridge", "mako_vm124_admission_stop"]),
      hostShell(
        "find /var/lib/vz/dump/mako-cloud-public-beta -maxdepth 3 -type f -printf '%P\\t%s\\n' | sort",
      ),
    ]);
  const configEntries = Object.fromEntries(
    config
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split(/: (.*)/s).slice(0, 2)),
  );
  const disks = Object.entries(configEntries)
    .filter(([key]) => /^(?:efidisk|ide|sata|scsi|virtio)\d+$/.test(key))
    .map(([slot, value]) => ({ slot, value }));
  return {
    vm: {
      id: 124,
      status: status.trim().split(/\s+/).at(-1),
      name: configEntries.name,
      node: "pve",
      disks,
    },
    admission: {
      serviceActive: admissionActive.trim() === "active",
      serviceEnabled: admissionEnabled.trim() === "enabled",
      edgeFilterInstalled: edgeFilter.includes("tcp dport { 80, 443 } drop"),
    },
    backups: parseTabInventory(backups),
    evidence: [
      "docs/evidence/public-beta-admission-stop.json",
      "docs/evidence/public-beta-backup-qualification.json",
      "docs/evidence/public-beta-restore-qualification.json",
    ],
  };
}

async function discoverGuest() {
  const [release, disks, credentials, certificates, operations] = await Promise.all([
    guestShell("sudo /usr/local/sbin/mako-release inspect"),
    guestShell("lsblk --json --output NAME,PATH,FSTYPE,LABEL,UUID,MOUNTPOINTS"),
    guestShell("sudo find /etc/mako/credentials -maxdepth 1 -type f -printf '%f\\n' | sort"),
    guestShell(
      "sudo find /var/lib/caddy -type f \\( -name '*.crt' -o -name '*.key' \\) -printf '%P\\n' 2>/dev/null | sort",
    ),
    guestShell(
      "sudo find /var/lib/mako-release-operations -maxdepth 1 -type f -name '*.json' -printf '%f\\n' | sort",
    ),
  ]);
  return {
    guest: {
      release: JSON.parse(release),
      blockDevices: JSON.parse(disks).blockdevices,
      credentials: credentials.split("\n").filter(Boolean),
      certificates: certificates.split("\n").filter(Boolean),
      releaseOperationEvidence: operations.split("\n").filter(Boolean),
    },
    evidence: [
      "docs/evidence/public-beta-release-rollback.json",
      "docs/evidence/public-beta-rollback-matrix.json",
      "docs/evidence/public-beta-observability.json",
    ],
  };
}

async function discoverDns() {
  const resolver = new Resolver();
  resolver.setServers(["8.8.8.8"]);
  const a = await resolver.resolve4("cloud-test.makodb.com").catch(() => []);
  const aaaa = await resolver.resolve6("cloud-test.makodb.com").catch(() => []);
  return { name: "cloud-test.makodb.com", a: a.sort(), aaaa: aaaa.sort() };
}

async function hostCommand(args) {
  const { stdout } = await execFileAsync(
    "ssh",
    ["-o", "BatchMode=yes", "root@localhost", "--", ...args],
    { encoding: "utf8" },
  );
  return stdout;
}

async function hostShell(command) {
  const { stdout } = await execFileAsync(
    "ssh",
    ["-o", "BatchMode=yes", "root@localhost", `bash -lc ${shellQuote(command)}`],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  );
  return stdout;
}

async function guestShell(command) {
  const { stdout } = await execFileAsync(
    "ssh",
    [
      "-o",
      `UserKnownHostsFile=${resolve(root, ".local/ansible/public-beta-known-hosts")}`,
      "-o",
      "StrictHostKeyChecking=yes",
      "mako-admin@130.245.173.11",
      `bash -lc ${shellQuote(command)}`,
    ],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  );
  return stdout;
}

function shellQuote(value) {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function parseTabInventory(value) {
  return value
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [path, size] = line.split("\t");
      return { path, sizeBytes: Number(size) };
    });
}

function parseOptions(args) {
  const options = {};
  for (const argument of args) {
    if (argument.startsWith("--fixture=")) options.fixture = argument.slice(10);
    else if (argument.startsWith("--output=")) options.output = argument.slice(9);
    else throw new Error(`unsupported argument: ${argument}`);
  }
  return options;
}
